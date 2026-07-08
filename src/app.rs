//! Main egui window + tray orchestration.

use crate::audio::{play_wav, start_recording, stop_playback, stop_recording, temp_wav_path};
use crate::config::Config;
use crate::hotkeys::{HotkeyAction, HotkeyHub};
use crate::policy::Role;
use crate::tray::{TrayAction, TrayHandle};
use crate::workers::{resolve_python_bin, resolve_python_root, WorkerManager};
use crate::x11util::{self, ClipboardSel};
use anyhow::Result;
use eframe::egui;
use std::path::PathBuf;
use std::process::Child;

pub struct YapperApp {
    cfg: Config,
    workers: WorkerManager,
    status: String,
    stt_model: String,
    tts_tone: String,
    tts_language: String,
    stt_language: String,
    transcript: String,
    tts_text: String,
    tones: Vec<String>,
    copy_transcript: bool,
    read_clipboard: bool,
    recording: Option<Child>,
    record_path: Option<PathBuf>,
    playback: Option<Child>,
    hotkeys: Option<HotkeyHub>,
    hotkey_error: Option<String>,
    tray: Option<TrayHandle>,
    tray_error: Option<String>,
    tray_tried: bool,
}

impl YapperApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut cfg = Config::load_or_default().unwrap_or_default();
        // Resolve paths for this install/checkout
        cfg.paths.python_root = resolve_python_root(&cfg).to_string_lossy().into();
        cfg.paths.python_bin = resolve_python_bin(&cfg);

        let mut workers = WorkerManager::new(cfg.clone());
        let tones = workers.list_tones().unwrap_or_else(|_| {
            vec![
                "neutral".into(),
                "calm".into(),
                "excited".into(),
                "serious".into(),
            ]
        });

        let (hotkeys, hotkey_error) =
            match HotkeyHub::register(&cfg.hotkeys.read_aloud, &cfg.hotkeys.push_to_talk) {
                Ok(h) => (Some(h), None),
                Err(e) => (None, Some(format!("hotkey grab failed: {e:#}"))),
            };

        let stt_model = cfg.stt.model.clone();
        let tts_tone = cfg.tts.tone.clone();
        let tts_language = if cfg.tts.language == "auto" {
            "en".into()
        } else {
            cfg.tts.language.clone()
        };
        let stt_language = cfg.stt.language.clone();
        let copy_transcript = cfg.stt.copy_transcript;
        let read_clipboard = cfg.read_aloud.source == "clipboard";

        let _ = cc; // creation context reserved for future fonts/theme

        Self {
            cfg,
            workers,
            status: "ready".into(),
            stt_model,
            tts_tone,
            tts_language,
            stt_language,
            transcript: String::new(),
            tts_text: String::new(),
            tones,
            copy_transcript,
            read_clipboard,
            recording: None,
            record_path: None,
            playback: None,
            hotkeys,
            hotkey_error,
            tray: None,
            tray_error: None,
            tray_tried: false,
        }
    }

    fn persist(&mut self) {
        self.cfg.stt.model = self.stt_model.clone();
        self.cfg.stt.language = self.stt_language.clone();
        self.cfg.stt.copy_transcript = self.copy_transcript;
        self.cfg.tts.tone = self.tts_tone.clone();
        self.cfg.tts.language = self.tts_language.clone();
        self.cfg.read_aloud.source = if self.read_clipboard {
            "clipboard".into()
        } else {
            "selection".into()
        };
        if let Err(e) = self.cfg.save_default_location() {
            self.status = format!("config save failed: {e}");
        }
    }

    fn load_stt(&mut self) {
        self.status = format!("loading STT {}…", self.stt_model);
        match self
            .workers
            .load(Role::Stt, &self.stt_model, "cuda")
        {
            Ok(()) => self.status = format!("STT {} loaded", self.stt_model),
            Err(e) => self.status = format!("STT load error: {e:#}"),
        }
    }

    fn unload_stt(&mut self) {
        match self.workers.unload(Role::Stt) {
            Ok(()) => self.status = "STT unloaded".into(),
            Err(e) => self.status = format!("STT unload error: {e:#}"),
        }
    }

    fn load_tts(&mut self) {
        self.status = "loading TTS…".into();
        match self
            .workers
            .load(Role::Tts, "chatterbox-multilingual", "cuda")
        {
            Ok(()) => self.status = "TTS loaded".into(),
            Err(e) => self.status = format!("TTS load error: {e:#}"),
        }
    }

    fn unload_tts(&mut self) {
        match self.workers.unload(Role::Tts) {
            Ok(()) => self.status = "TTS unloaded".into(),
            Err(e) => self.status = format!("TTS unload error: {e:#}"),
        }
    }

    fn do_transcribe_file(&mut self, path: PathBuf) {
        if !self.workers.stt_loaded() {
            self.load_stt();
        }
        match self.workers.transcribe(&path, &self.stt_language) {
            Ok(text) => {
                self.transcript = text.clone();
                if self.copy_transcript {
                    if let Err(e) = x11util::write_clipboard(&text) {
                        self.status = format!("transcribed; clipboard failed: {e}");
                        return;
                    }
                }
                self.status = "transcribed".into();
            }
            Err(e) => self.status = format!("transcribe error: {e:#}"),
        }
    }

    fn do_speak(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            self.status = "nothing to speak".into();
            return;
        }
        if !self.workers.tts_loaded() {
            self.load_tts();
        }
        let out = temp_wav_path("tts");
        match self.workers.synthesize(
            text,
            &self.tts_language,
            &self.tts_tone,
            &self.cfg.tts.voice,
            &out,
        ) {
            Ok(path) => {
                stop_playback(&mut self.playback);
                match play_wav(&path) {
                    Ok(child) => {
                        self.playback = Some(child);
                        self.status = "speaking…".into();
                    }
                    Err(e) => self.status = format!("playback error: {e:#}"),
                }
            }
            Err(e) => self.status = format!("synth error: {e:#}"),
        }
    }

    fn read_aloud(&mut self) {
        let text = if self.read_clipboard {
            x11util::read_selection(ClipboardSel::Clipboard).unwrap_or_default()
        } else {
            x11util::read_selection(ClipboardSel::Primary).unwrap_or_default()
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            self.status = "selection/clipboard empty".into();
            return;
        }
        self.tts_text = text.clone();
        self.do_speak(&text);
    }

    fn ptt_press(&mut self) {
        if self.recording.is_some() {
            return;
        }
        let path = temp_wav_path("rec");
        match start_recording(&path) {
            Ok(child) => {
                self.recording = Some(child);
                self.record_path = Some(path);
                self.status = "recording…".into();
            }
            Err(e) => self.status = format!("record error: {e:#}"),
        }
    }

    fn ptt_release(&mut self) {
        if let Some(child) = self.recording.take() {
            let _ = stop_recording(child);
            // tiny wait for file flush
            std::thread::sleep(std::time::Duration::from_millis(150));
            if let Some(path) = self.record_path.take() {
                if path.is_file() && path.metadata().map(|m| m.len() > 1000).unwrap_or(false) {
                    self.do_transcribe_file(path.clone());
                    if !self.transcript.is_empty() {
                        if let Err(e) = x11util::paste_at_cursor(&self.transcript) {
                            self.status = format!("transcribed; paste failed: {e}");
                        } else {
                            self.status = "inserted transcript".into();
                        }
                    }
                } else {
                    self.status = "recording too short / empty".into();
                }
            }
        }
    }

    fn poll_hotkeys(&mut self) {
        let events: Vec<_> = if let Some(hk) = self.hotkeys.as_ref() {
            hk.rx.try_iter().collect()
        } else {
            Vec::new()
        };
        for ev in events {
            match (ev.action, ev.pressed) {
                (HotkeyAction::ReadAloud, true) => self.read_aloud(),
                (HotkeyAction::PushToTalk, true) => self.ptt_press(),
                (HotkeyAction::PushToTalk, false) => self.ptt_release(),
                _ => {}
            }
        }
    }

    fn ensure_tray(&mut self) {
        if self.tray_tried {
            return;
        }
        self.tray_tried = true;
        // tray-icon/libappindicator needs a live display after eframe starts
        match TrayHandle::try_create() {
            Ok(t) => {
                self.tray = Some(t);
                self.tray_error = None;
            }
            Err(e) => {
                self.tray_error = Some(format!("tray unavailable: {e:#}"));
            }
        }
    }

    fn poll_tray(&mut self, ctx: &egui::Context) {
        self.ensure_tray();
        let actions: Vec<TrayAction> = if let Some(t) = self.tray.as_ref() {
            std::iter::from_fn(|| t.try_recv()).collect()
        } else {
            Vec::new()
        };
        for a in actions {
            match a {
                TrayAction::Open => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.status = "window focused".into();
                }
                TrayAction::LoadStt => self.load_stt(),
                TrayAction::UnloadStt => self.unload_stt(),
                TrayAction::LoadTts => self.load_tts(),
                TrayAction::UnloadTts => self.unload_tts(),
                TrayAction::Quit => {
                    let _ = self.workers.unload_all();
                    self.workers.shutdown_all();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }
}

impl eframe::App for YapperApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_hotkeys();
        self.poll_tray(ctx);
        ctx.request_repaint_after(std::time::Duration::from_millis(100));

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Yapper");
                ui.separator();
                ui.label(&self.status);
            });
            if let Some(err) = &self.hotkey_error {
                ui.colored_label(egui::Color32::YELLOW, err);
            }
            if let Some(err) = &self.tray_error {
                ui.colored_label(egui::Color32::YELLOW, err);
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.collapsing("Models / load", |ui| {
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
                    ui.label(if self.workers.stt_loaded() {
                        "● loaded"
                    } else {
                        "○ unloaded"
                    });
                });
                ui.horizontal(|ui| {
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
                    ui.label(if self.workers.tts_loaded() {
                        "TTS ●"
                    } else {
                        "TTS ○"
                    });
                });
            });

            ui.separator();
            ui.heading("Speech → Text");
            ui.horizontal(|ui| {
                ui.label("Language");
                egui::ComboBox::from_id_salt("stt_lang")
                    .selected_text(&self.stt_language)
                    .show_ui(ui, |ui| {
                        for l in ["auto", "en", "fr"] {
                            ui.selectable_value(&mut self.stt_language, l.into(), l);
                        }
                    });
                ui.checkbox(&mut self.copy_transcript, "Copy transcript to clipboard");
            });
            ui.horizontal(|ui| {
                if self.recording.is_none() {
                    if ui.button("Hold-to-talk (click = start)").clicked() {
                        self.ptt_press();
                    }
                } else if ui.button("Stop & transcribe").clicked() {
                    self.ptt_release();
                }
                if ui.button("Transcribe file…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("audio", &["wav", "mp3", "m4a", "flac", "ogg"])
                        .pick_file()
                    {
                        self.do_transcribe_file(path);
                    }
                }
                if ui.button("Copy").clicked() {
                    let _ = x11util::write_clipboard(&self.transcript);
                }
                if ui.button("Clear").clicked() {
                    self.transcript.clear();
                }
            });
            ui.add(
                egui::TextEdit::multiline(&mut self.transcript)
                    .desired_width(f32::INFINITY)
                    .desired_rows(6),
            );

            ui.separator();
            ui.heading("Text → Speech");
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
                "Read aloud uses clipboard (else primary selection)",
            );
            ui.horizontal(|ui| {
                if ui.button("Speak").clicked() {
                    let t = self.tts_text.clone();
                    self.do_speak(&t);
                }
                if ui.button("Stop").clicked() {
                    stop_playback(&mut self.playback);
                    self.status = "playback stopped".into();
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
                if ui.button("Read selection/clipboard").clicked() {
                    self.read_aloud();
                }
            });
            ui.add(
                egui::TextEdit::multiline(&mut self.tts_text)
                    .desired_width(f32::INFINITY)
                    .desired_rows(6),
            );

            ui.separator();
            ui.heading("Hotkeys");
            ui.label(format!(
                "Read aloud: {}  |  Hold-to-talk: {}",
                self.cfg.hotkeys.read_aloud, self.cfg.hotkeys.push_to_talk
            ));
            ui.horizontal(|ui| {
                ui.label("Rebind read-aloud");
                ui.text_edit_singleline(&mut self.cfg.hotkeys.read_aloud);
            });
            ui.horizontal(|ui| {
                ui.label("Rebind PTT");
                ui.text_edit_singleline(&mut self.cfg.hotkeys.push_to_talk);
            });
            if ui.button("Apply hotkeys & save config").clicked() {
                self.persist();
                match HotkeyHub::register(&self.cfg.hotkeys.read_aloud, &self.cfg.hotkeys.push_to_talk)
                {
                    Ok(h) => {
                        self.hotkeys = Some(h);
                        self.hotkey_error = None;
                        self.status = "hotkeys updated".into();
                    }
                    Err(e) => {
                        self.hotkey_error = Some(format!("hotkey grab failed: {e:#}"));
                        self.status = "hotkey update failed".into();
                    }
                }
            }

            ui.separator();
            if ui.button("Save settings").clicked() {
                self.persist();
                self.status = "settings saved".into();
            }
            if ui.button("Quit").clicked() {
                let _ = self.workers.unload_all();
                self.workers.shutdown_all();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.workers.unload_all();
        self.workers.shutdown_all();
        stop_playback(&mut self.playback);
        if let Some(c) = self.recording.take() {
            let _ = stop_recording(c);
        }
    }
}

pub fn run_gui() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([720.0, 820.0])
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
