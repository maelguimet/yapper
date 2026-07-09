//! Mic capture, hold-to-talk, GUI record, and read-aloud entrypoints.

use super::super::state::{recording_status_line, RecordingIntent};
use super::super::YapperApp;
use crate::audio::{start_recording, stop_recording, temp_wav_path};
use crate::x11util::{self, ClipboardSel};

impl YapperApp {
    pub(crate) fn read_aloud(&mut self) {
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

    /// Start mic capture with an explicit intent (GUI panel vs hotkey insert).
    /// Last writer wins if both paths race; concurrent multi-session is out of scope.
    pub(crate) fn begin_recording(&mut self, intent: RecordingIntent) {
        if self.recording.is_some() {
            return;
        }
        let path = temp_wav_path("rec");
        let source = self.mic_source.clone();
        match start_recording(&path, &source) {
            Ok(session) => {
                self.recording = Some(session);
                self.recording_intent = intent;
                self.record_level = 0.0;
                self.status = recording_status_line(intent, &self.active_mic_label());
            }
            Err(e) => {
                self.recording_intent = RecordingIntent::Idle;
                self.status = format!("record error: {e:#}");
            }
        }
    }

    /// Stop mic and queue transcribe; keeps [`YapperApp::recording_intent`] set at start
    /// so `Transcribed` knows whether to paste. Short/failed recordings clear intent.
    pub(crate) fn end_recording(&mut self) {
        if let Some(session) = self.recording.take() {
            self.record_level = 0.0;
            match stop_recording(session) {
                Ok(path) => {
                    if path.is_file() && path.metadata().map(|m| m.len() > 1000).unwrap_or(false) {
                        // Intent already set at begin_recording (GUI vs hotkey).
                        self.do_transcribe_file(path);
                    } else {
                        self.recording_intent = RecordingIntent::Idle;
                        self.status = "recording too short / empty".into();
                    }
                }
                Err(e) => {
                    self.recording_intent = RecordingIntent::Idle;
                    self.status = format!("record stop error: {e:#}");
                }
            }
        }
    }

    /// Global hold-to-dictate hotkey press — insert at cursor after transcribe.
    pub(crate) fn ptt_press(&mut self) {
        self.begin_recording(RecordingIntent::HotkeyInsert);
    }

    /// Global hold-to-dictate hotkey release.
    pub(crate) fn ptt_release(&mut self) {
        self.end_recording();
    }

    /// Manual Dictate Record — transcript panel only (never paste).
    pub(crate) fn gui_record_press(&mut self) {
        self.begin_recording(RecordingIntent::GuiPanel);
    }

    /// Manual Dictate Stop and transcribe — panel only.
    pub(crate) fn gui_record_release(&mut self) {
        self.end_recording();
    }

    pub(crate) fn poll_record_level(&mut self) {
        if let Some(session) = self.recording.as_ref() {
            if let Some(level) = session.level_01() {
                self.record_level = self.record_level * 0.4 + level * 0.6;
            }
        }
    }
}
