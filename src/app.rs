//! Main egui window + tray orchestration.

use crate::audio::{
    list_pulse_sources, resolve_mic_source, start_recording, stop_recording, temp_wav_path,
    PulseSource, RecordingSession,
};
use crate::config::Config;
use crate::hotkeys::{
    canonicalize_hotkey_spec, capture_mod_state, format_capture_hotkey, reregister, HotkeyAction,
    HotkeyHub,
};
use crate::lifecycle::{
    close_request_intent, minimize_request_intent, should_cancel_close, tray_menu_intent,
    ExitPromptState, ShellIntent,
};
use crate::policy::Role;
use crate::segment::split_for_tts;
use crate::transport::{AudioTransport, TransportStatus};
use crate::tray::{tray_failure_hint, TrayAction, TrayHandle};
use crate::workers::{resolve_python_bin, resolve_python_root, WorkerManager};
use crate::x11util::{self, ClipboardSel};
use anyhow::Result;
use eframe::egui;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Minimum window size so controls are not born clipped (Phase 10 / B6).
const MIN_WINDOW_WIDTH: f32 = 720.0;
const MIN_WINDOW_HEIGHT: f32 = 560.0;
const DEFAULT_WINDOW_WIDTH: f32 = 880.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 720.0;
/// Max grapheme-ish chars for mic labels in combo chrome (full name on hover).
const MIC_LABEL_MAX_CHARS: usize = 42;
/// Minimum multiline rows for transcript / TTS; grows with available height.
const TEXT_PANEL_MIN_ROWS: usize = 6;
const TEXT_PANEL_MAX_ROWS: usize = 28;
const TEXT_ROW_HEIGHT_EST: f32 = 18.0;
/// Soft warn when TTS paste is large (still allowed).
pub const TTS_LONG_TEXT_WARN_CHARS: usize = 800;
/// Hard-ish warn threshold for very long monologues.
pub const TTS_VERY_LONG_TEXT_CHARS: usize = 2_500;

/// Which hotkey field is listening for a key-capture press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotkeyCaptureField {
    ReadAloud,
    PushToTalk,
}

/// Primary work tabs: STT and TTS are peer workspaces; Settings holds models/hotkeys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainTab {
    Stt,
    Tts,
    Settings,
}

/// TTS character-count label + optional length warning (pure, tested).
pub fn tts_text_stats(text: &str) -> (String, Option<String>) {
    let n = text.chars().count();
    let label = if n == 1 {
        "1 character".into()
    } else {
        format!("{n} characters")
    };
    let warn = if n >= TTS_VERY_LONG_TEXT_CHARS {
        Some(format!(
            "Very long text ({n} chars) — synthesis may take a while; streaming splits by sentence."
        ))
    } else if n >= TTS_LONG_TEXT_WARN_CHARS {
        Some(format!(
            "Long paste ({n} chars) — first audio after the first sentence when streaming."
        ))
    } else {
        None
    };
    (label, warn)
}

/// Empty-state guidance when a work action cannot run yet.
pub fn stt_empty_guidance(stt_loaded: bool, mic_ok: bool) -> Option<&'static str> {
    if !mic_ok {
        return Some("Select a microphone (or system default) before recording.");
    }
    if !stt_loaded {
        return Some("Load STT (Models in Settings, or tray menu) before dictation.");
    }
    None
}

pub fn tts_empty_guidance(tts_loaded: bool, text_empty: bool) -> Option<&'static str> {
    if text_empty {
        return Some("Paste or type text to speak, or use Read selection / Speak file.");
    }
    if !tts_loaded {
        return Some("Load TTS before speaking (Settings → Models, or tray menu).");
    }
    None
}

/// Apply a high-contrast dark theme (not default grey soup).
pub fn apply_yapper_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let mut visuals = egui::Visuals::dark();
    visuals.window_fill = egui::Color32::from_rgb(22, 24, 28);
    visuals.panel_fill = egui::Color32::from_rgb(28, 31, 36);
    visuals.extreme_bg_color = egui::Color32::from_rgb(18, 20, 24);
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

fn section_heading(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(title).strong().size(16.0).color(egui::Color32::from_rgb(
        140, 190, 255,
    )));
    ui.add_space(2.0);
}

fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).strong())
            .fill(egui::Color32::from_rgb(50, 110, 200))
            .min_size(egui::vec2(110.0, 28.0)),
    )
}

fn danger_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).strong())
            .fill(egui::Color32::from_rgb(160, 50, 50))
            .min_size(egui::vec2(96.0, 28.0)),
    )
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
    transport: AudioTransport,
    /// Remaining TTS segments waiting to synthesize (chunked path).
    tts_queue: Vec<String>,
    /// Index of next segment to synth (1-based status uses total).
    tts_queue_total: usize,
    /// Cancel flag for in-flight multi-segment speak.
    tts_cancel: bool,
    /// Paths of temp WAVs for current monologue (deleted on stop/finish).
    tts_chunk_paths: Vec<PathBuf>,
    /// Concatenated last successful monologue for whole-utterance replay when available.
    tts_last_full_path: Option<PathBuf>,
    hotkeys: Option<HotkeyHub>,
    hotkey_error: Option<String>,
    /// Field currently listening for a key combo (capture picker).
    hotkey_capture: Option<HotkeyCaptureField>,
    tray: Option<TrayHandle>,
    tray_error: Option<String>,
    /// First tray create already attempted (may still retry while failed).
    tray_tried: bool,
    /// Next tray create retry instant when first create failed.
    tray_retry_at: Option<Instant>,
    /// When true, window close is allowed to end the process (tray Quit / confirmed Exit).
    hard_quit_armed: bool,
    /// In-window Exit confirmation dialog state.
    exit_prompt: ExitPromptState,
    /// Pulse source names for the mic dropdown (plus empty = system default).
    mic_sources: Vec<PulseSource>,
    mic_list_error: Option<String>,
    /// Selected source name; empty string means system default.
    mic_source: String,
    /// Live peak level 0..=1 while recording.
    record_level: f32,
    /// Main workspace tab (STT / TTS / Settings).
    main_tab: MainTab,
    /// Theme applied once after first frame context is ready.
    theme_applied: bool,
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
            transport: AudioTransport::default(),
            tts_queue: Vec::new(),
            tts_queue_total: 0,
            tts_cancel: false,
            tts_chunk_paths: Vec::new(),
            tts_last_full_path: None,
            hotkeys,
            hotkey_error,
            hotkey_capture: None,
            tray: None,
            tray_error: None,
            tray_tried: false,
            tray_retry_at: None,
            hard_quit_armed: false,
            exit_prompt: ExitPromptState::Idle,
            mic_sources: Vec::new(),
            mic_list_error: None,
            mic_source,
            record_level: 0.0,
            main_tab: MainTab::Stt,
            theme_applied: false,
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
        let read_canon = match canonicalize_hotkey_spec(&self.cfg.hotkeys.read_aloud) {
            Ok(s) => s,
            Err(e) => {
                self.hotkey_error = Some(format!("read-aloud invalid: {e:#}"));
                self.status = "hotkey update failed".into();
                return;
            }
        };
        let ptt_canon = match canonicalize_hotkey_spec(&self.cfg.hotkeys.push_to_talk) {
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
        // Drop previous grabs *before* registering — double-register is the B13 bug.
        let previous = self.hotkeys.take();
        match reregister(
            previous,
            &self.cfg.hotkeys.read_aloud,
            &self.cfg.hotkeys.push_to_talk,
        ) {
            Ok(h) => {
                self.hotkeys = Some(h);
                self.hotkey_error = None;
                self.hotkey_capture = None;
                self.status = format!(
                    "hotkeys live: {} | {}",
                    self.cfg.hotkeys.read_aloud, self.cfg.hotkeys.push_to_talk
                );
            }
            Err(e) => {
                // Leave hub empty so we never claim grabs we do not hold.
                self.hotkeys = None;
                self.hotkey_error = Some(format!(
                    "hotkey grab failed (DE conflict or bad combo): {e:#}. \
                     Rebind or free the shortcut in system settings, then Apply again."
                ));
                self.status = "hotkey update failed — shortcuts inactive until fixed".into();
            }
        }
    }

    fn hide_to_tray(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        // Also clear minimized state so Open restores cleanly.
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        self.status = "hidden to tray (right-click tray → Open / Quit)".into();
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        self.status = "window focused".into();
    }

    fn arm_hard_quit_and_close(&mut self, ctx: &egui::Context) {
        self.hard_quit_armed = true;
        self.cancel_tts_pipeline();
        let _ = self.workers.unload_all();
        self.workers.shutdown_all();
        if let Some(session) = self.recording.take() {
            let _ = stop_recording(session);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn cancel_tts_pipeline(&mut self) {
        self.tts_cancel = true;
        self.tts_queue.clear();
        self.tts_queue_total = 0;
        self.transport.set_pending_queue(false);
        self.transport.stop();
        self.cleanup_chunk_temps();
    }

    fn cleanup_chunk_temps(&mut self) {
        for p in self.tts_chunk_paths.drain(..) {
            let _ = std::fs::remove_file(p);
        }
    }

    /// Intercept title-bar close / minimize → hide; only hard_quit_armed exits.
    fn handle_viewport_lifecycle(&mut self, ctx: &egui::Context) {
        let (close_req, minimized) = ctx.input(|i| {
            let vp = i.viewport();
            (vp.close_requested(), vp.minimized.unwrap_or(false))
        });

        if close_req {
            if should_cancel_close(self.hard_quit_armed) {
                // Always-on product: close button hides, does not exit.
                let _ = close_request_intent();
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.hide_to_tray(ctx);
            }
            // else: hard quit armed — allow process exit
            return;
        }

        if minimized {
            let _ = minimize_request_intent();
            self.hide_to_tray(ctx);
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
        self.cancel_tts_pipeline();
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
            if !self.workers.tts_loaded() {
                return;
            }
        }
        // Cancel any prior monologue; start chunked pipeline.
        self.cancel_tts_pipeline();
        self.tts_cancel = false;
        let segments = split_for_tts(text);
        if segments.is_empty() {
            self.status = "nothing to speak".into();
            return;
        }
        self.tts_queue_total = segments.len();
        self.tts_queue = segments;
        self.transport.set_pending_queue(self.tts_queue.len() > 1);
        self.status = format!("synthesizing 1/{}…", self.tts_queue_total);
        self.pump_tts_queue();
    }

    /// Synth next segment if transport is free enough to accept more audio.
    fn pump_tts_queue(&mut self) {
        if self.tts_cancel {
            return;
        }
        // Only start next segment when idle/buffering or queue is the first item.
        let can_start = matches!(
            self.transport.status(),
            TransportStatus::Idle | TransportStatus::Buffering
        );
        if !can_start {
            return;
        }
        let Some(segment) = self.tts_queue.first().cloned() else {
            self.transport.set_pending_queue(false);
            return;
        };
        let done = self.tts_queue_total.saturating_sub(self.tts_queue.len()) + 1;
        self.status = format!("synthesizing {done}/{}…", self.tts_queue_total);
        let out = temp_wav_path("tts-chunk");
        match self.workers.synthesize(
            &segment,
            &self.tts_language,
            &self.tts_tone,
            &self.cfg.tts.voice,
            &out,
        ) {
            Ok(path) => {
                let _ = self.tts_queue.remove(0);
                self.tts_chunk_paths.push(path.clone());
                self.tts_last_full_path = Some(path.clone());
                self.transport
                    .set_pending_queue(!self.tts_queue.is_empty());
                if let Err(e) = self.transport.play_file(&path) {
                    self.status = format!("playback error: {e:#}");
                    return;
                }
                if self.tts_queue.is_empty() {
                    self.status = format!(
                        "speaking ({})…",
                        self.transport.machine().format_time_label()
                    );
                } else {
                    self.status = format!(
                        "speaking {done}/{} — more buffering…",
                        self.tts_queue_total
                    );
                }
            }
            Err(e) => {
                // Isolate bad chunk: drop it and try next unless queue empty.
                let _ = self.tts_queue.remove(0);
                self.status = format!("segment {done} failed: {e:#}");
                if !self.tts_queue.is_empty() && !self.tts_cancel {
                    self.pump_tts_queue();
                } else {
                    self.transport.set_pending_queue(false);
                }
            }
        }
    }

    fn poll_transport(&mut self) {
        self.transport.tick();
        // When a chunk ends and more remain, synth+play next.
        if matches!(
            self.transport.status(),
            TransportStatus::Idle | TransportStatus::Buffering
        ) && !self.tts_queue.is_empty()
            && !self.tts_cancel
        {
            self.pump_tts_queue();
        } else if self.transport.status() == TransportStatus::Playing {
            self.status = format!(
                "speaking ({}) [{}]",
                self.transport.machine().format_time_label(),
                self.transport.status().as_str()
            );
        } else if self.transport.status() == TransportStatus::Paused {
            self.status = format!(
                "paused ({})",
                self.transport.machine().format_time_label()
            );
        } else if self.tts_queue.is_empty()
            && self.transport.status() == TransportStatus::Idle
            && self.tts_queue_total > 0
        {
            // Finished monologue — leave a quiet ready status once.
            if self.status.starts_with("speaking") || self.status.starts_with("paused") {
                self.status = "ready".into();
                self.tts_queue_total = 0;
                // Keep last chunk for replay; drop intermediates except last.
                if let Some(last) = self.tts_chunk_paths.pop() {
                    for p in self.tts_chunk_paths.drain(..) {
                        if p != last {
                            let _ = std::fs::remove_file(p);
                        }
                    }
                    self.tts_chunk_paths.push(last);
                }
            }
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
            hk.poll_events()
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
        if self.tray.is_some() {
            return;
        }
        // Retry with backoff while create fails (SNI host / display race).
        if let Some(at) = self.tray_retry_at {
            if Instant::now() < at {
                return;
            }
        }
        self.tray_tried = true;
        match TrayHandle::try_create() {
            Ok(t) => {
                self.tray = Some(t);
                self.tray_error = None;
                self.tray_retry_at = None;
            }
            Err(e) => {
                self.tray_error = Some(format!(
                    "TRAY MISSING — always-on shell is broken without an icon. {e:#}\n{}",
                    tray_failure_hint()
                ));
                // Keep retrying every 3s for a while after start.
                self.tray_retry_at = Some(Instant::now() + Duration::from_secs(3));
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
            match tray_menu_intent(a) {
                ShellIntent::ShowWindow => self.show_window(ctx),
                ShellIntent::HardQuit => self.arm_hard_quit_and_close(ctx),
                ShellIntent::HideToTray => self.hide_to_tray(ctx),
                ShellIntent::Noop => match a {
                    TrayAction::LoadStt => self.load_stt(),
                    TrayAction::UnloadStt => self.unload_stt(),
                    TrayAction::LoadTts => self.load_tts(),
                    TrayAction::UnloadTts => self.unload_tts(),
                    _ => {}
                },
            }
        }
    }

    fn ui_tab_stt(&mut self, ui: &mut egui::Ui) {
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
            ui.checkbox(&mut self.copy_transcript, "Also copy transcript to clipboard");
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
            ui.weak("Empty — hold the push-to-talk hotkey or press Record.");
        }
        let stt_rows = text_panel_rows(ui.available_height(), 0.55);
        ui.add(
            egui::TextEdit::multiline(&mut self.transcript)
                .desired_width(f32::INFINITY)
                .desired_rows(stt_rows)
                .hint_text("Transcript appears here…"),
        );
    }

    fn ui_tab_tts(&mut self, ui: &mut egui::Ui) {
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
            "Read-aloud hotkey uses clipboard (else primary selection)",
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
        let segs = split_for_tts(&self.tts_text).len();
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
                .hint_text("Type or paste text to speak…"),
        );
    }

    fn ui_tab_settings(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
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
            "Live: read-aloud {}  ·  hold-to-talk {}",
            self.cfg.hotkeys.read_aloud, self.cfg.hotkeys.push_to_talk
        ));
        self.ui_hotkey_row(ui, "Read aloud", HotkeyCaptureField::ReadAloud);
        self.ui_hotkey_row(ui, "Hold-to-talk", HotkeyCaptureField::PushToTalk);
        ui.weak(
            "Capture: press a combo in this window, then Apply. Super/Win is read via X11 Mod4.",
        );
        if primary_button(ui, "Apply hotkeys").clicked() {
            self.apply_hotkeys();
        }

        ui.add_space(10.0);
        section_heading(ui, "Devices");
        self.ui_mic_controls(ui);

        ui.add_space(10.0);
        section_heading(ui, "Session");
        ui.weak("Close or minimize hides to the tray. Quit only from tray menu or Exit…");
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
        if !self.theme_applied {
            apply_yapper_theme(ctx);
            self.theme_applied = true;
        }

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
                        .button("Hide to tray")
                        .on_hover_text("Keep process + hotkeys; tray → Open to return")
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
                        "  ● RECORDING  ",
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
                            .on_hover_text("Play last successful audio without re-synthesizing")
                            .clicked()
                        {
                            match self.transport.replay() {
                                Ok(true) => self.status = "replaying…".into(),
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
                        if ui.button("Hide to tray").clicked() {
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
                         Prefer Hide to tray if you only want the window gone.",
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.exit_prompt = self.exit_prompt.on_cancel();
                        }
                        if ui
                            .button("Exit completely")
                            .on_hover_text("Same as tray → Quit")
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
                    (MainTab::Stt, "  Speech → Text  "),
                    (MainTab::Tts, "  Text → Speech  "),
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
        self.cancel_tts_pipeline();
        let _ = self.workers.unload_all();
        self.workers.shutdown_all();
        if let Some(session) = self.recording.take() {
            let _ = stop_recording(session);
        }
        // Drop hotkeys so X11 grabs release before process death.
        self.hotkeys = None;
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

    #[test]
    fn tts_text_stats_counts_and_warns() {
        let (s, w) = tts_text_stats("hi");
        assert_eq!(s, "2 characters");
        assert!(w.is_none());
        let (s1, _) = tts_text_stats("x");
        assert_eq!(s1, "1 character");
        let long: String = "a".repeat(TTS_LONG_TEXT_WARN_CHARS);
        let (_, w) = tts_text_stats(&long);
        assert!(w.unwrap().contains("Long paste"));
        let huge: String = "b".repeat(TTS_VERY_LONG_TEXT_CHARS);
        let (_, w2) = tts_text_stats(&huge);
        assert!(w2.unwrap().contains("Very long"));
    }

    #[test]
    fn empty_guidance_for_stt_and_tts() {
        assert!(stt_empty_guidance(false, true)
            .unwrap()
            .to_ascii_lowercase()
            .contains("load stt"));
        assert!(stt_empty_guidance(true, false)
            .unwrap()
            .to_ascii_lowercase()
            .contains("microphone"));
        assert!(stt_empty_guidance(true, true).is_none());
        assert!(tts_empty_guidance(false, false)
            .unwrap()
            .to_ascii_lowercase()
            .contains("load tts"));
        assert!(tts_empty_guidance(true, true)
            .unwrap()
            .to_ascii_lowercase()
            .contains("paste"));
        assert!(tts_empty_guidance(true, false).is_none());
    }

    #[test]
    fn theme_visuals_are_dark_not_default_grey() {
        // Structural: apply_yapper_theme is the shipped entry; dark panel fill is intentional.
        let ctx = egui::Context::default();
        apply_yapper_theme(&ctx);
        let v = &ctx.style().visuals;
        assert!(v.dark_mode);
        // Custom panel fill from apply_yapper_theme
        assert_eq!(v.panel_fill, egui::Color32::from_rgb(28, 31, 36));
    }
}
