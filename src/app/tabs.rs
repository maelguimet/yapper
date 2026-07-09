//! egui tabs and controls.

use super::{HotkeyCaptureField, YapperApp, MIC_LABEL_MAX_CHARS};
use crate::segment::estimate_segment_count;
use crate::ui::{
    primary_button, section_heading, stt_empty_guidance, text_panel_rows, truncate_display,
    tts_empty_guidance, tts_text_stats,
};
use eframe::egui;


impl YapperApp {
    pub(crate) fn ui_hotkey_row(&mut self, ui: &mut egui::Ui, label: &str, field: HotkeyCaptureField) {
        ui.horizontal(|ui| {
            ui.label(label);
            let capturing = self.hotkey_capture == Some(field);
            if capturing {
                ui.colored_label(egui::Color32::LIGHT_BLUE, "Press combo... (Esc cancel)");
            } else {
                let value = match field {
                    HotkeyCaptureField::ReadAloud => &mut self.cfg.hotkeys.read_aloud,
                    HotkeyCaptureField::PushToTalk => &mut self.cfg.hotkeys.push_to_talk,
                };
                ui.add(
                    egui::TextEdit::singleline(value)
                        .desired_width(200.0)
                        .hint_text("e.g. Super+Shift+S"),
                );
            }
            let cap_label = if capturing { "Listening..." } else { "Capture" };
            if ui.button(cap_label).clicked() {
                if capturing {
                    self.hotkey_capture = None;
                    self.status = "hotkey capture cancelled".into();
                } else {
                    self.hotkey_capture = Some(field);
                    self.status = format!("capturing {label}…");
                }
            }
        });
    }

    pub(crate) fn ui_tab_stt(&mut self, ui: &mut egui::Ui) {
        section_heading(ui, "Dictation");

        let mic_ok = self.mic_list_error.is_none();
        if let Some(guide) = stt_empty_guidance(self.workers.stt_loaded(), mic_ok) {
            ui.colored_label(egui::Color32::from_rgb(255, 200, 100), guide);
            ui.add_space(4.0);
        }

        self.ui_mic_controls(ui);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Language");
            egui::ComboBox::from_id_salt("stt_lang")
                .selected_text(&self.stt_language)
                .show_ui(ui, |ui| {
                    for l in ["auto", "en", "fr"] {
                        ui.selectable_value(&mut self.stt_language, l.into(), l);
                    }
                });
            ui.checkbox(&mut self.copy_transcript, "Copy transcript on dictate");
        });

        if self.recording.is_some() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::from_rgb(255, 80, 80), "Recording in progress");
                ui.add(
                    egui::ProgressBar::new(self.record_level)
                        .desired_width(ui.available_width().min(280.0))
                        .text(format!("level {:.0}%", self.record_level * 100.0)),
                );
            });
        }

        ui.add_space(6.0);
        section_heading(ui, "Transcript");
        if self.transcript.is_empty() && self.recording.is_none() {
            ui.weak("Empty - Record or use hold-to-talk.");
        }
        let stt_rows = text_panel_rows(ui.available_height(), 0.55);
        ui.add(
            egui::TextEdit::multiline(&mut self.transcript)
                .desired_width(f32::INFINITY)
                .desired_rows(stt_rows)
                .hint_text("Transcript appears here..."),
        );
    }

    pub(crate) fn ui_tab_tts(&mut self, ui: &mut egui::Ui) {
        section_heading(ui, "Speak");

        let text_empty = self.tts_text.trim().is_empty();
        if let Some(guide) = tts_empty_guidance(self.workers.tts_loaded(), text_empty) {
            ui.colored_label(egui::Color32::from_rgb(255, 200, 100), guide);
            ui.add_space(4.0);
        }

        ui.horizontal(|ui| {
            ui.label("Language");
            egui::ComboBox::from_id_salt("tts_lang")
                .selected_text(&self.tts_language)
                .show_ui(ui, |ui| {
                    for l in ["en", "fr"] {
                        ui.selectable_value(&mut self.tts_language, l.into(), l);
                    }
                });
            ui.label("Tone");
            egui::ComboBox::from_id_salt("tts_tone")
                .selected_text(&self.tts_tone)
                .show_ui(ui, |ui| {
                    for t in &self.tones.clone() {
                        ui.selectable_value(&mut self.tts_tone, t.clone(), t);
                    }
                });
        });
        ui.checkbox(
            &mut self.read_clipboard,
            "Read clipboard (else selection)",
        );

        // Transport strip
        ui.add_space(6.0);
        section_heading(ui, "Transport");
        let st = self.transport.status();
        ui.horizontal(|ui| {
            ui.label(format!("Status: {}", st.as_str()));
            ui.separator();
            ui.label(self.transport.machine().format_time_label());
            if !self.tts_queue.is_empty() {
                let left = self.tts_queue.len();
                ui.separator();
                ui.colored_label(
                    egui::Color32::from_rgb(180, 200, 255),
                    format!("{left} segment(s) queued"),
                );
            }
        });
        let mut progress = self.transport.machine().progress_01();
        let scrub = ui.add(
            egui::Slider::new(&mut progress, 0.0..=1.0)
                .show_value(false)
                .text("seek"),
        );
        if scrub.changed() {
            self.transport.seek_progress(progress);
        }
        ui.horizontal(|ui| {
            ui.label("Volume");
            let mut vol = self.transport.volume();
            if ui
                .add(egui::Slider::new(&mut vol, 0.0..=1.0).show_value(false))
                .changed()
            {
                self.transport.set_volume(vol);
            }
        });

        ui.add_space(6.0);
        section_heading(ui, "Text");
        let (stats, warn) = tts_text_stats(&self.tts_text);
        let segs = estimate_segment_count(&self.tts_text);
        ui.horizontal(|ui| {
            ui.weak(format!("{stats} · ~{segs} segment(s)"));
            if let Some(w) = &warn {
                ui.colored_label(egui::Color32::from_rgb(255, 190, 90), w);
            }
        });
        let tts_rows = text_panel_rows(ui.available_height(), 0.45);
        ui.add(
            egui::TextEdit::multiline(&mut self.tts_text)
                .desired_width(f32::INFINITY)
                .desired_rows(tts_rows)
                .hint_text("Type or paste text to speak..."),
        );
    }

    pub(crate) fn ui_tab_settings(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        section_heading(ui, "Models");
        ui.horizontal(|ui| {
            ui.label("STT model");
            egui::ComboBox::from_id_salt("stt_model")
                .selected_text(&self.stt_model)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.stt_model, "small".into(), "small");
                    ui.selectable_value(&mut self.stt_model, "medium".into(), "medium");
                });
            if ui.button("Load STT").clicked() {
                self.load_stt();
            }
            if ui.button("Unload STT").clicked() {
                self.unload_stt();
            }
            ui.label(self.stt_status_label());
        });
        ui.horizontal(|ui| {
            ui.label("TTS");
            ui.weak("chatterbox-multilingual");
            if ui.button("Load TTS").clicked() {
                self.load_tts();
            }
            if ui.button("Unload TTS").clicked() {
                self.unload_tts();
            }
            if ui.button("Unload all").clicked() {
                let _ = self.workers.unload_all();
                self.status = "all unloaded".into();
            }
            ui.label(self.tts_status_label());
        });

        ui.add_space(10.0);
        section_heading(ui, "Hotkeys");
        ui.label(format!(
            "{}  |  {}",
            self.cfg.hotkeys.read_aloud, self.cfg.hotkeys.push_to_talk
        ));
        self.ui_hotkey_row(ui, "Read aloud", HotkeyCaptureField::ReadAloud);
        self.ui_hotkey_row(ui, "Hold-to-talk", HotkeyCaptureField::PushToTalk);
        ui.weak(
            "Press Capture, then Apply.",
        );
        if primary_button(ui, "Apply hotkeys").clicked() {
            self.apply_hotkeys();
        }

        ui.add_space(10.0);
        section_heading(ui, "Devices");
        self.ui_mic_controls(ui);
    }

    pub(crate) fn ui_mic_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Microphone");
            let selected_full = if self.mic_source.is_empty() {
                "System default".to_string()
            } else {
                // Prefer list label when present
                self.mic_sources
                    .iter()
                    .find(|s| s.name == self.mic_source)
                    .map(|s| s.label())
                    .unwrap_or_else(|| self.mic_source.clone())
            };
            let selected_display = truncate_display(&selected_full, MIC_LABEL_MAX_CHARS);
            let combo = egui::ComboBox::from_id_salt("mic_source")
                .selected_text(selected_display)
                .width(360.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.mic_source, String::new(), "System default");
                    for src in &self.mic_sources.clone() {
                        let full = src.label();
                        let shown = truncate_display(&full, MIC_LABEL_MAX_CHARS);
                        let response =
                            ui.selectable_value(&mut self.mic_source, src.name.clone(), shown);
                        if full.chars().count() > MIC_LABEL_MAX_CHARS {
                            response.on_hover_text(full);
                        }
                    }
                });
            combo.response.on_hover_text(&selected_full);
            if ui.button("Refresh").clicked() {
                self.refresh_mic_sources();
                self.status = format!("mic list: {} source(s)", self.mic_sources.len());
            }
        });
        if let Some(err) = &self.mic_list_error {
            ui.colored_label(egui::Color32::YELLOW, format!("mic list: {err}"));
        }
        if self.recording.is_some() {
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::RED, "* REC");
                let device_full = self.active_mic_label();
                let device_shown = truncate_display(&device_full, MIC_LABEL_MAX_CHARS);
                ui.label(format!("device: {device_shown}"))
                    .on_hover_text(device_full);
                ui.add(
                    egui::ProgressBar::new(self.record_level)
                        .desired_width(200.0)
                        .text(format!("level {:.0}%", self.record_level * 100.0)),
                );
            });
        }
    }
}

