//! High-contrast dark theme for the main window.

use eframe::egui;

/// Apply a high-contrast dark theme (not default grey soup).
pub fn apply_yapper_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let mut visuals = egui::Visuals::dark();
    visuals.window_fill = egui::Color32::from_rgb(22, 24, 28);
    visuals.panel_fill = egui::Color32::from_rgb(28, 31, 36);
    // Slightly above pure black so multiline TextEdit does not feel abyss-like.
    visuals.extreme_bg_color = egui::Color32::from_rgb(26, 30, 36);
    visuals.faint_bg_color = egui::Color32::from_rgb(36, 40, 48);
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(36, 40, 48);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(48, 54, 64);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(64, 72, 88);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(70, 110, 180);
    visuals.selection.bg_fill = egui::Color32::from_rgb(50, 100, 180);
    visuals.override_text_color = Some(egui::Color32::from_rgb(230, 234, 240));
    visuals.widgets.noninteractive.fg_stroke.color = egui::Color32::from_rgb(200, 206, 216);
    visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(220, 226, 236);
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.visuals = visuals;
    ctx.set_style(style);
}
