//! Unit tests for pure UI helpers (sibling of production helpers.rs).

use super::*;

#[test]
fn primary_tabs_are_friendly_not_stt_tts() {
    let tabs = primary_tab_labels();
    assert_eq!(tabs, ["Dictate", "Speak", "Settings"]);
    for t in tabs {
        assert!(!t.eq_ignore_ascii_case("STT"), "{t}");
        assert!(!t.eq_ignore_ascii_case("TTS"), "{t}");
        assert!(!t.contains("STT"), "{t}");
        assert!(!t.contains("TTS"), "{t}");
    }
}

#[test]
fn stt_ready_requires_matching_selected_model() {
    assert!(!stt_ready_for_selected(false, None, "small"));
    assert!(!stt_ready_for_selected(true, Some("small"), "medium"));
    assert!(!stt_ready_for_selected(true, None, "small"));
    assert!(stt_ready_for_selected(true, Some("medium"), "medium"));
    assert!(stt_ready_for_selected(true, Some("small"), "small"));
}

#[test]
fn speak_restart_requires_oob_kill_when_synth_in_flight() {
    assert!(speak_restart_needs_oob_kill(true));
    assert!(!speak_restart_needs_oob_kill(false));
    assert_eq!(speak_action_label(true), "Restart");
    assert_eq!(speak_action_label(false), "Speak");
    assert!(can_stop_tts(true));
    assert!(!can_stop_tts(false));
    assert!(can_replay_tts(false, true));
    assert!(!can_replay_tts(true, true));
    assert!(!can_replay_tts(false, false));
}

#[test]
fn dictation_chip_shows_selected_vs_active_mismatch() {
    let s = dictation_chip_label(false, true, Some("small"), "medium");
    assert!(s.contains("small"), "{s}");
    assert!(s.contains("medium"), "{s}");
    assert_eq!(
        dictation_chip_label(false, false, None, "small"),
        "Dictation: unloaded"
    );
    assert!(dictation_chip_label(true, false, None, "medium").contains("loading"));
}

#[test]
fn transport_idle_hides_zero_time() {
    assert_eq!(
        transport_status_line(false, None, 0, true, "0:00 / 0:00", false),
        "Idle"
    );
    let s = transport_status_line(true, Some(1), 9, false, "0:12 / 0:22", false);
    assert!(s.contains("2/9"), "{s}");
    assert!(s.contains("0:12"), "{s}");
    let synth = transport_status_line(true, None, 5, true, "0:00 / 0:00", true);
    assert!(synth.contains("Synthesizing"), "{synth}");
    assert!(synth.contains("1/5"), "{synth}");
    assert!(!synth.contains("0:00 / 0:00"), "{synth}");
}

#[test]
fn synth_errors_always_reset_worker_flag() {
    assert!(synth_error_resets_worker());
}

#[test]
fn fallback_tones_non_empty() {
    let t = fallback_tones();
    assert!(t.contains(&"neutral".into()));
    assert!(t.len() >= 3);
}

#[test]
fn truncate_preserves_short_strings() {
    assert_eq!(truncate_display("TONOR", 42), "TONOR");
    assert_eq!(truncate_display("", 10), "");
    assert_eq!(truncate_display("abc", 3), "abc");
}

#[test]
fn truncate_ellipsizes_long_device_strings() {
    let long = "alsa_input.usb-TONOR_INC._TONOR_TC30_XXXX-00.analog-stereo";
    let out = truncate_display(long, 20);
    assert!(out.ends_with("..."), "{out}");
    assert!(out.chars().count() <= 20, "{out}");
    assert!(out.starts_with("alsa"), "{out}");
}

#[test]
fn load_status_symmetric_with_model_id() {
    assert_eq!(
        load_status_label("STT", true, Some("medium")),
        "STT * medium"
    );
    assert_eq!(
        load_status_label("TTS", true, Some("chatterbox-multilingual")),
        "TTS * chatterbox-multilingual"
    );
    assert_eq!(load_status_label("STT", false, None), "STT - unloaded");
    assert_eq!(load_status_label("TTS", false, Some("x")), "TTS - unloaded");
    assert_eq!(load_status_label("STT", true, Some("")), "STT * loaded");
    assert_eq!(load_status_label("TTS", true, None), "TTS * loaded");
}

#[test]
fn text_panel_rows_clamped() {
    assert_eq!(text_panel_rows(50.0, 0.28), TEXT_PANEL_MIN_ROWS);
    assert!(text_panel_rows(2000.0, 0.5) <= TEXT_PANEL_MAX_ROWS);
    assert!(text_panel_rows(400.0, 0.28) >= TEXT_PANEL_MIN_ROWS);
}

#[test]
fn tts_text_stats_counts_and_warns() {
    let (s, w) = tts_text_stats("hi");
    assert_eq!(s, "2 characters");
    assert!(w.is_none());
    let (s1, _) = tts_text_stats("x");
    assert_eq!(s1, "1 character");
    let long: String = "a".repeat(TTS_LONG_TEXT_WARN_CHARS);
    let (_, w) = tts_text_stats(&long);
    assert!(w.unwrap().contains("Long paste"));
    let huge: String = "b".repeat(TTS_VERY_LONG_TEXT_CHARS);
    let (_, w2) = tts_text_stats(&huge);
    assert!(w2.unwrap().contains("Very long"));
}

#[test]
fn empty_guidance_for_stt_and_tts() {
    let stt = stt_empty_guidance(false, true).unwrap().to_ascii_lowercase();
    assert!(
        stt.contains("loads on first") || stt.contains("dictation"),
        "{stt}"
    );
    assert!(
        !stt.contains("settings"),
        "must not require Settings for autoload path: {stt}"
    );
    assert!(!stt_guidance_is_warning(false, true));
    assert!(stt_guidance_is_warning(true, false));
    assert!(stt_empty_guidance(true, false)
        .unwrap()
        .to_ascii_lowercase()
        .contains("microphone"));
    assert!(stt_empty_guidance(true, true).is_none());
    let tts = tts_empty_guidance(false, false).unwrap().to_ascii_lowercase();
    assert!(
        tts.contains("loads on first") || tts.contains("speak") || tts.contains("voice"),
        "{tts}"
    );
    assert!(!tts.contains("settings"), "{tts}");
    assert!(!tts_guidance_is_warning(false, false));
    assert!(tts_empty_guidance(true, true)
        .unwrap()
        .to_ascii_lowercase()
        .contains("paste"));
    assert!(tts_empty_guidance(true, false).is_none());
}

#[test]
fn stop_cleanup_keeps_last_success_for_replay() {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("yapper-replay-keep-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("chunk-a.wav");
    let b = dir.join("chunk-b.wav");
    let c = dir.join("chunk-c.wav");
    for p in [&a, &b, &c] {
        std::fs::write(p, b"RIFF....WAVE").unwrap();
    }
    let chunks = vec![a.clone(), b.clone(), c.clone()];
    let last = c.clone();

    let remove = chunk_paths_to_remove(&chunks, Some(last.as_path()));
    assert_eq!(remove, vec![a.clone(), b.clone()]);
    let keep = chunk_paths_retained_for_replay(&chunks, Some(last.as_path()));
    assert_eq!(keep, vec![c.clone()]);

    for p in remove {
        std::fs::remove_file(p).unwrap();
    }
    assert!(!a.is_file());
    assert!(!b.is_file());
    assert!(c.is_file(), "last success must survive Stop");

    let resolved = resolve_replay_path(Some(c.as_path()), Some(c.as_path()));
    assert_eq!(resolved, Some(c.clone()));
    std::fs::remove_file(&c).unwrap();
    assert!(resolve_replay_path(Some(c.as_path()), Some(c.as_path())).is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn resolve_replay_prefers_durable_last_over_stale_transport() {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("yapper-replay-pref-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    let good = dir.join("good.wav");
    let stale = dir.join("stale.wav");
    std::fs::write(&good, b"RIFF").unwrap();
    let r = resolve_replay_path(Some(good.as_path()), Some(stale.as_path()));
    assert_eq!(r, Some(good));
    let _ = std::fs::remove_dir_all(&dir);
}
