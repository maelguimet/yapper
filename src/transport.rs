//! Controllable TTS playback transport (pure state machine + mpv IPC backend).
//!
//! Replaces fire-and-forget `ffplay` with Play / Pause / Stop / Replay / seek.

use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// High-level playback status shown in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportStatus {
    Idle,
    Buffering,
    Playing,
    Paused,
}

impl TransportStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Buffering => "buffering",
            Self::Playing => "speaking",
            Self::Paused => "paused",
        }
    }
}

/// Pure transport intent / transition table (no I/O).
#[derive(Debug, Clone, PartialEq)]
pub struct TransportMachine {
    pub status: TransportStatus,
    /// Seconds into the current timeline (whole utterance or active chunk).
    pub position_secs: f64,
    pub duration_secs: f64,
    /// Path of last successfully loaded audio (for Replay without re-synth).
    pub last_path: Option<PathBuf>,
    /// True when a queue is still synthesizing more segments.
    pub has_pending_queue: bool,
}

impl Default for TransportMachine {
    fn default() -> Self {
        Self {
            status: TransportStatus::Idle,
            position_secs: 0.0,
            duration_secs: 0.0,
            last_path: None,
            has_pending_queue: false,
        }
    }
}

impl TransportMachine {
    pub fn begin_buffering(&mut self) {
        self.status = TransportStatus::Buffering;
        self.position_secs = 0.0;
        self.duration_secs = 0.0;
    }

    pub fn on_audio_ready(&mut self, path: PathBuf, duration_secs: f64) {
        self.last_path = Some(path);
        self.duration_secs = duration_secs.max(0.0);
        self.position_secs = 0.0;
        self.status = TransportStatus::Playing;
    }

    pub fn pause(&mut self) {
        if self.status == TransportStatus::Playing {
            self.status = TransportStatus::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.status == TransportStatus::Paused {
            self.status = TransportStatus::Playing;
        }
    }

    pub fn stop(&mut self) {
        self.status = TransportStatus::Idle;
        self.position_secs = 0.0;
        // Keep last_path for replay.
        self.has_pending_queue = false;
    }

    /// Replay last path if any; returns the path to play or None.
    pub fn replay_request(&mut self) -> Option<PathBuf> {
        let path = self.last_path.clone()?;
        self.position_secs = 0.0;
        self.status = TransportStatus::Playing;
        Some(path)
    }

    /// Clamp seek into `[0, duration]`. Returns the clamped position.
    pub fn seek_to(&mut self, secs: f64) -> f64 {
        if self.duration_secs <= 0.0 {
            self.position_secs = 0.0;
            return 0.0;
        }
        let pos = secs.clamp(0.0, self.duration_secs);
        self.position_secs = pos;
        if self.status == TransportStatus::Idle && self.last_path.is_some() {
            self.status = TransportStatus::Paused;
        }
        pos
    }

    pub fn set_position(&mut self, secs: f64) {
        if self.duration_secs > 0.0 {
            self.position_secs = secs.clamp(0.0, self.duration_secs);
        } else {
            self.position_secs = secs.max(0.0);
        }
    }

    pub fn on_playback_ended(&mut self) {
        if self.has_pending_queue {
            self.status = TransportStatus::Buffering;
            self.position_secs = 0.0;
            self.duration_secs = 0.0;
        } else {
            self.status = TransportStatus::Idle;
            self.position_secs = self.duration_secs;
        }
    }

    pub fn progress_01(&self) -> f32 {
        if self.duration_secs <= 0.0 {
            return 0.0;
        }
        (self.position_secs / self.duration_secs).clamp(0.0, 1.0) as f32
    }

    pub fn format_time_label(&self) -> String {
        format!(
            "{} / {}",
            format_mmss(self.position_secs),
            format_mmss(self.duration_secs)
        )
    }
}

pub fn format_mmss(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "0:00".into();
    }
    let total = secs.floor() as u64;
    let m = total / 60;
    let s = total % 60;
    format!("{m}:{s:02}")
}

/// Controllable player using `mpv --input-ipc-server` when available.
pub struct AudioTransport {
    machine: TransportMachine,
    backend: Option<MpvBackend>,
    /// Volume 0.0..=1.0
    volume: f32,
}

impl Default for AudioTransport {
    fn default() -> Self {
        Self {
            machine: TransportMachine::default(),
            backend: None,
            volume: 1.0,
        }
    }
}

impl AudioTransport {
    pub fn machine(&self) -> &TransportMachine {
        &self.machine
    }

    pub fn status(&self) -> TransportStatus {
        self.machine.status
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        if let Some(b) = self.backend.as_mut() {
            let _ = b.set_volume((self.volume * 100.0) as i32);
        }
    }

    /// Play a single WAV/audio file (stops any current playback first).
    pub fn play_file(&mut self, path: &Path) -> Result<()> {
        self.stop_internal(false);
        if !path.is_file() {
            bail!("audio file missing: {}", path.display());
        }
        let duration = wav_duration_secs(path).unwrap_or(0.0);
        self.machine.begin_buffering();
        match MpvBackend::start(path, self.volume) {
            Ok(backend) => {
                self.backend = Some(backend);
                self.machine
                    .on_audio_ready(path.to_path_buf(), duration);
                Ok(())
            }
            Err(e) => {
                // Fallback: legacy fire-and-forget if mpv missing.
                if which_bin("ffplay") || which_bin("paplay") || which_bin("aplay") {
                    let child = crate::audio::play_wav(path)?;
                    self.backend = Some(MpvBackend::fallback_child(child));
                    self.machine
                        .on_audio_ready(path.to_path_buf(), duration);
                    Ok(())
                } else {
                    self.machine.stop();
                    Err(e)
                }
            }
        }
    }

    pub fn pause(&mut self) {
        if let Some(b) = self.backend.as_mut() {
            let _ = b.pause(true);
        }
        self.machine.pause();
    }

    pub fn resume(&mut self) {
        if let Some(b) = self.backend.as_mut() {
            let _ = b.pause(false);
        }
        self.machine.resume();
    }

    pub fn toggle_pause(&mut self) {
        match self.machine.status {
            TransportStatus::Playing => self.pause(),
            TransportStatus::Paused => self.resume(),
            _ => {}
        }
    }

    pub fn stop(&mut self) {
        self.stop_internal(true);
    }

    fn stop_internal(&mut self, keep_last: bool) {
        if let Some(mut b) = self.backend.take() {
            b.kill();
        }
        let last = if keep_last {
            self.machine.last_path.clone()
        } else {
            self.machine.last_path.clone()
        };
        self.machine.stop();
        if keep_last {
            self.machine.last_path = last;
        }
    }

    /// Replay last successful file without re-synthesis.
    ///
    /// Returns `Ok(false)` when there is no last path, or the file was deleted.
    pub fn replay(&mut self) -> Result<bool> {
        let Some(path) = self.machine.last_path.clone() else {
            return Ok(false);
        };
        if !path.is_file() {
            // Stale path after temp cleanup — clear so UI can show "nothing to replay".
            self.machine.last_path = None;
            return Ok(false);
        }
        self.play_file(&path)?;
        Ok(true)
    }

    /// Point Replay at a durable file without playing yet (e.g. after Stop cleanup).
    pub fn remember_path(&mut self, path: PathBuf) {
        if path.is_file() {
            self.machine.last_path = Some(path);
        }
    }

    pub fn seek_secs(&mut self, secs: f64) {
        let pos = self.machine.seek_to(secs);
        if let Some(b) = self.backend.as_mut() {
            let _ = b.seek_absolute(pos);
        }
    }

    pub fn seek_progress(&mut self, progress_01: f32) {
        let dur = self.machine.duration_secs;
        self.seek_secs(dur * progress_01.clamp(0.0, 1.0) as f64);
    }

    /// Poll backend; update position; detect end-of-file.
    pub fn tick(&mut self) {
        let ended = match self.backend.as_mut() {
            Some(b) => {
                if let Some(pos) = b.time_pos() {
                    self.machine.set_position(pos);
                }
                if let Some(dur) = b.duration() {
                    if dur > 0.0 {
                        self.machine.duration_secs = dur;
                    }
                }
                b.is_ended()
            }
            None => false,
        };
        if ended {
            if let Some(mut b) = self.backend.take() {
                b.kill();
            }
            self.machine.on_playback_ended();
        }
    }

    pub fn set_pending_queue(&mut self, pending: bool) {
        self.machine.has_pending_queue = pending;
        if pending && self.machine.status == TransportStatus::Idle {
            self.machine.begin_buffering();
        }
    }
}

struct MpvBackend {
    child: Child,
    ipc_path: Option<PathBuf>,
    /// When true, no IPC — only kill support (ffplay fallback).
    fallback: bool,
}

impl MpvBackend {
    fn start(path: &Path, volume: f32) -> Result<Self> {
        if !which_bin("mpv") {
            bail!("mpv not found");
        }
        let ipc = ipc_socket_path();
        if let Some(parent) = ipc.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::remove_file(&ipc);
        let vol = (volume.clamp(0.0, 1.0) * 100.0).round() as i32;
        let child = Command::new("mpv")
            .args([
                "--no-video",
                "--really-quiet",
                "--force-window=no",
                &format!("--volume={vol}"),
                &format!("--input-ipc-server={}", ipc.display()),
                path.to_str().context("path utf-8")?,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn mpv")?;
        // Wait briefly for socket.
        let deadline = Instant::now() + Duration::from_millis(800);
        while Instant::now() < deadline {
            if ipc.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(Self {
            child,
            ipc_path: Some(ipc),
            fallback: false,
        })
    }

    fn fallback_child(child: Child) -> Self {
        Self {
            child,
            ipc_path: None,
            fallback: true,
        }
    }

    fn connect(&self) -> Result<UnixStream> {
        let path = self
            .ipc_path
            .as_ref()
            .context("no ipc socket (fallback player)")?;
        UnixStream::connect(path).with_context(|| format!("connect {}", path.display()))
    }

    fn command_raw(&self, cmd: &str) -> Result<String> {
        if self.fallback {
            bail!("fallback player has no IPC");
        }
        let mut stream = self.connect()?;
        stream.set_read_timeout(Some(Duration::from_millis(300)))?;
        stream.set_write_timeout(Some(Duration::from_millis(300)))?;
        stream.write_all(cmd.as_bytes())?;
        stream.write_all(b"\n")?;
        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).unwrap_or(0);
        Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
    }

    fn pause(&mut self, paused: bool) -> Result<()> {
        if self.fallback {
            return Ok(());
        }
        let _ = self.command_raw(&format!(
            r#"{{"command":["set_property","pause",{}]}}"#,
            if paused { "true" } else { "false" }
        ));
        Ok(())
    }

    fn seek_absolute(&mut self, secs: f64) -> Result<()> {
        if self.fallback {
            return Ok(());
        }
        let _ = self.command_raw(&format!(
            r#"{{"command":["seek",{secs},"absolute"]}}"#
        ));
        Ok(())
    }

    fn set_volume(&mut self, vol: i32) -> Result<()> {
        if self.fallback {
            return Ok(());
        }
        let _ = self.command_raw(&format!(
            r#"{{"command":["set_property","volume",{vol}]}}"#
        ));
        Ok(())
    }

    fn time_pos(&mut self) -> Option<f64> {
        if self.fallback {
            return None;
        }
        let resp = self
            .command_raw(r#"{"command":["get_property","time-pos"]}"#)
            .ok()?;
        parse_mpv_data_f64(&resp)
    }

    fn duration(&mut self) -> Option<f64> {
        if self.fallback {
            return None;
        }
        let resp = self
            .command_raw(r#"{"command":["get_property","duration"]}"#)
            .ok()?;
        parse_mpv_data_f64(&resp)
    }

    fn is_ended(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(_) => true,
        }
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(p) = self.ipc_path.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

impl Drop for MpvBackend {
    fn drop(&mut self) {
        self.kill();
    }
}

fn parse_mpv_data_f64(resp: &str) -> Option<f64> {
    // {"data":12.3,"request_id":0,"error":"success"}
    let v: serde_json::Value = serde_json::from_str(resp.trim()).ok()?;
    if v.get("error").and_then(|e| e.as_str()) != Some("success") {
        return None;
    }
    v.get("data").and_then(|d| d.as_f64())
}

fn ipc_socket_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("yapper-mpv-{nanos}.sock"))
}

fn which_bin(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Best-effort duration from WAV header (PCM).
pub fn wav_duration_secs(path: &Path) -> Result<f64> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    wav_duration_from_bytes(&bytes)
}

pub fn wav_duration_from_bytes(bytes: &[u8]) -> Result<f64> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a WAV");
    }
    let mut offset = 12usize;
    let mut sample_rate = 0u32;
    let mut channels = 1u16;
    let mut bits = 16u16;
    let mut data_bytes = 0u32;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let data_start = offset + 8;
        let data_end = data_start.saturating_add(size).min(bytes.len());
        if id == b"fmt " && size >= 16 {
            channels = u16::from_le_bytes(bytes[data_start + 2..data_start + 4].try_into().unwrap());
            sample_rate =
                u32::from_le_bytes(bytes[data_start + 4..data_start + 8].try_into().unwrap());
            bits = u16::from_le_bytes(bytes[data_start + 14..data_start + 16].try_into().unwrap());
        } else if id == b"data" {
            data_bytes = size as u32;
            break;
        }
        offset = data_end + (size % 2);
        if size == 0 {
            break;
        }
    }
    if sample_rate == 0 || channels == 0 || bits == 0 {
        bail!("incomplete WAV fmt");
    }
    let bytes_per_frame = (channels as u32) * (bits as u32 / 8);
    if bytes_per_frame == 0 {
        bail!("bad frame size");
    }
    Ok(data_bytes as f64 / (sample_rate as f64 * bytes_per_frame as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_play_pause_resume_stop() {
        let mut m = TransportMachine::default();
        assert_eq!(m.status, TransportStatus::Idle);
        m.begin_buffering();
        assert_eq!(m.status, TransportStatus::Buffering);
        m.on_audio_ready(PathBuf::from("/tmp/a.wav"), 10.0);
        assert_eq!(m.status, TransportStatus::Playing);
        assert_eq!(m.duration_secs, 10.0);
        m.pause();
        assert_eq!(m.status, TransportStatus::Paused);
        m.resume();
        assert_eq!(m.status, TransportStatus::Playing);
        m.stop();
        assert_eq!(m.status, TransportStatus::Idle);
        assert!(m.last_path.is_some());
    }

    #[test]
    fn machine_seek_bounds() {
        let mut m = TransportMachine::default();
        m.on_audio_ready(PathBuf::from("/tmp/a.wav"), 12.0);
        assert_eq!(m.seek_to(-5.0), 0.0);
        assert_eq!(m.seek_to(100.0), 12.0);
        assert_eq!(m.seek_to(3.5), 3.5);
        assert!((m.progress_01() - (3.5 / 12.0) as f32).abs() < 0.001);
    }

    #[test]
    fn machine_replay_without_resynth() {
        let mut m = TransportMachine::default();
        assert!(m.replay_request().is_none());
        m.on_audio_ready(PathBuf::from("/tmp/last.wav"), 5.0);
        m.stop();
        // last_path survives stop for Replay without re-synth
        assert_eq!(m.last_path, Some(PathBuf::from("/tmp/last.wav")));
        let p = m.replay_request().unwrap();
        assert_eq!(p, PathBuf::from("/tmp/last.wav"));
        assert_eq!(m.status, TransportStatus::Playing);
        assert_eq!(m.position_secs, 0.0);
    }

    #[test]
    fn transport_replay_false_when_file_missing() {
        let mut t = AudioTransport::default();
        t.machine.last_path = Some(PathBuf::from("/tmp/yapper-definitely-missing-replay.wav"));
        assert_eq!(t.replay().unwrap(), false);
        assert!(t.machine.last_path.is_none());
    }

    #[test]
    fn machine_end_with_pending_goes_buffering() {
        let mut m = TransportMachine::default();
        m.on_audio_ready(PathBuf::from("/tmp/a.wav"), 2.0);
        m.has_pending_queue = true;
        m.on_playback_ended();
        assert_eq!(m.status, TransportStatus::Buffering);
    }

    #[test]
    fn format_mmss_values() {
        assert_eq!(format_mmss(0.0), "0:00");
        assert_eq!(format_mmss(65.2), "1:05");
        assert_eq!(format_mmss(-1.0), "0:00");
    }

    #[test]
    fn status_labels() {
        assert_eq!(TransportStatus::Playing.as_str(), "speaking");
        assert_eq!(TransportStatus::Paused.as_str(), "paused");
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
    fn time_label_format() {
        let mut m = TransportMachine::default();
        m.on_audio_ready(PathBuf::from("/tmp/a.wav"), 90.0);
        m.set_position(45.0);
        assert_eq!(m.format_time_label(), "0:45 / 1:30");
    }
}
