//! egui tabs: Dictate / Speak workspaces and Settings cards.

use super::{HotkeyCaptureField, YapperApp, MIC_LABEL_MAX_CHARS};
use crate::segment::estimate_segment_count;
use crate::transport::TransportStatus;
use crate::ui::{
    card, form_row, helper_text, primary_button, settings_model_status, stt_empty_guidance,
    stt_guidance_is_warning, text_panel_rows, toolbar_row, transport_status_line, truncate_display,
    tts_empty_guidance, tts_guidance_is_warning, tts_text_stats,
};
use eframe::egui;

impl YapperApp {
    pub(crate) fn ui_hotkey_row(&mut self, ui: &mut egui::Ui, label: &str, field: HotkeyCaptureField) {
        form_row(ui, label, |ui| {
            let capturing = self.hotkey_capture == Some(field);
            if capturing {
                ui.colored_label(egui::Color32::LIGHT_BLUE, "Press combo… (Esc cancel)");
            } else {
                // Draft only — live cfg.hotkeys changes solely on successful Apply.
                let value = match field {
                    HotkeyCaptureField::ReadAloud => &mut self.hotkey_draft_read_aloud,
                    HotkeyCaptureField::PushToTalk => &mut self.hotkey_draft_push_to_talk,
                };
                ui.add(
                    egui::TextEdit::singleline(value)
                        .desired_width(200.0)
                        .hint_text("e.g. Super+Shift+S"),
                );
            }
            let cap_label = if capturing { "Listening…" } else { "Capture" };
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

    pub(crate) fn ui_tab_dictate(&mut self, ui: &mut egui::Ui) {
        let mic_ok = self.mic_list_error.is_none();
        if let Some(guide) = stt_empty_guidance(self.stt_loaded, mic_ok) {
            if stt_guidance_is_warning(self.stt_loaded, mic_ok) {
                ui.colored_label(egui::Color32::from_rgb(255, 200, 100), guide);
            } else {
                helper_text(ui, guide);
            }
            ui.add_space(4.0);
        }

        card(ui, "Options", |ui| {
            toolbar_row(ui, |ui| {
                self.ui_mic_controls_inline(ui);
                ui.separator();
                ui.label("Language");
                egui::ComboBox::from_id_salt("stt_lang")
                    .selected_text(&self.stt_language)
                    .show_ui(ui, |ui| {
                        for l in ["auto", "en", "fr"] {
                            ui.selectable_value(&mut self.stt_language, l.into(), l);
                        }
                    });
                ui.checkbox(&mut self.copy_transcript, "Copy transcript");
            });
        });

        if self.recording.is_some() {
            let card_title = super::state::recording_card_label(self.recording_intent);
            card(ui, card_title, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(255, 80, 80), card_title);
                    ui.add(
                        egui::ProgressBar::new(self.record_level)
                            .desired_width(ui.available_width().min(320.0))
                            .text(format!("level {:.0}%", self.record_level * 100.0)),
                    );
                });
            });
        }

        // Transcript card owns remaining vertical space.
        let rows = text_panel_rows((ui.available_height() - 8.0).max(0.0), 0.95);
        let frame = egui::Frame::none()
            .fill(egui::Color32::from_rgb(34, 38, 46))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(52, 58, 68)))
            .rounding(8.0)
            .inner_margin(egui::Margin::symmetric(12.0, 10.0));
        frame.show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(
                egui::RichText::new("Transcript")
                    .strong()
                    .size(14.0)
                    .color(egui::Color32::from_rgb(150, 195, 255)),
            );
            ui.add_space(6.0);
            let avail = ui.available_size();
            ui.add_sized(
                [avail.x, avail.y.max(rows as f32 * 18.0)],
                egui::TextEdit::multiline(&mut self.transcript)
                    .desired_width(f32::INFINITY)
                    .desired_rows(rows)
                    .hint_text("Transcript appears here…"),
            );
        });
    }

    pub(crate) fn ui_tab_speak(&mut self, ui: &mut egui::Ui) {
        let text_empty = self.tts_text.trim().is_empty();
        let voice_ok = crate::ui::neutral_voice_present(std::path::Path::new(
            self.cfg.models.voices_dir.trim(),
        ));
        if let Some(guide) = crate::ui::voice_missing_guidance(voice_ok) {
            ui.colored_label(egui::Color32::from_rgb(255, 180, 80), guide);
            ui.add_space(4.0);
        } else if let Some(guide) = tts_empty_guidance(self.tts_loaded, text_empty) {
            if tts_guidance_is_warning(self.tts_loaded, text_empty) {
                ui.colored_label(egui::Color32::from_rgb(255, 200, 100), guide);
            } else {
                helper_text(ui, guide);
            }
            ui.add_space(4.0);
        }

        card(ui, "Options", |ui| {
            toolbar_row(ui, |ui| {
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
                ui.checkbox(&mut self.read_clipboard, "Prefer clipboard")
                    .on_hover_text("When off, read-aloud uses the primary selection");
            });
        });

        // Transport card — honest sentence progress, no idle 0:00/0:00.
        let st = self.transport.status();
        let transport_idle = matches!(st, TransportStatus::Idle);
        let time = self.transport.machine().format_time_label();
        let line = transport_status_line(
            self.tts.active_job.is_some(),
            self.tts.playing_index,
            self.tts.total,
            transport_idle,
            &time,
            self.tts.synth_in_flight,
        );
        card(ui, "Transport", |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(&line)
                        .strong()
                        .color(egui::Color32::from_rgb(200, 215, 240)),
                );
                if !transport_idle {
                    ui.weak(format!("  ·  {}", st.as_str()));
                }
                if !self.transport.supports_transport_controls()
                    && matches!(st, TransportStatus::Playing | TransportStatus::Paused)
                {
                    ui.weak("  ·  mpv required for pause/seek");
                }
            });
            if !transport_idle || self.tts.active_job.is_some() {
                let mut progress = self.transport.machine().progress_01();
                let can_seek = self.transport.supports_transport_controls() && !transport_idle;
                ui.add_enabled_ui(can_seek, |ui| {
                    let scrub = ui.add(
                        egui::Slider::new(&mut progress, 0.0..=1.0)
                            .show_value(false)
                            .text("sentence progress"),
                    );
                    if scrub.changed() {
                        self.transport.seek_progress(progress);
                    }
                });
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
            }
        });

        let (stats, warn) = tts_text_stats(&self.tts_text);
        let segs = estimate_segment_count(&self.tts_text);
        let rows = text_panel_rows((ui.available_height() - 8.0).max(0.0), 0.9);
        let frame = egui::Frame::none()
            .fill(egui::Color32::from_rgb(34, 38, 46))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(52, 58, 68)))
            .rounding(8.0)
            .inner_margin(egui::Margin::symmetric(12.0, 10.0));
        frame.show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Text")
                        .strong()
                        .size(14.0)
                        .color(egui::Color32::from_rgb(150, 195, 255)),
                );
                ui.weak(format!("{stats} · ~{segs} sentence(s)"));
            });
            if let Some(w) = &warn {
                ui.colored_label(egui::Color32::from_rgb(255, 190, 90), w);
            }
            ui.add_space(6.0);
            let avail = ui.available_size();
            ui.add_sized(
                [avail.x, avail.y.max(rows as f32 * 18.0)],
                egui::TextEdit::multiline(&mut self.tts_text)
                    .desired_width(f32::INFINITY)
                    .desired_rows(rows)
                    .hint_text("Paste text here…"),
            );
        });
    }

    pub(crate) fn ui_tab_settings(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        card(ui, "Models", |ui| {
            helper_text(ui, "Models load on first use; unload frees VRAM.");
            ui.add_space(6.0);
            form_row(ui, "Dictation model", |ui| {
                egui::ComboBox::from_id_salt("stt_model")
                    .selected_text(&self.stt_model)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.stt_model, "small".into(), "small");
                        ui.selectable_value(&mut self.stt_model, "medium".into(), "medium");
                    });
                ui.weak(settings_model_status(
                    self.stt_loading,
                    self.stt_loaded,
                    self.stt_model_id.as_deref(),
                ));
            });
            ui.add_space(4.0);
            form_row(ui, "Voice model", |ui| {
                ui.weak("chatterbox-multilingual");
                ui.weak(settings_model_status(
                    self.tts_loading,
                    self.tts_loaded,
                    self.tts_model_id.as_deref(),
                ));
            });
            ui.add_space(6.0);
            ui.collapsing("Advanced: load / unload", |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.stt_loading, egui::Button::new("Load dictation"))
                        .clicked()
                    {
                        self.load_stt();
                    }
                    if ui.button("Unload dictation").clicked() {
                        self.unload_stt();
                    }
                    ui.weak(self.stt_status_label());
                });
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.tts_loading, egui::Button::new("Load voice"))
                        .clicked()
                    {
                        self.load_tts();
                    }
                    if ui.button("Unload voice").clicked() {
                        self.unload_tts();
                    }
                    if ui.button("Unload all").clicked() {
                        // Shared stop/OOB-kill/cleanup then UnloadAll (not bare enqueue).
                        self.unload_all_models();
                    }
                    ui.weak(self.tts_status_label());
                });
            });
        });

        card(ui, "Hotkeys", |ui| {
            ui.label(format!(
                "Live: {}  ·  {}",
                self.cfg.hotkeys.read_aloud, self.cfg.hotkeys.push_to_talk
            ));
            ui.add_space(4.0);
            self.ui_hotkey_row(ui, "Read selection aloud", HotkeyCaptureField::ReadAloud);
            self.ui_hotkey_row(ui, "Hold to dictate", HotkeyCaptureField::PushToTalk);
            helper_text(
                ui,
                "Edit or Capture a draft, then Apply. Save settings keeps live hotkeys only.",
            );
            ui.add_space(4.0);
            if primary_button(ui, "Apply hotkeys").clicked() {
                self.apply_hotkeys();
            }
        });

        card(ui, "Audio", |ui| {
            self.ui_mic_controls(ui);
        });
    }

    pub(crate) fn ui_mic_controls(&mut self, ui: &mut egui::Ui) {
        toolbar_row(ui, |ui| {
            self.ui_mic_controls_inline(ui);
        });
        if let Some(err) = &self.mic_list_error {
            ui.colored_label(egui::Color32::YELLOW, format!("mic list: {err}"));
        }
    }

    fn ui_mic_controls_inline(&mut self, ui: &mut egui::Ui) {
        ui.label("Mic");
        let selected_full = if self.mic_source.is_empty() {
            "System default".to_string()
        } else {
            self.mic_sources
                .iter()
                .find(|s| s.name == self.mic_source)
                .map(|s| s.label())
                .unwrap_or_else(|| self.mic_source.clone())
        };
        let selected_display = truncate_display(&selected_full, MIC_LABEL_MAX_CHARS);
        // Clone names/labels once for combo to avoid borrow issues without cloning every frame's Vec.
        let sources: Vec<(String, String)> = self
            .mic_sources
            .iter()
            .map(|s| (s.name.clone(), s.label()))
            .collect();
        let combo = egui::ComboBox::from_id_salt("mic_source")
            .selected_text(selected_display)
            .width(280.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.mic_source, String::new(), "System default");
                for (name, full) in &sources {
                    let shown = truncate_display(full, MIC_LABEL_MAX_CHARS);
                    let response =
                        ui.selectable_value(&mut self.mic_source, name.clone(), shown);
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
    }
}
