//! Shared chrome widgets (section headers, primary/danger buttons).

use eframe::egui;

pub fn section_heading(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(title)
            .strong()
            .size(16.0)
            .color(egui::Color32::from_rgb(140, 190, 255)),
    );
    ui.add_space(2.0);
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
