//! Pure UI string/path helpers (unit-tested, no app state).

use std::path::PathBuf;

/// Soft warn when TTS paste is large (still allowed).
pub const TTS_LONG_TEXT_WARN_CHARS: usize = 800;
/// Hard-ish warn threshold for very long monologues.
pub const TTS_VERY_LONG_TEXT_CHARS: usize = 2_500;
/// Minimum multiline rows for transcript / TTS; grows with available height.
pub const TEXT_PANEL_MIN_ROWS: usize = 6;
pub const TEXT_PANEL_MAX_ROWS: usize = 28;
const TEXT_ROW_HEIGHT_EST: f32 = 18.0;

/// TTS character-count label + optional length warning (pure, tested).
pub fn tts_text_stats(text: &str) -> (String, Option<String>) {
    let n = text.chars().count();
    let label = if n == 1 {
        "1 character".into()
    } else {
        format!("{n} characters")
    };
    let warn = if n >= TTS_VERY_LONG_TEXT_CHARS {
        Some(format!(
            "Very long text ({n} chars) — synthesis may take a while; streaming splits by sentence."
        ))
    } else if n >= TTS_LONG_TEXT_WARN_CHARS {
        Some(format!(
            "Long paste ({n} chars) — first audio after the first sentence when streaming."
        ))
    } else {
        None
    };
    (label, warn)
}

/// Empty-state guidance when a work action cannot run yet.
/// Model loads auto-run on Record/Speak — do not claim Settings is required.
pub fn stt_empty_guidance(stt_loaded: bool, mic_ok: bool) -> Option<&'static str> {
    if !mic_ok {
        return Some("Select a microphone before recording.");
    }
    if !stt_loaded {
        return Some("STT loads on first transcription.");
    }
    None
}

pub fn tts_empty_guidance(tts_loaded: bool, text_empty: bool) -> Option<&'static str> {
    if text_empty {
        return Some("Paste or type text to speak.");
    }
    if !tts_loaded {
        return Some("TTS loads on first Speak.");
    }
    None
}

/// Chunk temp files safe to delete after Stop; keep `last_success` for Replay.
pub fn chunk_paths_to_remove(
    chunk_paths: &[PathBuf],
    last_success: Option<&std::path::Path>,
) -> Vec<PathBuf> {
    chunk_paths
        .iter()
        .filter(|p| match last_success {
            Some(keep) => p.as_path() != keep,
            None => true,
        })
        .cloned()
        .collect()
}

/// Surviving paths after a stop cleanup (0 or 1 entries: the replay file).
pub fn chunk_paths_retained_for_replay(
    chunk_paths: &[PathBuf],
    last_success: Option<&std::path::Path>,
) -> Vec<PathBuf> {
    match last_success {
        Some(keep) if chunk_paths.iter().any(|p| p.as_path() == keep) => {
            vec![keep.to_path_buf()]
        }
        Some(keep) => vec![keep.to_path_buf()],
        None => Vec::new(),
    }
}

/// Resolve the file path Replay should play (must exist on disk).
pub fn resolve_replay_path(
    last_success: Option<&std::path::Path>,
    transport_last: Option<&std::path::Path>,
) -> Option<PathBuf> {
    if let Some(p) = last_success {
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    if let Some(p) = transport_last {
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    None
}

/// Ellipsize long display strings for combo boxes (ASCII `...` only — B26 font-safe).
pub fn truncate_display(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }
    let keep = max_chars - 3;
    let mut out: String = s.chars().take(keep).collect();
    out.push_str("...");
    out
}

/// Symmetric load badge text: `STT * medium` / `TTS - unloaded`.
pub fn load_status_label(role: &str, loaded: bool, model_id: Option<&str>) -> String {
    if loaded {
        match model_id.map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => format!("{role} * {id}"),
            None => format!("{role} * loaded"),
        }
    } else {
        format!("{role} - unloaded")
    }
}

/// Estimate multiline TextEdit rows from available height (share of remaining panel).
pub fn text_panel_rows(available_height: f32, share: f32) -> usize {
    let budget = (available_height * share).max(TEXT_PANEL_MIN_ROWS as f32 * TEXT_ROW_HEIGHT_EST);
    let rows = (budget / TEXT_ROW_HEIGHT_EST).floor() as usize;
    rows.clamp(TEXT_PANEL_MIN_ROWS, TEXT_PANEL_MAX_ROWS)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            stt.contains("loads on first") || stt.contains("transcription"),
            "{stt}"
        );
        assert!(
            !stt.contains("settings"),
            "must not require Settings for autoload path: {stt}"
        );
        assert!(stt_empty_guidance(true, false)
            .unwrap()
            .to_ascii_lowercase()
            .contains("microphone"));
        assert!(stt_empty_guidance(true, true).is_none());
        let tts = tts_empty_guidance(false, false).unwrap().to_ascii_lowercase();
        assert!(
            tts.contains("loads on first") || tts.contains("speak"),
            "{tts}"
        );
        assert!(!tts.contains("settings"), "{tts}");
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
}
