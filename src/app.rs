//! Main egui window + tray orchestration.

use crate::audio::{
    list_pulse_sources, play_wav, resolve_mic_source, start_recording, stop_playback,
    stop_recording, temp_wav_path, PulseSource, RecordingSession,
};
use crate::config::Config;
use crate::hotkeys::{
    capture_mod_state, format_capture_hotkey, format_hotkey, parse_hotkey, HotkeyAction,
    HotkeyHub,
};
use crate::policy::Role;
use crate::tray::{TrayAction, TrayHandle};
use crate::workers::{resolve_python_bin, resolve_python_root, WorkerManager};
use crate::x11util::{self, ClipboardSel};
use anyhow::Result;
use eframe::egui;
use std::path::PathBuf;
use std::process::Child;

/// Minimum window size so controls are not born clipped (Phase 10 / B6).
const MIN_WINDOW_WIDTH: f32 = 640.0;
const MIN_WINDOW_HEIGHT: f32 = 520.0;
const DEFAULT_WINDOW_WIDTH: f32 = 720.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 860.0;
/// Max grapheme-ish chars for mic labels in combo chrome (full name on hover).
const MIC_LABEL_MAX_CHARS: usize = 42;
/// Minimum multiline rows for transcript / TTS; grows with available height.
const TEXT_PANEL_MIN_ROWS: usize = 6;
const TEXT_PANEL_MAX_ROWS: usize = 24;
const TEXT_ROW_HEIGHT_EST: f32 = 18.0;

/// Which hotkey field is listening for a key-capture press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotkeyCaptureField {
    ReadAloud,
    PushToTalk,
}

/// Ellipsize long display strings for combo boxes without losing short labels.
pub fn truncate_display(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    if max_chars == 1 {
        return "…".to_string();
    }
    let keep = max_chars - 1;
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

/// Symmetric load badge text: `STT ● medium` / `TTS ○ unloaded`.
pub fn load_status_label(role: &str, loaded: bool, model_id: Option<&str>) -> String {
    if loaded {
        match model_id.map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => format!("{role} ● {id}"),
            None => format!("{role} ● loaded"),
        }
    } else {
        format!("{role} ○ unloaded")
    }
}

/// Map an egui key to the hotkey config token (`S`, `1`, `Space`, …).
fn egui_key_to_token(key: egui::Key) -> Option<&'static str> {
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
        // Escape cancels capture; modifiers alone are not keys.
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

fn text_panel_rows(available_height: f32, share: f32) -> usize {
    let budget = (available_height * share).max(TEXT_PANEL_MIN_ROWS as f32 * TEXT_ROW_HEIGHT_EST);
    let rows = (budget / TEXT_ROW_HEIGHT_EST).floor() as usize;
    rows.clamp(TEXT_PANEL_MIN_ROWS, TEXT_PANEL_MAX_ROWS)
}

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
    recording: Option<RecordingSession>,
    playback: Option<Child>,
    hotkeys: Option<HotkeyHub>,
    hotkey_error: Option<String>,
    /// Field currently listening for a key combo (capture picker).
    hotkey_capture: Option<HotkeyCaptureField>,
    tray: Option<TrayHandle>,
    tray_error: Option<String>,
    tray_tried: bool,
    /// Pulse source names for the mic dropdown (plus empty = system default).
    mic_sources: Vec<PulseSource>,
    mic_list_error: Option<String>,
    /// Selected source name; empty string means system default.
    mic_source: String,
    /// Live peak level 0..=1 while recording.
    record_level: f32,
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
        let mic_source = cfg.audio.mic_source.clone();

        let _ = cc; // creation context reserved for future fonts/theme

        let mut app = Self {
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
            playback: None,
            hotkeys,
            hotkey_error,
            hotkey_capture: None,
            tray: None,
            tray_error: None,
            tray_tried: false,
            mic_sources: Vec::new(),
            mic_list_error: None,
            mic_source,
            record_level: 0.0,
        };
        app.refresh_mic_sources();
        app
    }

    fn stt_status_label(&self) -> String {
        load_status_label(
            "STT",
            self.workers.stt_loaded(),
            self.workers.policy.stt.model_id.as_deref(),
        )
    }

    fn tts_status_label(&self) -> String {
        load_status_label(
            "TTS",
            self.workers.tts_loaded(),
            self.workers.policy.tts.model_id.as_deref(),
        )
    }

    fn apply_hotkeys(&mut self) {
        let read = self.cfg.hotkeys.read_aloud.trim();
        let ptt = self.cfg.hotkeys.push_to_talk.trim();
        if read.is_empty() || ptt.is_empty() {
            self.hotkey_error = Some("hotkey binding cannot be empty".into());
            self.status = "hotkey update failed".into();
            return;
        }
        let read_canon = match parse_hotkey(read).and_then(|hk| format_hotkey(&hk)) {
            Ok(s) => s,
            Err(e) => {
                self.hotkey_error = Some(format!("read-aloud invalid: {e:#}"));
                self.status = "hotkey update failed".into();
                return;
            }
        };
        let ptt_canon = match parse_hotkey(ptt).and_then(|hk| format_hotkey(&hk)) {
            Ok(s) => s,
            Err(e) => {
                self.hotkey_error = Some(format!("push-to-talk invalid: {e:#}"));
                self.status = "hotkey update failed".into();
                return;
            }
        };
        self.cfg.hotkeys.read_aloud = read_canon;
        self.cfg.hotkeys.push_to_talk = ptt_canon;
        self.persist();
        match HotkeyHub::register(&self.cfg.hotkeys.read_aloud, &self.cfg.hotkeys.push_to_talk) {
            Ok(h) => {
                self.hotkeys = Some(h);
                self.hotkey_error = None;
                self.hotkey_capture = None;
                self.status = "hotkeys updated".into();
            }
            Err(e) => {
                self.hotkey_error = Some(format!("hotkey grab failed: {e:#}"));
                self.status = "hotkey update failed".into();
            }
        }
    }

    /// While a capture field is active, turn the next non-modifier key press into a binding.
    fn poll_hotkey_capture(&mut self, ctx: &egui::Context) {
        let Some(field) = self.hotkey_capture else {
            return;
        };

        let outcome = ctx.input(|i| {
            if i.key_pressed(egui::Key::Escape) {
                return Some(CaptureOutcome::Cancel);
            }
            for ev in &i.events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    repeat,
                    ..
                } = ev
                {
                    if *repeat {
                        continue;
                    }
                    if *key == egui::Key::Escape {
                        return Some(CaptureOutcome::Cancel);
                    }
                    let Some(token) = egui_key_to_token(*key) else {
                        continue;
                    };
                    // egui drops Super on Linux (mac_cmd always false; command=ctrl).
                    // Read Super/Mod4 from X11 session state during capture.
                    let platform_super = x11util::super_modifier_down();
                    let mods = capture_mod_state(
                        modifiers.mac_cmd,
                        modifiers.ctrl,
                        modifiers.alt,
                        modifiers.shift,
                        platform_super,
                    );
                    match format_capture_hotkey(mods, token) {
                        Ok(spec) => return Some(CaptureOutcome::Bound(spec)),
                        Err(e) => return Some(CaptureOutcome::Error(format!("{e:#}"))),
                    }
                }
            }
            None
        });

        match outcome {
            Some(CaptureOutcome::Cancel) => {
                self.hotkey_capture = None;
                self.status = "hotkey capture cancelled".into();
            }
            Some(CaptureOutcome::Bound(spec)) => {
                match field {
                    HotkeyCaptureField::ReadAloud => self.cfg.hotkeys.read_aloud = spec.clone(),
                    HotkeyCaptureField::PushToTalk => self.cfg.hotkeys.push_to_talk = spec.clone(),
                }
                self.hotkey_capture = None;
                self.hotkey_error = None;
                self.status = format!("captured {spec} (Apply to register)");
            }
            Some(CaptureOutcome::Error(msg)) => {
                self.hotkey_error = Some(msg);
                self.status = "hotkey capture failed".into();
                self.hotkey_capture = None;
            }
            None => {}
        }
    }

    fn ui_hotkey_row(&mut self, ui: &mut egui::Ui, label: &str, field: HotkeyCaptureField) {
        ui.horizontal(|ui| {
            ui.label(label);
            let capturing = self.hotkey_capture == Some(field);
            if capturing {
                ui.colored_label(egui::Color32::LIGHT_BLUE, "Press combo… (Esc cancel)");
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

    fn refresh_mic_sources(&mut self) {
        match list_pulse_sources() {
            Ok(list) => {
                self.mic_sources = list;
                self.mic_list_error = None;
            }
            Err(e) => {
                self.mic_sources.clear();
                self.mic_list_error = Some(format!("{e:#}"));
            }
        }
    }

    fn active_mic_label(&self) -> String {
        let resolved = resolve_mic_source(&self.mic_source);
        if resolved == "default" {
            "system default".into()
        } else {
            resolved.to_string()
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
        self.cfg.audio.mic_source = self.mic_source.clone();
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
        let source = self.mic_source.clone();
        match start_recording(&path, &source) {
            Ok(session) => {
                self.recording = Some(session);
                self.record_level = 0.0;
                self.status = format!("recording… ({})", self.active_mic_label());
            }
            Err(e) => self.status = format!("record error: {e:#}"),
        }
    }

    fn ptt_release(&mut self) {
        if let Some(session) = self.recording.take() {
            self.record_level = 0.0;
            match stop_recording(session) {
                Ok(path) => {
                    if path.is_file() && path.metadata().map(|m| m.len() > 1000).unwrap_or(false) {
                        self.do_transcribe_file(path);
                        if !self.transcript.is_empty() {
                            if let Err(e) =
                                x11util::insert_transcript_at_cursor(&self.transcript, true)
                            {
                                self.status = format!("transcribed; paste failed: {e}");
                            } else {
                                self.status = "inserted transcript".into();
                            }
                        }
                    } else {
                        self.status = "recording too short / empty".into();
                    }
                }
                Err(e) => self.status = format!("record stop error: {e:#}"),
            }
        }
    }

    fn poll_record_level(&mut self) {
        if let Some(session) = self.recording.as_ref() {
            if let Some(level) = session.level_01() {
                // light smoothing so the bar is readable
                self.record_level = self.record_level * 0.4 + level * 0.6;
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

    fn ui_mic_controls(&mut self, ui: &mut egui::Ui) {
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
                .width(280.0)
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
                ui.colored_label(egui::Color32::RED, "● REC");
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
        } else {
            let device_full = self.active_mic_label();
            let device_shown = truncate_display(&device_full, MIC_LABEL_MAX_CHARS);
            ui.label(format!("Active mic: {device_shown}"))
                .on_hover_text(device_full);
        }
    }
}

enum CaptureOutcome {
    Bound(String),
    Cancel,
    Error(String),
}

impl eframe::App for YapperApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_hotkey_capture(ctx);
        self.poll_hotkeys();
        self.poll_tray(ctx);
        self.poll_record_level();
        // Faster repaint while capturing a hotkey so key events feel immediate.
        let repaint_ms = if self.hotkey_capture.is_some() { 16 } else { 100 };
        ctx.request_repaint_after(std::time::Duration::from_millis(repaint_ms));

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
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());

                    ui.collapsing("Models / load", |ui| {
                        ui.horizontal(|ui| {
                            ui.label("STT model");
                            egui::ComboBox::from_id_salt("stt_model")
                                .selected_text(&self.stt_model)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.stt_model,
                                        "small".into(),
                                        "small",
                                    );
                                    ui.selectable_value(
                                        &mut self.stt_model,
                                        "medium".into(),
                                        "medium",
                                    );
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
                    });

                    ui.separator();
                    ui.heading("Speech to Text");
                    self.ui_mic_controls(ui);
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
                    let stt_rows = text_panel_rows(ui.available_height(), 0.28);
                    ui.add(
                        egui::TextEdit::multiline(&mut self.transcript)
                            .desired_width(f32::INFINITY)
                            .desired_rows(stt_rows),
                    );

                    ui.separator();
                    ui.heading("Text to Speech");
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
                    let tts_rows = text_panel_rows(ui.available_height(), 0.28);
                    ui.add(
                        egui::TextEdit::multiline(&mut self.tts_text)
                            .desired_width(f32::INFINITY)
                            .desired_rows(tts_rows),
                    );

                    ui.separator();
                    ui.heading("Hotkeys");
                    ui.label(format!(
                        "Read aloud: {}  |  Hold-to-talk: {}",
                        self.cfg.hotkeys.read_aloud, self.cfg.hotkeys.push_to_talk
                    ));
                    self.ui_hotkey_row(ui, "Read aloud", HotkeyCaptureField::ReadAloud);
                    self.ui_hotkey_row(ui, "Hold-to-talk", HotkeyCaptureField::PushToTalk);
                    ui.label(
                        "Capture: press a combo in the window, then Apply. Super/Win is read via X11 Mod4 on Linux (not only Ctrl/Alt/Shift).",
                    );
                    if ui.button("Apply hotkeys & save config").clicked() {
                        self.apply_hotkeys();
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
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.workers.unload_all();
        self.workers.shutdown_all();
        stop_playback(&mut self.playback);
        if let Some(session) = self.recording.take() {
            let _ = stop_recording(session);
        }
    }
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
    use super::*;

    #[test]
    fn truncate_preserves_short_strings() {
        assert_eq!(truncate_display("TONOR", 42), "TONOR");
        assert_eq!(truncate_display("", 10), "");
        assert_eq!(truncate_display("abc", 3), "abc");
    }

    #[test]
    fn truncate_ellipsizes_long_device_strings() {
        let long = "alsa_input.usb-TONOR_INC._TONOR_TC30_XXXX-00.analog-stereo";
        let out = truncate_display(long, 20);
        assert_eq!(out.chars().count(), 20);
        assert!(out.ends_with('…'), "{out}");
        assert!(out.starts_with("alsa_input"), "{out}");
        // Identity of short strings and different max widths
        assert_ne!(truncate_display(long, 20), truncate_display(long, 30));
    }

    #[test]
    fn load_status_symmetric_with_model_id() {
        assert_eq!(
            load_status_label("STT", true, Some("medium")),
            "STT ● medium"
        );
        assert_eq!(
            load_status_label("TTS", true, Some("chatterbox-multilingual")),
            "TTS ● chatterbox-multilingual"
        );
        assert_eq!(load_status_label("STT", false, None), "STT ○ unloaded");
        assert_eq!(load_status_label("TTS", false, Some("x")), "TTS ○ unloaded");
        assert_eq!(
            load_status_label("STT", true, Some("")),
            "STT ● loaded"
        );
        assert_eq!(load_status_label("TTS", true, None), "TTS ● loaded");
    }

    #[test]
    fn text_panel_rows_clamped() {
        assert_eq!(text_panel_rows(50.0, 0.28), TEXT_PANEL_MIN_ROWS);
        assert!(text_panel_rows(2000.0, 0.5) <= TEXT_PANEL_MAX_ROWS);
        assert!(text_panel_rows(400.0, 0.28) >= TEXT_PANEL_MIN_ROWS);
    }
}
