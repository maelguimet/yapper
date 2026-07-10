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

/// Fallback tones when async list has not arrived (or fails).
pub fn fallback_tones() -> Vec<String> {
    vec![
        "neutral".into(),
        "calm".into(),
        "excited".into(),
        "serious".into(),
    ]
}

/// Primary product tab labels (never STT/TTS).
pub fn primary_tab_labels() -> [&'static str; 3] {
    ["Dictate", "Speak", "Settings"]
}

/// True when loaded STT weights match the Settings selector.
pub fn stt_ready_for_selected(
    stt_loaded: bool,
    loaded_model: Option<&str>,
    selected_model: &str,
) -> bool {
    stt_loaded
        && loaded_model
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_some_and(|m| m == selected_model)
}

/// Whether Speak restart must use out-of-band kill (same path as Stop).
pub fn speak_restart_needs_oob_kill(synth_in_flight: bool) -> bool {
    synth_in_flight
}

/// Speak primary button label when a job/playback is active.
pub fn speak_action_label(tts_busy: bool) -> &'static str {
    if tts_busy {
        "Restart"
    } else {
        "Speak"
    }
}

/// Stop enabled only while synth or playback is active.
pub fn can_stop_tts(tts_busy: bool) -> bool {
    tts_busy
}

/// Replay when idle and a replayable file exists.
pub fn can_replay_tts(tts_busy: bool, replay_exists: bool) -> bool {
    !tts_busy && replay_exists
}

/// Dictation chip label; distinguishes active vs selected when they differ.
pub fn dictation_chip_label(
    loading: bool,
    loaded: bool,
    active_model: Option<&str>,
    selected_model: &str,
) -> String {
    if loading {
        return format!("Dictation: loading {selected_model}");
    }
    if !loaded {
        return "Dictation: unloaded".into();
    }
    let active = active_model.unwrap_or("loaded");
    if active != selected_model {
        format!("Dictation: {active} (selected {selected_model})")
    } else {
        format!("Dictation: {active}")
    }
}

pub fn voice_chip_label(loading: bool, loaded: bool, model: Option<&str>) -> String {
    if loading {
        return "Voice: loading".into();
    }
    if !loaded {
        return "Voice: unloaded".into();
    }
    match model.map(str::trim).filter(|s| !s.is_empty()) {
        Some(m) if m.len() > 18 => "Voice: loaded".into(),
        Some(m) => format!("Voice: {m}"),
        None => "Voice: loaded".into(),
    }
}

/// Transport chrome: never lead with idle `0:00/0:00`.
pub fn transport_status_line(
    has_active_job: bool,
    playing_index: Option<usize>,
    total: usize,
    transport_idle: bool,
    time_label: &str,
    synth_in_flight: bool,
) -> String {
    if !has_active_job && transport_idle {
        return "Idle".into();
    }
    if has_active_job && total > 0 {
        let n = playing_index.map(|i| i + 1).unwrap_or_else(|| {
            // Synthesizing first chunk before any play.
            1usize.min(total)
        });
        let phase = if synth_in_flight && transport_idle {
            "Synthesizing"
        } else if transport_idle {
            "Buffering"
        } else {
            "Speaking"
        };
        if transport_idle && time_label.contains("0:00") {
            return format!("{phase} {n}/{total}");
        }
        return format!("{phase} {n}/{total} · sentence {time_label}");
    }
    if transport_idle {
        "Idle".into()
    } else {
        format!("Playing · {time_label}")
    }
}

/// TTS character-count label + optional length warning (pure, tested).
pub fn tts_text_stats(text: &str) -> (String, Option<String>) {
    let n = text.chars().count();
    let label = if n == 1 {
        "1 character".into()
    } else {
        format!("{n} characters")
    };
    let warn = if n >= TTS_VERY_LONG_TEXT_CHARS {
        Some(format!("Very long ({n} chars) — may take a while."))
    } else if n >= TTS_LONG_TEXT_WARN_CHARS {
        Some(format!("Long paste ({n} chars) — streams by sentence."))
    } else {
        None
    };
    (label, warn)
}

/// Empty-state guidance when a work action cannot run yet.
/// Quiet helper — not a yellow error. Model loads on first use.
pub fn stt_empty_guidance(stt_loaded: bool, mic_ok: bool) -> Option<&'static str> {
    if !mic_ok {
        return Some("Select a microphone before recording.");
    }
    if !stt_loaded {
        return Some("Dictation model loads on first use.");
    }
    None
}

/// Whether unloaded-model guidance should use warning chrome (yellow).
/// Mic failures are real warnings; unloaded model is not.
pub fn stt_guidance_is_warning(stt_loaded: bool, mic_ok: bool) -> bool {
    !mic_ok && stt_empty_guidance(stt_loaded, mic_ok).is_some()
}

pub fn tts_empty_guidance(tts_loaded: bool, text_empty: bool) -> Option<&'static str> {
    if text_empty {
        return Some("Paste or type text to speak.");
    }
    if !tts_loaded {
        return Some("Voice model loads on first Speak.");
    }
    None
}

pub fn tts_guidance_is_warning(_tts_loaded: bool, text_empty: bool) -> bool {
    // Empty text is quiet helper, not an error.
    let _ = text_empty;
    false
}

/// Path to the required neutral reference under a voices root (`{voice}_neutral.wav`).
pub fn neutral_ref_wav(voices_dir: &std::path::Path, voice: &str) -> std::path::PathBuf {
    let id = voice.trim();
    let id = if id.is_empty() { "default" } else { id };
    voices_dir.join(format!("{id}_neutral.wav"))
}

/// True when `{voice}_neutral.wav` exists (or legacy `eve_neutral.wav` for older installs).
pub fn neutral_voice_present(voices_dir: &std::path::Path, voice: &str) -> bool {
    if voices_dir.as_os_str().is_empty() {
        return false;
    }
    if neutral_ref_wav(voices_dir, voice).is_file() {
        return true;
    }
    voices_dir.join("eve_neutral.wav").is_file()
}

/// Speak primary enablement: non-empty text, not loading, neutral ref present.
pub fn can_speak_now(text_nonempty: bool, tts_loading: bool, neutral_present: bool) -> bool {
    text_nonempty && !tts_loading && neutral_present
}

/// Loud guidance when Eve neutral is missing (install path).
pub fn voice_missing_guidance(neutral_present: bool) -> Option<&'static str> {
    if neutral_present {
        None
    } else {
        Some(
            "Missing neutral voice reference — run scripts/install_voices.sh (set YAPPER_VOICES_DIR).",
        )
    }
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

/// Symmetric load badge text for advanced Settings (not primary chrome).
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

/// Compact Settings model-row status (no debug-ish "Active: loaded").
pub fn settings_model_status(
    loading: bool,
    loaded: bool,
    model_id: Option<&str>,
) -> String {
    if loading {
        return "loading…".into();
    }
    if !loaded {
        return "not loaded".into();
    }
    match model_id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => id.to_string(),
        None => "ready".into(),
    }
}

/// Estimate multiline TextEdit rows from available height (share of remaining panel).
pub fn text_panel_rows(available_height: f32, share: f32) -> usize {
    let budget = (available_height * share).max(TEXT_PANEL_MIN_ROWS as f32 * TEXT_ROW_HEIGHT_EST);
    let rows = (budget / TEXT_ROW_HEIGHT_EST).floor() as usize;
    rows.clamp(TEXT_PANEL_MIN_ROWS, TEXT_PANEL_MAX_ROWS)
}

/// After any synth request error that may have killed the worker, always refresh model status.
/// (WorkerManager::synthesize_timeout kills TTS on any request Err.)
#[cfg(test)]
pub fn synth_error_resets_worker() -> bool {
    true
}

#[cfg(test)]
#[path = "helpers_tests.rs"]
mod tests;
