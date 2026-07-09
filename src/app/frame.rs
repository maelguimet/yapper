//! eframe::App: top/bottom chrome and frame loop.

use super::{MainTab, YapperApp};
use crate::audio::stop_recording;
use crate::lifecycle::{ExitPromptState, ShellIntent};
use crate::transport::TransportStatus;
use crate::tray::pump_gtk_events;
use crate::ui::{
    apply_yapper_theme, can_replay_tts, can_stop_tts, danger_button, primary_tab_labels,
    speak_action_label, status_chip, truncate_display, ChipState,
};
use crate::x11util;
use eframe::egui;

impl eframe::App for YapperApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.theme_applied {
            apply_yapper_theme(ctx);
            self.theme_applied = true;
        }

        pump_gtk_events();
        self.handle_viewport_lifecycle(ctx);
        self.poll_hotkey_capture(ctx);
        self.poll_hotkeys();
        self.poll_tray(ctx);
        self.poll_record_level();
        self.drain_job_messages();
        self.poll_transport();
        self.autosave_prefs_if_dirty();

        let recording = self.recording.is_some();
        let playing = matches!(
            self.transport.status(),
            TransportStatus::Playing | TransportStatus::Buffering
        );
        let busy = self.stt_loading
            || self.tts_loading
            || self.tts.synth_in_flight
            || self.tts.has_work();
        let repaint_ms = if self.hotkey_capture.is_some()
            || self.tray.is_none()
            || recording
            || playing
            || busy
        {
            16
        } else {
            100
        };
        ctx.request_repaint_after(std::time::Duration::from_millis(repaint_ms));

        // ── Top chrome: brand, status sentence, chips, hide ───────────────
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading(
                    egui::RichText::new("Yapper")
                        .color(egui::Color32::from_rgb(120, 180, 255))
                        .size(20.0),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(&self.status)
                        .color(egui::Color32::from_rgb(200, 210, 220))
                        .size(13.5),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("Hide")
                        .on_hover_text("Hide window; tray Open restores")
                        .clicked()
                    {
                        self.hide_to_tray(ctx);
                    }
                });
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let dict_state = if self.stt_loading {
                    ChipState::Loading
                } else if self.stt_loaded {
                    ChipState::Good
                } else {
                    ChipState::Off
                };
                let voice_state = if self.tts_loading {
                    ChipState::Loading
                } else if self.tts_loaded {
                    ChipState::Good
                } else if self.tts_busy() {
                    ChipState::Active
                } else {
                    ChipState::Off
                };
                status_chip(ui, &self.dictation_chip(), dict_state);
                status_chip(ui, &self.voice_chip(), voice_state);
                let mic = truncate_display(&self.active_mic_label(), 28);
                status_chip(ui, &format!("Mic: {mic}"), ChipState::Off);
                if recording {
                    let t = ctx.input(|i| i.time);
                    let pulse = 0.55 + 0.45 * ((t * 6.0).sin() * 0.5 + 0.5);
                    let r = (255.0 * pulse) as u8;
                    ui.colored_label(
                        egui::Color32::from_rgb(r, 40, 40),
                        "  * RECORDING  ",
                    );
                }
            });

            if let Some(err) = &self.hotkey_error {
                status_chip(ui, err, ChipState::Error);
            }
            if let Some(err) = &self.tray_error {
                status_chip(ui, err, ChipState::Error);
            }
            ui.add_space(2.0);
        });

        // ── Bottom: sticky primary actions for active tab ────────────────
        egui::TopBottomPanel::bottom("bottom_actions").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                match self.main_tab {
                    MainTab::Dictate => {
                        if recording {
                            if danger_button(ui, "Stop and transcribe").clicked() {
                                self.ptt_release();
                            }
                        } else {
                            let rec = ui.add_enabled(
                                !self.stt_loading,
                                egui::Button::new(
                                    egui::RichText::new("Record")
                                        .color(egui::Color32::from_rgb(240, 248, 255))
                                        .strong(),
                                )
                                .fill(egui::Color32::from_rgb(50, 110, 200))
                                .min_size(egui::vec2(100.0, 28.0)),
                            );
                            if rec.clicked() {
                                self.ptt_press();
                            }
                            if self.stt_loading {
                                ui.weak("model loading…");
                            }
                        }
                        if ui
                            .add_enabled(!self.stt_loading, egui::Button::new("Transcribe file…"))
                            .clicked()
                        {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("audio", &["wav", "mp3", "m4a", "flac", "ogg"])
                                .pick_file()
                            {
                                self.insert_after_transcribe = false;
                                self.do_transcribe_file(path);
                            }
                        }
                        if ui
                            .add_enabled(
                                !self.transcript.is_empty(),
                                egui::Button::new("Copy"),
                            )
                            .clicked()
                        {
                            let _ = x11util::write_clipboard(&self.transcript);
                        }
                        if ui.button("Clear").clicked() {
                            self.transcript.clear();
                        }
                    }
                    MainTab::Speak => {
                        let tts_busy = self.tts_busy();
                        let can_speak = !self.tts_text.trim().is_empty() && !self.tts_loading;
                        let speak_lbl = speak_action_label(tts_busy);
                        let speak = ui.add_enabled(
                            can_speak,
                            egui::Button::new(
                                egui::RichText::new(speak_lbl)
                                    .color(egui::Color32::from_rgb(240, 248, 255))
                                    .strong(),
                            )
                            .fill(egui::Color32::from_rgb(50, 110, 200))
                            .min_size(egui::vec2(100.0, 28.0)),
                        );
                        if speak.clicked() {
                            let t = self.tts_text.clone();
                            self.do_speak(&t);
                        }
                        let pause_label = match self.transport.status() {
                            TransportStatus::Paused => "Resume",
                            _ => "Pause",
                        };
                        let can_pause = self.transport.supports_transport_controls()
                            && matches!(
                                self.transport.status(),
                                TransportStatus::Playing | TransportStatus::Paused
                            );
                        let pause_btn = ui
                            .add_enabled(can_pause, egui::Button::new(pause_label))
                            .on_hover_text(if self.transport.supports_transport_controls() {
                                "Pause / resume"
                            } else {
                                "mpv required for pause/seek"
                            });
                        if pause_btn.clicked() {
                            self.transport.toggle_pause();
                        }
                        let stop_enabled = can_stop_tts(tts_busy);
                        // Danger fill only when enabled — egui does not mute custom fills.
                        let stop_btn = if stop_enabled {
                            ui.add(danger_button_widget("Stop"))
                        } else {
                            ui.add_enabled(
                                false,
                                egui::Button::new("Stop").min_size(egui::vec2(96.0, 28.0)),
                            )
                        };
                        if stop_btn.clicked() {
                            self.cancel_tts_pipeline();
                            self.status = "playback stopped".into();
                        }
                        let replay_exists = self
                            .tts_last_full_path
                            .as_ref()
                            .is_some_and(|p| p.is_file())
                            || self
                                .transport
                                .machine()
                                .last_path
                                .as_ref()
                                .is_some_and(|p| p.is_file());
                        let can_replay = can_replay_tts(tts_busy, replay_exists);
                        if ui
                            .add_enabled(can_replay, egui::Button::new("Replay"))
                            .on_hover_text("Replay full last utterance")
                            .clicked()
                        {
                            match self.replay_last() {
                                Ok(true) => self.status = "replaying…".into(),
                                Ok(false) => self.status = "nothing to replay".into(),
                                Err(e) => self.status = format!("replay error: {e:#}"),
                            }
                        }
                        if ui
                            .add_enabled(!self.tts_loading, egui::Button::new("Read selection"))
                            .clicked()
                        {
                            self.read_aloud();
                        }
                        if ui
                            .add_enabled(!self.tts_loading, egui::Button::new("Speak file…"))
                            .clicked()
                        {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("text", &["txt", "md"])
                                .pick_file()
                            {
                                if let Ok(t) = std::fs::read_to_string(path) {
                                    self.tts_text = t;
                                    let t = self.tts_text.clone();
                                    self.do_speak(&t);
                                }
                            }
                        }
                    }
                    MainTab::Settings => {
                        if ui.button("Save settings").clicked() {
                            self.persist();
                            self.status = "settings saved".into();
                        }
                        if ui.button("Hide").clicked() {
                            self.hide_to_tray(ctx);
                        }
                        if ui.button("Exit…").clicked() {
                            self.exit_prompt = self.exit_prompt.on_exit_clicked();
                        }
                    }
                }
            });
            ui.add_space(4.0);
        });

        if self.exit_prompt == ExitPromptState::AwaitingConfirm {
            egui::Window::new("Exit Yapper completely?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(
                        "This unloads models and stops the tray process.\n\
                         Prefer Hide if you only want the window gone.",
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.exit_prompt = self.exit_prompt.on_cancel();
                        }
                        if ui
                            .button("Exit")
                            .on_hover_text("Same as tray Quit")
                            .clicked()
                        {
                            let (next, intent) = self.exit_prompt.on_confirm();
                            self.exit_prompt = next;
                            if intent == Some(ShellIntent::HardQuit) {
                                self.arm_hard_quit_and_close(ctx);
                            }
                        }
                    });
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let labels = primary_tab_labels();
                for (tab, label) in [
                    (MainTab::Dictate, labels[0]),
                    (MainTab::Speak, labels[1]),
                    (MainTab::Settings, labels[2]),
                ] {
                    let selected = self.main_tab == tab;
                    let text = if selected {
                        egui::RichText::new(format!("  {label}  ")).strong()
                    } else {
                        egui::RichText::new(format!("  {label}  "))
                    };
                    let btn = egui::Button::new(text).fill(if selected {
                        egui::Color32::from_rgb(50, 90, 150)
                    } else {
                        egui::Color32::from_rgb(40, 44, 52)
                    });
                    if ui.add(btn).clicked() {
                        self.main_tab = tab;
                    }
                }
            });
            ui.add_space(8.0);

            match self.main_tab {
                MainTab::Dictate => self.ui_tab_dictate(ui),
                MainTab::Speak => self.ui_tab_speak(ui),
                MainTab::Settings => {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            self.ui_tab_settings(ui, ctx);
                        });
                }
            }
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.discard_all_tts_audio();
        let _ = self.jobs.kill_tts_now();
        self.jobs.send(super::messages::JobCmd::UnloadAll);
        self.jobs.send(super::messages::JobCmd::Shutdown);
        if let Some(session) = self.recording.take() {
            let _ = stop_recording(session);
        }
        self.hotkeys = None;
    }
}

/// Danger-styled button that returns a Widget so it can be nested in add_enabled.
fn danger_button_widget(label: &str) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(label.to_owned()).strong())
        .fill(egui::Color32::from_rgb(160, 50, 50))
        .min_size(egui::vec2(96.0, 28.0))
}

#[cfg(test)]
mod tests {
    use crate::ui::primary_tab_labels;

    #[test]
    fn frame_tabs_match_primary_labels() {
        let tabs = primary_tab_labels();
        assert_eq!(tabs[0], "Dictate");
        assert_eq!(tabs[1], "Speak");
        assert_eq!(tabs[2], "Settings");
    }
}
