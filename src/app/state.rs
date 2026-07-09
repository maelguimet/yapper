//! YapperApp state construction and status labels.

use super::{HotkeyCaptureField, MainTab};
use crate::audio::{
    human_mic_label, list_capture_sources, resolve_mic_source, PulseSource, RecordingSession,
};
use crate::config::Config;
use crate::hotkeys::HotkeyHub;
use crate::lifecycle::ExitPromptState;
use crate::transport::AudioTransport;
use crate::tray::TrayHandle;
use crate::ui::load_status_label;
use crate::workers::{resolve_python_bin, resolve_python_root, WorkerManager};
use std::path::PathBuf;
use std::time::Instant;

pub struct YapperApp {
    pub(crate) cfg: Config,
    pub(crate) workers: WorkerManager,
    pub(crate) status: String,
    pub(crate) stt_model: String,
    pub(crate) tts_tone: String,
    pub(crate) tts_language: String,
    pub(crate) stt_language: String,
    pub(crate) transcript: String,
    pub(crate) tts_text: String,
    pub(crate) tones: Vec<String>,
    pub(crate) copy_transcript: bool,
    pub(crate) read_clipboard: bool,
    pub(crate) recording: Option<RecordingSession>,
    pub(crate) transport: AudioTransport,
    /// Remaining TTS segments waiting to synthesize (chunked path).
    pub(crate) tts_queue: Vec<String>,
    /// Index of next segment to synth (1-based status uses total).
    pub(crate) tts_queue_total: usize,
    /// Cancel flag for in-flight multi-segment speak.
    pub(crate) tts_cancel: bool,
    /// Paths of temp WAVs for current monologue (deleted on stop/finish).
    pub(crate) tts_chunk_paths: Vec<PathBuf>,
    /// Concatenated last successful monologue for whole-utterance replay when available.
    pub(crate) tts_last_full_path: Option<PathBuf>,
    pub(crate) hotkeys: Option<HotkeyHub>,
    pub(crate) hotkey_error: Option<String>,
    /// Field currently listening for a key combo (capture picker).
    pub(crate) hotkey_capture: Option<HotkeyCaptureField>,
    pub(crate) tray: Option<TrayHandle>,
    pub(crate) tray_error: Option<String>,
    /// First tray create already attempted (may still retry while failed).
    pub(crate) tray_tried: bool,
    /// Next tray create retry instant when first create failed.
    pub(crate) tray_retry_at: Option<Instant>,
    /// When true, window close is allowed to end the process (tray Quit / confirmed Exit).
    pub(crate) hard_quit_armed: bool,
    /// In-window Exit confirmation dialog state.
    pub(crate) exit_prompt: ExitPromptState,
    /// Pulse source names for the mic dropdown (plus empty = system default).
    pub(crate) mic_sources: Vec<PulseSource>,
    pub(crate) mic_list_error: Option<String>,
    /// Selected source name; empty string means system default.
    pub(crate) mic_source: String,
    /// Live peak level 0..=1 while recording.
    pub(crate) record_level: f32,
    /// Main workspace tab (STT / TTS / Settings).
    pub(crate) main_tab: MainTab,
    /// Theme applied once after first frame context is ready.
    pub(crate) theme_applied: bool,
}

impl YapperApp {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>) -> Self {
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

    pub(crate) fn stt_status_label(&self) -> String {
        load_status_label(
            "STT",
            self.workers.stt_loaded(),
            self.workers.policy.stt.model_id.as_deref(),
        )
    }

    pub(crate) fn tts_status_label(&self) -> String {
        load_status_label(
            "TTS",
            self.workers.tts_loaded(),
            self.workers.policy.tts.model_id.as_deref(),
        )
    }

    pub(crate) fn refresh_mic_sources(&mut self) {
        match list_capture_sources() {
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

    pub(crate) fn active_mic_label(&self) -> String {
        let resolved = resolve_mic_source(&self.mic_source);
        if resolved == "default" {
            "System default".into()
        } else if let Some(src) = self.mic_sources.iter().find(|s| s.name == resolved) {
            src.label()
        } else {
            human_mic_label(resolved)
        }
    }

    pub(crate) fn persist(&mut self) {
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

}
