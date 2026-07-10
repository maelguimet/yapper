//! Pure UI helpers, theme, and chrome widgets (no app state).

mod chrome;
mod helpers;
mod theme;

pub use chrome::{
    card, danger_button, form_row, helper_text, primary_button, status_chip, toolbar_row, ChipState,
};
pub use helpers::{
    can_replay_tts, can_speak_now, can_stop_tts, chunk_paths_retained_for_replay,
    chunk_paths_to_remove, dictation_chip_label, fallback_tones, load_status_label,
    neutral_ref_wav, neutral_voice_present, primary_tab_labels, resolve_replay_path,
    settings_model_status, speak_action_label, speak_restart_needs_oob_kill, stt_empty_guidance,
    stt_guidance_is_warning, stt_ready_for_selected, text_panel_rows, transport_status_line,
    truncate_display, tts_empty_guidance, tts_guidance_is_warning, tts_text_stats, voice_chip_label,
    voice_missing_guidance,
};
#[cfg(test)]
pub use helpers::synth_error_resets_worker;
pub use theme::apply_yapper_theme;
