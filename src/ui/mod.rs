//! Pure UI helpers, theme, and chrome widgets (no app state).

mod chrome;
mod helpers;
mod theme;

pub use chrome::{danger_button, primary_button, section_heading};
pub use helpers::{
    chunk_paths_retained_for_replay, chunk_paths_to_remove, load_status_label, resolve_replay_path,
    stt_empty_guidance, text_panel_rows, truncate_display, tts_empty_guidance, tts_text_stats,
};
pub use theme::apply_yapper_theme;
