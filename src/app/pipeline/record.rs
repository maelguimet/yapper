//! Mic capture, hold-to-talk, GUI record, and read-aloud entrypoints.

use super::super::messages::{release_matches_open_recording, RecordingIntent};
use super::super::state::recording_status_line;
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
    /// Refuses if a session is already open (does not steal another path's mic).
    pub(crate) fn begin_recording(&mut self, intent: RecordingIntent) {
        if self.recording.is_some() {
            return;
        }
        if !intent.is_open_mic() {
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

    /// Stop mic and queue transcribe for `release` only when it matches the open session.
    /// Mismatched GUI/PTT releases are no-ops so one path cannot stop the other.
    pub(crate) fn end_recording(&mut self, release: RecordingIntent) {
        if !release_matches_open_recording(self.recording_intent, release) {
            return;
        }
        let Some(session) = self.recording.take() else {
            self.recording_intent = RecordingIntent::Idle;
            return;
        };
        let intent = self.recording_intent;
        self.record_level = 0.0;
        match stop_recording(session) {
            Ok(path) => {
                if path.is_file() && path.metadata().map(|m| m.len() > 1000).unwrap_or(false) {
                    // Intent travels with the job; clear open-mic state before queue.
                    self.recording_intent = RecordingIntent::Idle;
                    self.do_transcribe_file(path, intent);
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

    /// Global hold-to-dictate hotkey press — insert at cursor after transcribe.
    pub(crate) fn ptt_press(&mut self) {
        self.begin_recording(RecordingIntent::HotkeyInsert);
    }

    /// Global hold-to-dictate hotkey release (ignored if GUI owns the mic).
    pub(crate) fn ptt_release(&mut self) {
        self.end_recording(RecordingIntent::HotkeyInsert);
    }

    /// Manual Dictate Record — transcript panel only (never paste).
    pub(crate) fn gui_record_press(&mut self) {
        self.begin_recording(RecordingIntent::GuiPanel);
    }

    /// Manual Dictate Stop (ignored if hotkey PTT owns the mic).
    pub(crate) fn gui_record_release(&mut self) {
        self.end_recording(RecordingIntent::GuiPanel);
    }

    pub(crate) fn poll_record_level(&mut self) {
        if let Some(session) = self.recording.as_ref() {
            if let Some(level) = session.level_01() {
                self.record_level = self.record_level * 0.4 + level * 0.6;
            }
        }
    }
}
