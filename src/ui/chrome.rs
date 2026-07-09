//! Shared chrome widgets: chips, cards, toolbars, primary/danger buttons.

use eframe::egui;

/// Visual state for compact status chips in the top strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipState {
    Off,
    Good,
    Loading,
    Active,
    Error,
}

impl ChipState {
    fn fill(self) -> egui::Color32 {
        match self {
            ChipState::Off => egui::Color32::from_rgb(48, 52, 60),
            ChipState::Good => egui::Color32::from_rgb(36, 72, 52),
            ChipState::Loading => egui::Color32::from_rgb(72, 64, 36),
            ChipState::Active => egui::Color32::from_rgb(40, 70, 110),
            ChipState::Error => egui::Color32::from_rgb(96, 40, 40),
        }
    }

    fn text(self) -> egui::Color32 {
        match self {
            ChipState::Off => egui::Color32::from_rgb(160, 166, 176),
            ChipState::Good => egui::Color32::from_rgb(140, 230, 170),
            ChipState::Loading => egui::Color32::from_rgb(240, 210, 120),
            ChipState::Active => egui::Color32::from_rgb(170, 210, 255),
            ChipState::Error => egui::Color32::from_rgb(255, 160, 150),
        }
    }
}

/// Compact status chip for the top chrome strip.
pub fn status_chip(ui: &mut egui::Ui, label: &str, state: ChipState) {
    let frame = egui::Frame::none()
        .fill(state.fill())
        .rounding(6.0)
        .inner_margin(egui::Margin::symmetric(8.0, 3.0));
    frame.show(ui, |ui| {
        ui.label(egui::RichText::new(label).size(12.5).color(state.text()));
    });
}

/// Grouped card with a title and body content.
pub fn card(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    let frame = egui::Frame::none()
        .fill(egui::Color32::from_rgb(34, 38, 46))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(52, 58, 68),
        ))
        .rounding(8.0)
        .inner_margin(egui::Margin::symmetric(12.0, 10.0));
    frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        if !title.is_empty() {
            ui.label(
                egui::RichText::new(title)
                    .strong()
                    .size(14.0)
                    .color(egui::Color32::from_rgb(150, 195, 255)),
            );
            ui.add_space(6.0);
        }
        add_contents(ui);
    });
    ui.add_space(8.0);
}

/// Horizontal toolbar row with consistent spacing.
pub fn toolbar_row(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        add_contents(ui);
    });
}

/// Label + control form row.
pub fn form_row(ui: &mut egui::Ui, label: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_width(ui.available_width());
        ui.label(
            egui::RichText::new(label)
                .color(egui::Color32::from_rgb(180, 190, 205))
                .size(13.0),
        );
        ui.add_space(8.0);
        add_contents(ui);
    });
}

pub fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).strong())
            .fill(egui::Color32::from_rgb(50, 110, 200))
            .min_size(egui::vec2(110.0, 28.0)),
    )
}

pub fn danger_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).strong())
            .fill(egui::Color32::from_rgb(160, 50, 50))
            .min_size(egui::vec2(96.0, 28.0)),
    )
}

/// Quiet helper text (not a warning).
pub fn helper_text(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(12.5)
            .color(egui::Color32::from_rgb(140, 148, 160)),
    );
}
