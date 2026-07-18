//! YapperApp state construction and status labels.

use super::jobs::JobHub;
use super::messages::JobCmd;
use super::tts_controller::TtsController;
use super::{HotkeyCaptureField, MainTab};
use crate::audio::{
    human_mic_label, list_capture_sources, resolve_mic_source, PulseSource, RecordingSession,
};
use crate::config::Config;
use crate::hotkeys::HotkeyHub;
use crate::lifecycle::ExitPromptState;
use crate::transport::AudioTransport;
use crate::tray::TrayHandle;
use crate::ui::{
    dictation_chip_label, fallback_tones, load_status_label, stt_ready_for_selected, voice_chip_label,
};
use crate::workers::{resolve_python_bin, resolve_python_root};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Backoff after a failed config save so we do not spam persist every frame.
const AUTOSAVE_FAIL_BACKOFF: Duration = Duration::from_secs(30);

// Re-export for pipeline/UI modules that historically imported from state.
pub(crate) use super::messages::RecordingIntent;

/// Post-transcribe follow-up driven by [`RecordingIntent`] (pure; unit-tested).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscribeFollowUp {
    /// Update transcript panel (and optional copy); do not paste.
    PanelOnly,
    /// Also paste at cursor via X11 insert path (no Enter).
    InsertAtCursor,
}

/// Decide insert vs panel-only from intent + text. Empty text never inserts.
pub(crate) fn follow_up_after_transcribe(
    intent: RecordingIntent,
    text: &str,
) -> TranscribeFollowUp {
    if intent.wants_insert_at_cursor() && !text.is_empty() {
        TranscribeFollowUp::InsertAtCursor
    } else {
        TranscribeFollowUp::PanelOnly
    }
}

/// Status string after a successful panel fill (and optional insert attempt).
pub(crate) fn status_after_transcribe_success(follow_up: TranscribeFollowUp) -> &'static str {
    match follow_up {
        TranscribeFollowUp::InsertAtCursor => "inserted transcript",
        TranscribeFollowUp::PanelOnly => "transcribed",
    }
}

/// Live status while the mic is open (GUI vs hotkey wording).
pub(crate) fn recording_status_line(intent: RecordingIntent, mic_label: &str) -> String {
    match intent {
        RecordingIntent::Idle => "ready".into(),
        RecordingIntent::GuiPanel => format!("recording for transcript… ({mic_label})"),
        RecordingIntent::HotkeyInsert => format!("hold-to-dictate… ({mic_label})"),
    }
}

/// Card / chrome label while recording (GUI panel vs PTT insert).
pub(crate) fn recording_card_label(intent: RecordingIntent) -> &'static str {
    match intent {
        RecordingIntent::HotkeyInsert => "Hold-to-dictate",
        RecordingIntent::GuiPanel | RecordingIntent::Idle => "Recording",
    }
}

pub struct YapperApp {
    pub(crate) cfg: Config,
    /// Background job hub owns real WorkerManager (never block UI on synth/load).
    pub(crate) jobs: JobHub,
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
    /// Chunked TTS prebuffer controller (job ids, ready queue).
    pub(crate) tts: TtsController,
    /// Durable full-utterance WAV for Replay (concatenated multi-chunk).
    pub(crate) tts_last_full_path: Option<PathBuf>,
    /// Mirror of worker load state (updated via AppMsg::ModelStatus).
    pub(crate) stt_loaded: bool,
    pub(crate) stt_model_id: Option<String>,
    pub(crate) tts_loaded: bool,
    pub(crate) tts_model_id: Option<String>,
    pub(crate) stt_loading: bool,
    pub(crate) tts_loading: bool,
    /// Pending transcribe after STT autoload (path + intent; job_id assigned on send).
    pub(crate) pending_transcribe: Option<(PathBuf, RecordingIntent)>,
    /// Pending speak after TTS autoload.
    pub(crate) pending_speak: Option<String>,
    /// Intent for the open mic session only (UI labels / release matching).
    /// Completion uses job-scoped intent on Transcribed, not this alone.
    pub(crate) recording_intent: RecordingIntent,
    /// Live STT job id; stale Transcribed/Failed results are ignored.
    pub(crate) live_stt_job: Option<u64>,
    /// Monotonic allocator for STT transcribe job ids.
    pub(crate) next_stt_job_id: u64,
    pub(crate) hotkeys: Option<HotkeyHub>,
    pub(crate) hotkey_error: Option<String>,
    pub(crate) hotkey_capture: Option<HotkeyCaptureField>,
    /// Settings editor draft for read-aloud (not live until successful Apply).
    pub(crate) hotkey_draft_read_aloud: String,
    /// Settings editor draft for hold-to-dictate (not live until successful Apply).
    pub(crate) hotkey_draft_push_to_talk: String,
    pub(crate) tray: Option<TrayHandle>,
    pub(crate) tray_error: Option<String>,
    pub(crate) tray_tried: bool,
    pub(crate) tray_retry_at: Option<Instant>,
    pub(crate) hard_quit_armed: bool,
    pub(crate) exit_prompt: ExitPromptState,
    pub(crate) mic_sources: Vec<PulseSource>,
    pub(crate) mic_list_error: Option<String>,
    pub(crate) mic_source: String,
    pub(crate) record_level: f32,
    pub(crate) main_tab: MainTab,
    pub(crate) theme_applied: bool,
    /// Resolve `--hidden` only after the first tray creation attempt.
    pub(crate) start_hidden_pending: bool,
    /// Snapshot of work-tab prefs for autosave dirty detection.
    pub(crate) last_saved_prefs: PrefsSnapshot,
    /// When set, autosave is throttled until this instant after a persist failure.
    pub(crate) autosave_retry_after: Option<Instant>,
}

/// Work-tab preferences that should autosave (hotkeys still need Apply).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefsSnapshot {
    pub stt_language: String,
    pub copy_transcript: bool,
    pub tts_language: String,
    pub tts_tone: String,
    pub read_clipboard: bool,
    pub mic_source: String,
    pub stt_model: String,
}

impl YapperApp {
    /// Construct app state. Must not spawn Python workers or block on tone IPC.
    pub(crate) fn new(cc: &eframe::CreationContext<'_>, start_hidden: bool) -> Self {
        let mut cfg = Config::load_or_default().unwrap_or_default();
        cfg.paths.python_root = resolve_python_root(&cfg).to_string_lossy().into();
        cfg.paths.python_bin = resolve_python_bin(&cfg);

        // Immediate fallback tones — async ListTones refreshes in background.
        let tones = fallback_tones();

        let jobs = JobHub::start(cfg.clone());
        jobs.send(JobCmd::ListTones);

        let (hotkeys, hotkey_error) =
            match HotkeyHub::register(&cfg.hotkeys.read_aloud, &cfg.hotkeys.push_to_talk) {
                Ok(h) => (Some(h), None),
                Err(e) => (None, Some(format!("hotkey grab failed: {e:#}"))),
            };

        let stt_model = cfg.stt.model.clone();
        let tts_tone = cfg.tts.tone.clone();
        let tts_language = cfg.tts.language.clone();
        let stt_language = cfg.stt.language.clone();
        let copy_transcript = cfg.stt.copy_transcript;
        let read_clipboard = cfg.read_aloud.source == "clipboard";
        let mic_source = cfg.audio.mic_source.clone();

        let _ = cc;

        let last_saved_prefs = PrefsSnapshot {
            stt_language: stt_language.clone(),
            copy_transcript,
            tts_language: tts_language.clone(),
            tts_tone: tts_tone.clone(),
            read_clipboard,
            mic_source: mic_source.clone(),
            stt_model: stt_model.clone(),
        };

        let hotkey_draft_read_aloud = cfg.hotkeys.read_aloud.clone();
        let hotkey_draft_push_to_talk = cfg.hotkeys.push_to_talk.clone();

        let mut app = Self {
            cfg,
            jobs,
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
            tts: TtsController::default(),
            tts_last_full_path: None,
            stt_loaded: false,
            stt_model_id: None,
            tts_loaded: false,
            tts_model_id: None,
            stt_loading: false,
            tts_loading: false,
            pending_transcribe: None,
            pending_speak: None,
            recording_intent: RecordingIntent::Idle,
            live_stt_job: None,
            next_stt_job_id: 1,
            hotkeys,
            hotkey_error,
            hotkey_capture: None,
            hotkey_draft_read_aloud,
            hotkey_draft_push_to_talk,
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
            main_tab: super::parse_start_tab_env().unwrap_or(MainTab::Dictate),
            theme_applied: false,
            start_hidden_pending: start_hidden,
            last_saved_prefs,
            autosave_retry_after: None,
        };
        app.refresh_mic_sources();
        app
    }

    pub(crate) fn stt_ready_for_selected_model(&self) -> bool {
        stt_ready_for_selected(
            self.stt_loaded,
            self.stt_model_id.as_deref(),
            self.stt_model.as_str(),
        )
    }

    pub(crate) fn stt_status_label(&self) -> String {
        if self.stt_loading {
            return format!("STT … loading {}", self.stt_model);
        }
        load_status_label("STT", self.stt_loaded, self.stt_model_id.as_deref())
    }

    pub(crate) fn tts_status_label(&self) -> String {
        if self.tts_loading {
            return "TTS … loading".into();
        }
        load_status_label("TTS", self.tts_loaded, self.tts_model_id.as_deref())
    }

    pub(crate) fn dictation_chip(&self) -> String {
        dictation_chip_label(
            self.stt_loading,
            self.stt_loaded,
            self.stt_model_id.as_deref(),
            self.stt_model.as_str(),
        )
    }

    pub(crate) fn voice_chip(&self) -> String {
        voice_chip_label(
            self.tts_loading,
            self.tts_loaded,
            self.tts_model_id.as_deref(),
        )
    }

    /// True when TTS synth/playback should be treated as busy (Stop/Restart).
    pub(crate) fn tts_busy(&self) -> bool {
        self.tts.active_job.is_some()
            || self.tts.synth_in_flight
            || self.pending_speak.is_some()
            || matches!(
                self.transport.status(),
                crate::transport::TransportStatus::Playing
                    | crate::transport::TransportStatus::Paused
                    | crate::transport::TransportStatus::Buffering
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

    pub(crate) fn current_prefs(&self) -> PrefsSnapshot {
        PrefsSnapshot {
            stt_language: self.stt_language.clone(),
            copy_transcript: self.copy_transcript,
            tts_language: self.tts_language.clone(),
            tts_tone: self.tts_tone.clone(),
            read_clipboard: self.read_clipboard,
            mic_source: self.mic_source.clone(),
            stt_model: self.stt_model.clone(),
        }
    }

    pub(crate) fn prefs_dirty(&self) -> bool {
        self.current_prefs() != self.last_saved_prefs
    }

    /// Autosave low-risk work-tab prefs when dirty (throttled after save failure).
    pub(crate) fn autosave_prefs_if_dirty(&mut self) {
        if !self.prefs_dirty() {
            return;
        }
        if let Some(until) = self.autosave_retry_after {
            if Instant::now() < until {
                return;
            }
        }
        self.persist();
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
        // Hotkeys: only live applied specs. Drafts never merge here — Apply is
        // the sole path that commits drafts into cfg.hotkeys after a successful grab.
        self.cfg.hotkeys = crate::hotkey_apply::hotkeys_persist_payload(
            &self.cfg.hotkeys,
            &self.hotkey_draft_read_aloud,
            &self.hotkey_draft_push_to_talk,
        );
        match self.cfg.save_default_location() {
            Ok(()) => {
                self.last_saved_prefs = self.current_prefs();
                self.autosave_retry_after = None;
            }
            Err(e) => {
                self.status = format!("config save failed: {e}");
                self.autosave_retry_after = Some(Instant::now() + AUTOSAVE_FAIL_BACKOFF);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::stt_ready_for_selected;

    #[test]
    fn stt_selected_model_mismatch_not_ready() {
        // Mirrors UI fields: loaded small, selected medium → must reload.
        assert!(!stt_ready_for_selected(true, Some("small"), "medium"));
        assert!(stt_ready_for_selected(true, Some("medium"), "medium"));
    }

    #[test]
    fn fallback_tones_used_without_worker() {
        let tones = fallback_tones();
        assert!(!tones.is_empty());
        assert!(tones.iter().any(|t| t == "neutral"));
    }

    #[test]
    fn autosave_backoff_constant_is_positive() {
        assert!(AUTOSAVE_FAIL_BACKOFF.as_secs() >= 5);
    }

    /// GUI Record/Stop: panel + optional copy only — never insert at cursor.
    #[test]
    fn gui_record_completion_does_not_request_insert() {
        let intent = RecordingIntent::GuiPanel;
        assert!(!intent.wants_insert_at_cursor());
        let follow = follow_up_after_transcribe(intent, "hello from mic");
        assert_eq!(follow, TranscribeFollowUp::PanelOnly);
        assert_eq!(status_after_transcribe_success(follow), "transcribed");
        // Copy toggle is orthogonal: panel-only whether text empty or not.
        assert_eq!(
            follow_up_after_transcribe(RecordingIntent::GuiPanel, ""),
            TranscribeFollowUp::PanelOnly
        );
    }

    /// Global hold-to-dictate hotkey PTT: insert at cursor on non-empty text.
    #[test]
    fn hotkey_ptt_completion_does_request_insert() {
        let intent = RecordingIntent::HotkeyInsert;
        assert!(intent.wants_insert_at_cursor());
        let follow = follow_up_after_transcribe(intent, "paste me");
        assert_eq!(follow, TranscribeFollowUp::InsertAtCursor);
        assert_eq!(
            status_after_transcribe_success(follow),
            "inserted transcript"
        );
        // Empty transcription must not paste.
        assert_eq!(
            follow_up_after_transcribe(RecordingIntent::HotkeyInsert, ""),
            TranscribeFollowUp::PanelOnly
        );
    }

    /// Transcribe file… and any Idle/non-hotkey path never insert.
    #[test]
    fn file_transcribe_does_not_insert() {
        let intent = RecordingIntent::Idle;
        assert!(!intent.wants_insert_at_cursor());
        let follow = follow_up_after_transcribe(intent, "from wav file");
        assert_eq!(follow, TranscribeFollowUp::PanelOnly);
        assert_eq!(status_after_transcribe_success(follow), "transcribed");
    }

    #[test]
    fn recording_labels_distinguish_gui_panel_vs_hotkey_ptt() {
        assert!(recording_status_line(RecordingIntent::GuiPanel, "mic").contains("transcript"));
        assert!(recording_status_line(RecordingIntent::HotkeyInsert, "mic").contains("hold-to-dictate"));
        assert_eq!(recording_card_label(RecordingIntent::GuiPanel), "Recording");
        assert_eq!(
            recording_card_label(RecordingIntent::HotkeyInsert),
            "Hold-to-dictate"
        );
    }
}
