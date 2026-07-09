//! Main egui window + tray orchestration.

mod jobs;
pub(crate) mod messages;
mod pipeline;
mod shell;
mod state;
mod frame;
mod tabs;
mod tts_controller;

pub use state::YapperApp;

use anyhow::Result;
use eframe::egui;

/// Minimum window size so controls are not born clipped (Phase 10 / B6).
pub(crate) const MIN_WINDOW_WIDTH: f32 = 720.0;
pub(crate) const MIN_WINDOW_HEIGHT: f32 = 560.0;
pub(crate) const DEFAULT_WINDOW_WIDTH: f32 = 880.0;
pub(crate) const DEFAULT_WINDOW_HEIGHT: f32 = 720.0;
/// Max chars for mic labels in combo chrome (full name on hover).
pub(crate) const MIC_LABEL_MAX_CHARS: usize = 42;

/// Which hotkey field is listening for a key-capture press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HotkeyCaptureField {
    ReadAloud,
    PushToTalk,
}

/// Primary work tabs: STT and TTS are peer workspaces; Settings holds models/hotkeys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MainTab {
    Stt,
    Tts,
    Settings,
}

/// Map an egui key to the hotkey config token (`S`, `1`, `Space`, …).
pub(crate) fn egui_key_to_token(key: egui::Key) -> Option<&'static str> {
    use egui::Key;
    Some(match key {
        Key::A => "A",
        Key::B => "B",
        Key::C => "C",
        Key::D => "D",
        Key::E => "E",
        Key::F => "F",
        Key::G => "G",
        Key::H => "H",
        Key::I => "I",
        Key::J => "J",
        Key::K => "K",
        Key::L => "L",
        Key::M => "M",
        Key::N => "N",
        Key::O => "O",
        Key::P => "P",
        Key::Q => "Q",
        Key::R => "R",
        Key::S => "S",
        Key::T => "T",
        Key::U => "U",
        Key::V => "V",
        Key::W => "W",
        Key::X => "X",
        Key::Y => "Y",
        Key::Z => "Z",
        Key::Num0 => "0",
        Key::Num1 => "1",
        Key::Num2 => "2",
        Key::Num3 => "3",
        Key::Num4 => "4",
        Key::Num5 => "5",
        Key::Num6 => "6",
        Key::Num7 => "7",
        Key::Num8 => "8",
        Key::Num9 => "9",
        Key::Space => "Space",
        Key::F1 => "F1",
        Key::F2 => "F2",
        Key::F3 => "F3",
        Key::F4 => "F4",
        Key::F5 => "F5",
        Key::F6 => "F6",
        Key::F7 => "F7",
        Key::F8 => "F8",
        Key::F9 => "F9",
        Key::F10 => "F10",
        Key::F11 => "F11",
        Key::F12 => "F12",
        Key::Escape
        | Key::Tab
        | Key::Enter
        | Key::Backspace
        | Key::Delete
        | Key::ArrowUp
        | Key::ArrowDown
        | Key::ArrowLeft
        | Key::ArrowRight
        | Key::Home
        | Key::End
        | Key::PageUp
        | Key::PageDown
        | Key::Insert
        | Key::Copy
        | Key::Cut
        | Key::Paste => return None,
        _ => return None,
    })
}

pub(crate) enum CaptureOutcome {
    Bound(String),
    Cancel,
    Error(String),
}

pub fn run_gui() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT])
            .with_min_inner_size([MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT])
            .with_title("Yapper"),
        ..Default::default()
    };
    eframe::run_native(
        "Yapper",
        options,
        Box::new(|cc| Ok(Box::new(YapperApp::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::ui::apply_yapper_theme;
    use eframe::egui;

    #[test]
    fn theme_visuals_are_dark_not_default_grey() {
        let ctx = egui::Context::default();
        apply_yapper_theme(&ctx);
        let v = &ctx.style().visuals;
        assert!(v.dark_mode);
        assert_eq!(v.panel_fill, egui::Color32::from_rgb(28, 31, 36));
    }
}
