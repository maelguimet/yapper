//! eframe::App: top/bottom chrome and frame loop.

use super::{MainTab, YapperApp};
use crate::audio::stop_recording;
use crate::lifecycle::{ExitPromptState, ShellIntent};
use crate::transport::TransportStatus;
use crate::tray::pump_gtk_events;
use crate::ui::{apply_yapper_theme, danger_button, primary_button, truncate_display};
use crate::x11util;
use eframe::egui;

impl eframe::App for YapperApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.theme_applied {
            apply_yapper_theme(ctx);
            self.theme_applied = true;
        }

        // tray-icon needs GTK iterations (B20: icon missing without this).
        pump_gtk_events();
        self.handle_viewport_lifecycle(ctx);
        self.poll_hotkey_capture(ctx);
        self.poll_hotkeys();
        self.poll_tray(ctx);
        self.poll_record_level();
        self.poll_transport();

        let recording = self.recording.is_some();
        let playing = matches!(
            self.transport.status(),
            TransportStatus::Playing | TransportStatus::Buffering
        );
        let repaint_ms =
            if self.hotkey_capture.is_some() || self.tray.is_none() || recording || playing {
                16
            } else {
                100
            };
        ctx.request_repaint_after(std::time::Duration::from_millis(repaint_ms));

        // ── Top chrome: brand, status, hide ──────────────────────────────
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading(
                    egui::RichText::new("Yapper")
                        .color(egui::Color32::from_rgb(120, 180, 255))
                        .size(22.0),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(&self.status)
                        .color(egui::Color32::from_rgb(200, 210, 220)),
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

            // Compact strip: models + mic + live rec pulse
            ui.horizontal(|ui| {
                let stt_col = if self.workers.stt_loaded() {
                    egui::Color32::from_rgb(100, 220, 140)
                } else {
                    egui::Color32::from_rgb(140, 140, 150)
                };
                let tts_col = if self.workers.tts_loaded() {
                    egui::Color32::from_rgb(100, 220, 140)
                } else {
                    egui::Color32::from_rgb(140, 140, 150)
                };
                ui.colored_label(stt_col, self.stt_status_label());
                ui.separator();
                ui.colored_label(tts_col, self.tts_status_label());
                ui.separator();
                let mic = truncate_display(&self.active_mic_label(), 28);
                ui.label(format!("Mic: {mic}"))
                    .on_hover_text(self.active_mic_label());
                if recording {
                    // Pulsing red recording indicator (frame-driven via repaint).
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
                ui.colored_label(egui::Color32::from_rgb(255, 210, 80), err);
            }
            if let Some(err) = &self.tray_error {
                ui.colored_label(egui::Color32::from_rgb(255, 120, 80), err);
            }
            ui.add_space(2.0);
        });

        // ── Bottom: sticky primary actions for active tab ────────────────
        egui::TopBottomPanel::bottom("bottom_actions").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                match self.main_tab {
                    MainTab::Stt => {
                        if recording {
                            if danger_button(ui, "Stop & transcribe").clicked() {
                                self.ptt_release();
                            }
                        } else if primary_button(ui, "Record").clicked() {
                            self.ptt_press();
                        }
                        if ui.button("Transcribe file…").clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("audio", &["wav", "mp3", "m4a", "flac", "ogg"])
                                .pick_file()
                            {
                                self.do_transcribe_file(path);
                            }
                        }
                        if ui.button("Copy transcript").clicked() {
                            let _ = x11util::write_clipboard(&self.transcript);
                        }
                        if ui.button("Clear").clicked() {
                            self.transcript.clear();
                        }
                    }
                    MainTab::Tts => {
                        if primary_button(ui, "Speak").clicked() {
                            let t = self.tts_text.clone();
                            self.do_speak(&t);
                        }
                        let pause_label = match self.transport.status() {
                            TransportStatus::Paused => "Resume",
                            _ => "Pause",
                        };
                        if ui.button(pause_label).clicked() {
                            self.transport.toggle_pause();
                        }
                        if danger_button(ui, "Stop").clicked() {
                            self.cancel_tts_pipeline();
                            self.status = "playback stopped".into();
                        }
                        if ui
                            .button("Replay")
                            .on_hover_text(
                                "Replay last audio",
                            )
                            .clicked()
                        {
                            match self.replay_last() {
                                Ok(true) => self.status = "replaying...".into(),
                                Ok(false) => self.status = "nothing to replay".into(),
                                Err(e) => self.status = format!("replay error: {e:#}"),
                            }
                        }
                        if ui.button("Read selection").clicked() {
                            self.read_aloud();
                        }
                        if ui.button("Speak file…").clicked() {
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
            // Tab bar
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (tab, label) in [
                    (MainTab::Stt, "  STT  "),
                    (MainTab::Tts, "  TTS  "),
                    (MainTab::Settings, "  Settings  "),
                ] {
                    let selected = self.main_tab == tab;
                    let text = if selected {
                        egui::RichText::new(label).strong()
                    } else {
                        egui::RichText::new(label)
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
            ui.add_space(6.0);
            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    match self.main_tab {
                        MainTab::Stt => self.ui_tab_stt(ui),
                        MainTab::Tts => self.ui_tab_tts(ui),
                        MainTab::Settings => self.ui_tab_settings(ui, ctx),
                    }
                });
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Only reached on real process exit (hard quit or unexpected teardown).
        self.discard_all_tts_audio();
        let _ = self.workers.unload_all();
        self.workers.shutdown_all();
        if let Some(session) = self.recording.take() {
            let _ = stop_recording(session);
        }
        // Drop hotkeys so X11 grabs release before process death.
        self.hotkeys = None;
    }
}

