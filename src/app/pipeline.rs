//! YapperApp pipeline: load/unload, speak, record, transport.

use super::YapperApp;
use crate::audio::{start_recording, stop_recording, temp_wav_path};
use crate::policy::Role;
use crate::segment::{estimate_segment_count, split_for_tts};
use crate::textprep::sanitize_for_tts;
use crate::transport::TransportStatus;
use crate::ui::{
    chunk_paths_retained_for_replay, chunk_paths_to_remove, resolve_replay_path,
};
use crate::x11util::{self, ClipboardSel};
use std::path::PathBuf;

impl YapperApp {
    pub(crate) fn cancel_tts_pipeline(&mut self) {
        self.tts_cancel = true;
        self.tts_queue.clear();
        self.tts_queue_total = 0;
        self.transport.set_pending_queue(false);
        self.transport.stop();
        self.cleanup_chunk_temps_keeping_replay();
    }

    /// Delete intermediate chunk temps; retain `tts_last_full_path` on disk.
    pub(crate) fn cleanup_chunk_temps_keeping_replay(&mut self) {
        // Prefer durable last-success; fall back to transport path if set.
        let keep = self
            .tts_last_full_path
            .clone()
            .or_else(|| self.transport.machine().last_path.clone())
            .filter(|p| p.is_file());
        let original = std::mem::take(&mut self.tts_chunk_paths);
        for p in chunk_paths_to_remove(&original, keep.as_deref()) {
            let _ = std::fs::remove_file(p);
        }
        let mut retained = chunk_paths_retained_for_replay(&original, keep.as_deref());
        if let Some(ref k) = keep {
            if !retained.iter().any(|p| p == k) {
                retained.push(k.clone());
            }
        }
        self.tts_chunk_paths = retained;
        self.tts_last_full_path = keep.clone();
        if let Some(p) = keep {
            self.transport.remember_path(p);
        }
    }

    /// Full wipe (quit / unload) — no Replay after this.
    pub(crate) fn discard_all_tts_audio(&mut self) {
        self.tts_cancel = true;
        self.tts_queue.clear();
        self.tts_queue_total = 0;
        self.transport.set_pending_queue(false);
        self.transport.stop();
        self.tts_last_full_path = None;
        for p in self.tts_chunk_paths.drain(..) {
            let _ = std::fs::remove_file(p);
        }
        // Clear transport last_path by stopping without keep — use play of nothing.
        // stop() keeps last_path; force clear by replacing transport machine via stop then
        // dropping the path file already deleted. replay() will then return false.
    }

    /// Replay last successful synth without re-synthesizing (survives Stop).
    pub(crate) fn replay_last(&mut self) -> anyhow::Result<bool> {
        let transport_last = self.transport.machine().last_path.clone();
        let path = resolve_replay_path(
            self.tts_last_full_path.as_deref(),
            transport_last.as_deref(),
        );
        let Some(path) = path else {
            return Ok(false);
        };
        self.tts_last_full_path = Some(path.clone());
        if transport_last.as_deref() == Some(path.as_path()) {
            return self.transport.replay();
        }
        self.transport.remember_path(path);
        self.transport.replay()
    }

    /// Intercept title-bar close / minimize → hide; only hard_quit_armed exits.
    pub(crate) fn load_stt(&mut self) {
        self.status = format!("loading STT {}…", self.stt_model);
        match self
            .workers
            .load(Role::Stt, &self.stt_model, "cuda")
        {
            Ok(()) => self.status = format!("STT {} loaded", self.stt_model),
            Err(e) => self.status = format!("STT load error: {e:#}"),
        }
    }

    pub(crate) fn unload_stt(&mut self) {
        match self.workers.unload(Role::Stt) {
            Ok(()) => self.status = "STT unloaded".into(),
            Err(e) => self.status = format!("STT unload error: {e:#}"),
        }
    }

    pub(crate) fn load_tts(&mut self) {
        self.status = "loading TTS…".into();
        match self
            .workers
            .load(Role::Tts, "chatterbox-multilingual", "cuda")
        {
            Ok(()) => self.status = "TTS loaded".into(),
            Err(e) => self.status = format!("TTS load error: {e:#}"),
        }
    }

    pub(crate) fn unload_tts(&mut self) {
        // Unload frees the model; drop replay audio too (temps not needed).
        self.discard_all_tts_audio();
        match self.workers.unload(Role::Tts) {
            Ok(()) => self.status = "TTS unloaded".into(),
            Err(e) => self.status = format!("TTS unload error: {e:#}"),
        }
    }

    pub(crate) fn do_transcribe_file(&mut self, path: PathBuf) {
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

    pub(crate) fn do_speak(&mut self, text: &str) {
        let text = sanitize_for_tts(text);
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
        let segments = split_for_tts(&text);
        if segments.is_empty() {
            self.status = "nothing to speak".into();
            return;
        }
        // estimate_segment_count is the UI-facing counter; keep it wired.
        let est = estimate_segment_count(&text);
        self.tts_queue_total = est.max(segments.len());
        self.tts_queue = segments;
        self.transport.set_pending_queue(self.tts_queue.len() > 1);
        self.status = format!("synthesizing 1/{}...", self.tts_queue_total);
        self.pump_tts_queue();
    }

    /// Synth next segment if transport is free enough to accept more audio.
    pub(crate) fn pump_tts_queue(&mut self) {
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
                // Promote new last-success; drop previous durable WAV if different.
                if let Some(old) = self.tts_last_full_path.take() {
                    if old != path {
                        let _ = std::fs::remove_file(&old);
                        self.tts_chunk_paths.retain(|p| p != &old);
                    }
                }
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

    pub(crate) fn poll_transport(&mut self) {
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

    pub(crate) fn ptt_press(&mut self) {
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

    pub(crate) fn ptt_release(&mut self) {
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

    pub(crate) fn poll_record_level(&mut self) {
        if let Some(session) = self.recording.as_ref() {
            if let Some(level) = session.level_01() {
                // light smoothing so the bar is readable
                self.record_level = self.record_level * 0.4 + level * 0.6;
            }
        }
    }

}
