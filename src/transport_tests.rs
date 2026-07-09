//! Live transport tests (mpv playlist append, long-read continuity).
//! Included as `transport::tests` via `#[path]` so production `transport.rs` stays under the hard line cap.

use super::*;
use crate::mpv_backend::{wav_duration_secs, which_bin};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn transport_replay_false_when_file_missing() {
    let mut t = AudioTransport::default();
    t.machine.last_path = Some(PathBuf::from("/tmp/yapper-definitely-missing-replay.wav"));
    assert_eq!(t.replay().unwrap(), false);
    assert!(t.machine.last_path.is_none());
}

#[test]
fn wav_duration_from_minimal_pcm() {
    // 16000 Hz mono 16-bit, 16000 samples = 1.0s
    let samples = 16000usize;
    let mut data = Vec::new();
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(b"WAVE");
    data.extend_from_slice(b"fmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&16000u32.to_le_bytes());
    data.extend_from_slice(&32000u32.to_le_bytes());
    data.extend_from_slice(&2u16.to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes());
    data.extend_from_slice(b"data");
    let pcm_len = samples * 2;
    data.extend_from_slice(&(pcm_len as u32).to_le_bytes());
    data.extend(std::iter::repeat(0u8).take(pcm_len));
    let riff = (data.len() as u32) - 8;
    data[4..8].copy_from_slice(&riff.to_le_bytes());
    let dur = wav_duration_from_bytes(&data).unwrap();
    assert!((dur - 1.0).abs() < 0.01, "dur={dur}");
}

#[test]
fn stop_internal_keep_last_semantics() {
    let mut t = AudioTransport::default();
    t.machine.last_path = Some(PathBuf::from("/tmp/keep-me.wav"));
    t.stop_internal(true);
    assert_eq!(
        t.machine.last_path,
        Some(PathBuf::from("/tmp/keep-me.wav")),
        "keep_last=true must preserve last_path"
    );
    t.machine.last_path = Some(PathBuf::from("/tmp/drop-me.wav"));
    t.stop_internal(false);
    assert!(
        t.machine.last_path.is_none(),
        "keep_last=false must clear last_path"
    );
}

#[test]
fn clear_last_path_discards_replay() {
    let mut t = AudioTransport::default();
    t.machine.last_path = Some(PathBuf::from("/tmp/x.wav"));
    t.clear_last_path();
    assert!(t.machine.last_path.is_none());
}

/// Write a short silent mono PCM WAV (16-bit, 16 kHz).
fn write_silence_wav(path: &Path, duration_secs: f64) {
    let sample_rate = 16_000u32;
    let samples = ((duration_secs * sample_rate as f64).round() as usize).max(160);
    let pcm_len = samples * 2;
    let mut data = Vec::with_capacity(44 + pcm_len);
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(b"WAVE");
    data.extend_from_slice(b"fmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes()); // PCM
    data.extend_from_slice(&1u16.to_le_bytes()); // mono
    data.extend_from_slice(&sample_rate.to_le_bytes());
    data.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    data.extend_from_slice(&2u16.to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&(pcm_len as u32).to_le_bytes());
    data.extend(std::iter::repeat(0u8).take(pcm_len));
    let riff = (data.len() as u32) - 8;
    data[4..8].copy_from_slice(&riff.to_le_bytes());
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(&data).unwrap();
}

fn scratch_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("yapper-transport-test-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn enqueue_multi_chunk_uses_one_session_when_mpv_available() {
    if !which_bin("mpv") {
        eprintln!("skip: mpv not installed");
        return;
    }
    let dir = scratch_dir();
    let paths: Vec<PathBuf> = (0..4)
        .map(|i| {
            let p = dir.join(format!("c{i}.wav"));
            write_silence_wav(&p, 0.25);
            p
        })
        .collect();

    let mut t = AudioTransport::default();
    t.set_pending_queue(true);
    for (i, p) in paths.iter().enumerate() {
        t.enqueue_or_play(p).unwrap_or_else(|e| panic!("chunk {i}: {e:#}"));
    }
    assert!(
        t.files_on_session() >= 2,
        "expected appends on one session, got files_on_session={}",
        t.files_on_session()
    );
    assert!(
        t.supports_transport_controls(),
        "mpv IPC session should back controls"
    );
    assert_eq!(t.status(), TransportStatus::Playing);
    let playlist = t
        .backend
        .as_mut()
        .and_then(|b| b.playlist_count())
        .expect("playlist-count via IPC");
    assert!(
        playlist >= 2,
        "mpv playlist-count should reflect appends, got {playlist}"
    );

    // Pause / resume / seek stay on the same backend.
    t.pause();
    assert_eq!(t.status(), TransportStatus::Paused);
    t.resume();
    assert_eq!(t.status(), TransportStatus::Playing);
    t.seek_secs(0.05);

    // Stop mid-queue: kill backend, idle, no further play of stale queue.
    t.set_pending_queue(false);
    t.stop();
    assert_eq!(t.status(), TransportStatus::Idle);
    assert_eq!(t.files_on_session(), 0);
    assert!(t.backend.is_none());
    // Stale: ready paths still on disk but transport must not auto-play.
    t.tick();
    assert_eq!(t.status(), TransportStatus::Idle);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn enqueue_full_playlist_then_replay_full_concat() {
    if !which_bin("mpv") {
        eprintln!("skip: mpv not installed");
        return;
    }
    let dir = scratch_dir();
    let paths: Vec<PathBuf> = (0..3)
        .map(|i| {
            let p = dir.join(format!("r{i}.wav"));
            write_silence_wav(&p, 0.2);
            p
        })
        .collect();
    let mut t = AudioTransport::default();
    t.set_pending_queue(true);
    for p in &paths {
        t.enqueue_or_play(p).unwrap();
    }
    let session_files = t.files_on_session();
    assert!(session_files >= 2, "session_files={session_files}");

    // Build full-utterance concat (same path finalize_tts_job uses).
    let full = dir.join("full.wav");
    crate::wavutil::concat_wav_files(&paths, &full).unwrap();
    let full_dur = wav_duration_secs(&full).unwrap();
    let sum: f64 = paths
        .iter()
        .map(|p| wav_duration_secs(p).unwrap())
        .sum();
    assert!(
        (full_dur - sum).abs() < 0.05,
        "concat dur {full_dur} vs sum {sum}"
    );

    t.set_pending_queue(false);
    t.stop();
    t.remember_path(full.clone());
    assert!(t.replay().unwrap());
    assert_eq!(t.files_on_session(), 1, "replay is one full file");
    let replay_path = t.machine().last_path.clone().unwrap();
    assert_eq!(replay_path, full);
    let play_dur = t.machine().duration_secs;
    assert!(
        play_dur + 0.05 >= sum * 0.9,
        "replay duration {play_dur} should cover full utterance ~{sum}"
    );
    t.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Host long-read continuity: ≥20 short segments on one mpv session, ordered
/// enqueue, stop mid-queue (no stale play), full-utterance replay duration.
#[test]
fn long_read_twenty_chunks_one_session_stop_and_replay() {
    if !which_bin("mpv") {
        eprintln!("skip: mpv not installed");
        return;
    }
    const N: usize = 22;
    let dir = scratch_dir();
    let mut paths = Vec::with_capacity(N);
    let mut sum_dur = 0.0f64;
    for i in 0..N {
        let p = dir.join(format!("s{i:02}.wav"));
        write_silence_wav(&p, 0.15);
        let d = wav_duration_secs(&p).unwrap();
        sum_dur += d;
        paths.push(p);
        eprintln!("chunk {i}/{N} ready path={}", paths[i].display());
    }

    let mut t = AudioTransport::default();
    t.set_pending_queue(true);
    let mut completed = 0usize;
    for (i, p) in paths.iter().enumerate() {
        t.enqueue_or_play(p)
            .unwrap_or_else(|e| panic!("missed chunk {i}: {e:#}"));
        completed += 1;
        eprintln!(
            "chunk {i} enqueued; session_files={} status={:?}",
            t.files_on_session(),
            t.status()
        );
    }
    assert_eq!(completed, N, "every chunk index accounted for in order");
    assert_eq!(
        t.files_on_session() as usize,
        N,
        "one persistent session must hold all {N} files (no per-chunk respawn)"
    );
    assert_eq!(t.status(), TransportStatus::Playing);

    // Mid-run Stop: backend dead, idle, no further audio scheduled.
    t.set_pending_queue(false);
    t.stop();
    assert_eq!(t.status(), TransportStatus::Idle);
    assert!(t.backend.is_none(), "stop must kill player");
    assert_eq!(t.files_on_session(), 0);
    t.tick();
    assert_eq!(
        t.status(),
        TransportStatus::Idle,
        "no stale advance after stop"
    );
    eprintln!("stop mid-queue: idle, backend gone");

    // Full replay via concat (same durable path Speak finalize uses).
    let full = dir.join("long-full.wav");
    crate::wavutil::concat_wav_files(&paths, &full).unwrap();
    let full_dur = wav_duration_secs(&full).unwrap();
    assert!(
        (full_dur - sum_dur).abs() < 0.1,
        "full_dur={full_dur} sum={sum_dur}"
    );
    t.remember_path(full.clone());
    assert!(t.replay().unwrap(), "replay must start");
    assert_eq!(t.machine().last_path.as_deref(), Some(full.as_path()));
    assert!(
        t.machine().duration_secs + 0.05 >= sum_dur * 0.9,
        "replay duration {} < 90% of sum {}",
        t.machine().duration_secs,
        sum_dur
    );
    eprintln!(
        "replay full ok: duration={} sum_chunks={sum_dur} files_on_session={}",
        t.machine().duration_secs,
        t.files_on_session()
    );
    t.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
