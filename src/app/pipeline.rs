//! YapperApp pipeline: async load/speak/transcribe via JobHub; transport pump.

use super::messages::{
    apply_tts_synth_timeout, is_live_tts_job, should_reload_tts_after_live_synth_failure,
    AppMsg, JobCmd, SynthTimeoutDisposition, TtsTimeoutUiState,
};
use super::state::{
    follow_up_after_transcribe, recording_status_line, status_after_transcribe_success,
    RecordingIntent, TranscribeFollowUp,
};
use super::YapperApp;
use crate::audio::{start_recording, stop_recording, temp_wav_path};
use crate::policy::Role;
use crate::segment::{estimate_segment_count, split_for_tts};
use crate::textprep::sanitize_for_tts;
use crate::transport::TransportStatus;
use crate::ui::{
    chunk_paths_retained_for_replay, chunk_paths_to_remove, resolve_replay_path,
    speak_restart_needs_oob_kill, transport_status_line,
};
use crate::wavutil::concat_wav_files;
use crate::x11util::{self, ClipboardSel};
use std::path::PathBuf;

impl YapperApp {
    pub(crate) fn cancel_tts_pipeline(&mut self) {
        let had_in_flight = self.tts.synth_in_flight;
        self.tts.cancel();
        self.pending_speak = None;
        self.transport.set_pending_queue(false);
        self.transport.stop();
        if had_in_flight {
            // Mid-generate cannot cancel cooperatively. Out-of-band SIGKILL so we
            // do not wait for the serial job_loop stuck inside synthesize.
            let killed = self.jobs.kill_tts_now();
            // Job thread cleans WorkerManager/policy once the killed request returns.
            self.jobs.send(JobCmd::CancelTtsWorker);
            self.tts_loaded = false;
            self.tts_model_id = None;
            if killed {
                self.status = "playback stopped (synth cancelled)".into();
            }
        }
        self.cleanup_chunk_temps_keeping_replay();
    }

    /// Delete intermediate chunk temps; retain `tts_last_full_path` on disk.
    pub(crate) fn cleanup_chunk_temps_keeping_replay(&mut self) {
        let keep = self
            .tts_last_full_path
            .clone()
            .or_else(|| self.transport.machine().last_path.clone())
            .filter(|p| p.is_file());
        let original = std::mem::take(&mut self.tts.chunk_paths);
        for p in chunk_paths_to_remove(&original, keep.as_deref()) {
            let _ = std::fs::remove_file(p);
        }
        let mut retained = chunk_paths_retained_for_replay(&original, keep.as_deref());
        if let Some(ref k) = keep {
            if !retained.iter().any(|p| p == k) {
                retained.push(k.clone());
            }
        }
        self.tts.chunk_paths = retained;
        self.tts_last_full_path = keep.clone();
        if let Some(p) = keep {
            self.transport.remember_path(p);
        }
    }

    pub(crate) fn discard_all_tts_audio(&mut self) {
        self.tts.cancel();
        self.pending_speak = None;
        self.transport.set_pending_queue(false);
        self.transport.stop();
        self.transport.clear_last_path();
        self.tts_last_full_path = None;
        for p in self.tts.chunk_paths.drain(..) {
            let _ = std::fs::remove_file(p);
        }
    }

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

    pub(crate) fn load_stt(&mut self) {
        if self.stt_loading {
            return;
        }
        // Always load the *selected* model (may differ from currently loaded).
        self.stt_loading = true;
        self.status = format!("loading dictation model {}…", self.stt_model);
        self.jobs.send(JobCmd::LoadStt {
            model: self.stt_model.clone(),
            device: "cuda".into(),
        });
    }

    pub(crate) fn unload_stt(&mut self) {
        self.jobs.send(JobCmd::Unload { role: Role::Stt });
        self.status = "unloading dictation model…".into();
    }

    pub(crate) fn load_tts(&mut self) {
        if self.tts_loading {
            return;
        }
        if self.tts_loaded && !self.tts_loading {
            return;
        }
        self.tts_loading = true;
        self.status = "loading voice model…".into();
        self.jobs.send(JobCmd::LoadTts {
            device: "cuda".into(),
        });
    }

    pub(crate) fn unload_tts(&mut self) {
        self.discard_all_tts_audio();
        self.jobs.send(JobCmd::Unload { role: Role::Tts });
        self.status = "unloading voice model…".into();
    }

    pub(crate) fn do_transcribe_file(&mut self, path: PathBuf) {
        // Honor Settings selector: if wrong size is loaded, reload first.
        if !self.stt_ready_for_selected_model() {
            self.pending_transcribe = Some(path);
            self.load_stt();
            self.status = format!("loading dictation model {}…", self.stt_model);
            return;
        }
        self.status = "transcribing…".into();
        self.jobs.send(JobCmd::Transcribe {
            path,
            language: self.stt_language.clone(),
        });
    }

    pub(crate) fn do_speak(&mut self, text: &str) {
        let text = sanitize_for_tts(text);
        if text.is_empty() {
            self.status = "nothing to speak".into();
            return;
        }
        if !self.tts_loaded {
            self.pending_speak = Some(text);
            self.load_tts();
            self.status = "loading voice model…".into();
            return;
        }
        self.start_speak_job(text);
    }

    fn start_speak_job(&mut self, text: String) {
        // Cancel any prior monologue; start chunked pipeline.
        let had_in_flight = self.tts.synth_in_flight;
        self.tts.cancel();
        self.transport.set_pending_queue(false);
        self.transport.stop();
        if speak_restart_needs_oob_kill(had_in_flight) {
            // Same immediate path as Stop — do not queue Cancel behind blocking Synthesize.
            let _ = self.jobs.kill_tts_now();
            self.jobs.send(JobCmd::CancelTtsWorker);
            self.tts_loaded = false;
            self.tts_model_id = None;
            self.pending_speak = Some(text);
            self.status = "restarting voice…".into();
            self.cleanup_chunk_temps_keeping_replay();
            self.load_tts();
            return;
        }

        let segments = split_for_tts(&text);
        if segments.is_empty() {
            self.status = "nothing to speak".into();
            return;
        }
        let est = estimate_segment_count(&text);
        let _ = est;
        let job_id = self.tts.begin_job(segments);
        self.transport
            .set_pending_queue(self.tts.pending.len() > 1);
        self.status = format!("synthesizing 1/{}…", self.tts.total);
        let _ = job_id;
        self.pump_tts_synth();
    }

    /// Request background synth for next pending segment if prebuffer room.
    pub(crate) fn pump_tts_synth(&mut self) {
        if !self.tts_loaded || !self.tts.should_request_synth() {
            return;
        }
        let Some(job_id) = self.tts.active_job else {
            return;
        };
        let Some(seg) = self.tts.peek_pending().cloned() else {
            return;
        };
        self.tts.mark_synth_started();
        let out = temp_wav_path("tts-chunk");
        self.status = format!(
            "synthesizing {}/{}…",
            seg.index + 1,
            self.tts.total
        );
        self.jobs.send(JobCmd::Synthesize {
            job_id,
            index: seg.index,
            total: self.tts.total,
            text: seg.text,
            language: self.tts_language.clone(),
            tone: self.tts_tone.clone(),
            voice: self.cfg.tts.voice.clone(),
            out_path: out,
        });
    }

    /// Drain job messages (UI thread only).
    pub(crate) fn drain_job_messages(&mut self) {
        for msg in self.jobs.drain() {
            self.handle_app_msg(msg);
        }
    }

    fn handle_app_msg(&mut self, msg: AppMsg) {
        match msg {
            AppMsg::ModelStatus {
                stt_loaded,
                stt_model,
                tts_loaded,
                tts_model,
            } => {
                self.stt_loaded = stt_loaded;
                self.stt_model_id = stt_model;
                self.tts_loaded = tts_loaded;
                self.tts_model_id = tts_model;
            }
            AppMsg::SttLoaded { model, result } => {
                self.stt_loading = false;
                match result {
                    Ok(()) => {
                        self.stt_loaded = true;
                        self.stt_model_id = Some(model.clone());
                        self.status = format!("STT {model} loaded");
                        if let Some(path) = self.pending_transcribe.take() {
                            self.do_transcribe_file(path);
                        }
                    }
                    Err(e) => {
                        self.status = format!("STT load error: {e}");
                        self.pending_transcribe = None;
                        self.recording_intent = RecordingIntent::Idle;
                    }
                }
            }
            AppMsg::TtsLoaded { result } => {
                self.tts_loading = false;
                match result {
                    Ok(()) => {
                        self.tts_loaded = true;
                        self.tts_model_id = Some("chatterbox-multilingual".into());
                        self.status = "TTS loaded".into();
                        if let Some(text) = self.pending_speak.take() {
                            self.start_speak_job(text);
                        } else {
                            self.pump_tts_synth();
                        }
                    }
                    Err(e) => {
                        self.status = format!("TTS load error: {e}");
                        self.pending_speak = None;
                    }
                }
            }
            AppMsg::Unloaded { role, result } => {
                match result {
                    Ok(()) => {
                        self.status = match role {
                            Some(Role::Stt) => "STT unloaded".into(),
                            Some(Role::Tts) => "TTS unloaded".into(),
                            None => "all unloaded".into(),
                        };
                    }
                    Err(e) => self.status = format!("unload error: {e}"),
                }
            }
            AppMsg::Transcribed { text } => {
                // Take intent once so a following success cannot re-insert.
                let intent = std::mem::replace(&mut self.recording_intent, RecordingIntent::Idle);
                self.transcript = text.clone();
                let follow = follow_up_after_transcribe(intent, &text);
                if follow == TranscribeFollowUp::InsertAtCursor {
                    // Insert owns final CLIPBOARD: paste via clipboard+ctrl+v (no Enter);
                    // keep transcript only when Copy transcript is on (else restore prior).
                    match x11util::insert_transcript_at_cursor(&text, self.copy_transcript) {
                        Ok(()) => {
                            self.status = status_after_transcribe_success(follow).into();
                        }
                        Err(e) => {
                            self.status = format!("transcribed; paste failed: {e}");
                        }
                    }
                    return;
                }
                // Panel-only path (GUI Dictate / file): optional copy, never paste.
                if self.copy_transcript {
                    if let Err(e) = x11util::write_clipboard(&text) {
                        self.status = format!("transcribed; clipboard failed: {e}");
                        return;
                    }
                }
                self.status = status_after_transcribe_success(follow).into();
            }
            AppMsg::TranscribeFailed { error, path } => {
                self.recording_intent = RecordingIntent::Idle;
                self.status = format!("transcribe error: {error} ({})", path.display());
            }
            AppMsg::TtsChunkReady {
                job_id,
                index,
                text,
                path,
                duration_secs,
            } => {
                if !is_live_tts_job(self.tts.active_job, job_id) {
                    let _ = std::fs::remove_file(&path);
                    return;
                }
                if !self.tts.on_chunk_ready(job_id, index, text, path, duration_secs) {
                    return;
                }
                self.transport
                    .set_pending_queue(!self.tts.pending.is_empty() || !self.tts.ready.is_empty());
                self.try_play_next_ready();
                self.pump_tts_synth();
                self.update_speak_status();
            }
            AppMsg::TtsChunkFailed {
                job_id,
                index,
                error,
            } => {
                if !is_live_tts_job(self.tts.active_job, job_id) {
                    return;
                }
                let retry = self.tts.on_chunk_failed(job_id, index);
                if retry.is_some() {
                    self.status = format!("segment {} failed, retrying… ({error})", index + 1);
                } else {
                    self.status = format!("segment {} skipped: {error}", index + 1);
                }
                // If worker was killed on timeout, tts_loaded may be false — reload.
                if should_reload_tts_after_live_synth_failure(
                    self.tts_loaded,
                    self.tts.active_job,
                ) {
                    self.load_tts();
                } else {
                    self.pump_tts_synth();
                }
                self.try_play_next_ready();
            }
            AppMsg::WorkerTimedOut {
                role,
                op,
                error,
                job_id,
            } => {
                match role {
                    Role::Stt => {
                        self.stt_loaded = false;
                        self.stt_model_id = None;
                        self.stt_loading = false;
                        self.status = format!("{role:?} {op}: {error}");
                    }
                    Role::Tts => {
                        // Job-scoped filter: Stop/Restart kills must not overwrite
                        // "playback stopped" / "restarting voice…" status.
                        let mut ui = TtsTimeoutUiState {
                            status: self.status.clone(),
                            tts_loaded: self.tts_loaded,
                            tts_model_id: self.tts_model_id.clone(),
                            tts_loading: self.tts_loading,
                        };
                        let d = apply_tts_synth_timeout(
                            &mut ui,
                            self.tts.active_job,
                            job_id,
                            &op,
                            &error,
                        );
                        if d == SynthTimeoutDisposition::ReportAndClearBadge {
                            self.status = ui.status;
                            self.tts_loaded = ui.tts_loaded;
                            self.tts_model_id = ui.tts_model_id;
                            self.tts_loading = ui.tts_loading;
                        }
                        // SilentCleanup: leave status + loading flags alone;
                        // ModelStatus / CancelTtsWorker already reflect worker death.
                    }
                }
            }
            AppMsg::TonesListed { tones } => {
                if !tones.is_empty() {
                    self.tones = tones;
                    // Keep current selection if still present; else first tone.
                    if !self.tones.iter().any(|t| t == &self.tts_tone) {
                        if let Some(first) = self.tones.first() {
                            self.tts_tone = first.clone();
                        }
                    }
                }
            }
        }
    }

    fn try_play_next_ready(&mut self) {
        let status = self.transport.status();
        let can_start = matches!(
            status,
            TransportStatus::Idle | TransportStatus::Buffering
        );
        // While speaking/paused on mpv IPC, append ready chunks to the live playlist
        // instead of waiting for end-of-file + respawn.
        let can_append = matches!(status, TransportStatus::Playing | TransportStatus::Paused)
            && self.transport.can_append();
        if !can_start && !can_append {
            return;
        }
        if self.tts.ready.is_empty() {
            if can_start
                && self.tts.pending.is_empty()
                && !self.tts.synth_in_flight
            {
                self.transport.set_pending_queue(false);
                self.finalize_tts_job_if_done();
            }
            return;
        }
        // Drain ready into one persistent player session (start once, then append).
        while !self.tts.ready.is_empty() {
            let still_start = matches!(
                self.transport.status(),
                TransportStatus::Idle | TransportStatus::Buffering
            );
            let still_append = matches!(
                self.transport.status(),
                TransportStatus::Playing | TransportStatus::Paused
            ) && self.transport.can_append();
            if !still_start && !still_append {
                break;
            }
            let Some(chunk) = self.tts.pop_ready() else {
                break;
            };
            self.transport.set_pending_queue(
                !self.tts.pending.is_empty()
                    || !self.tts.ready.is_empty()
                    || self.tts.synth_in_flight,
            );
            if let Err(e) = self.transport.enqueue_or_play(&chunk.path) {
                self.status = format!("playback error: {e:#}");
                return;
            }
        }
        self.update_speak_status();
        // Keep prebuffer full while playing.
        self.pump_tts_synth();
    }

    fn update_speak_status(&mut self) {
        let transport_idle = matches!(self.transport.status(), TransportStatus::Idle);
        let time = self.transport.machine().format_time_label();
        let mut line = transport_status_line(
            self.tts.active_job.is_some(),
            self.tts.playing_index,
            self.tts.total,
            transport_idle,
            &time,
            self.tts.synth_in_flight,
        );
        // Keep controller progress_label wired for N/M when playing_index lags.
        let prog = self.tts.progress_label();
        if !prog.is_empty() && self.tts.active_job.is_some() && !line.contains(&prog) {
            line = format!("{line} · {prog}");
        }
        self.status = line;
    }

    fn finalize_tts_job_if_done(&mut self) {
        if self.tts.active_job.is_none() {
            return;
        }
        if self.tts.has_work() {
            return;
        }
        if !matches!(self.transport.status(), TransportStatus::Idle) {
            return;
        }
        // Build full-utterance WAV for Replay.
        let paths = self.tts.chunk_paths.clone();
        if !paths.is_empty() {
            let out = temp_wav_path("tts-full");
            match concat_wav_files(&paths, &out) {
                Ok(()) => {
                    // Drop intermediate chunks; keep full only.
                    for p in &paths {
                        if p != &out {
                            let _ = std::fs::remove_file(p);
                        }
                    }
                    self.tts.chunk_paths = vec![out.clone()];
                    self.tts_last_full_path = Some(out.clone());
                    self.transport.remember_path(out);
                }
                Err(e) => {
                    // Fallback: last chunk only.
                    if let Some(last) = paths.last() {
                        self.tts_last_full_path = Some(last.clone());
                        self.transport.remember_path(last.clone());
                    }
                    self.status = format!("ready (full concat failed: {e:#})");
                    self.tts.active_job = None;
                    self.tts.total = 0;
                    return;
                }
            }
        }
        self.tts.active_job = None;
        self.tts.total = 0;
        self.status = "ready".into();
    }

    pub(crate) fn poll_transport(&mut self) {
        self.transport.tick();
        // Idle/buffering: start; playing/paused: append ready chunks to live playlist.
        match self.transport.status() {
            TransportStatus::Idle | TransportStatus::Buffering => {
                self.try_play_next_ready();
            }
            TransportStatus::Playing => {
                self.update_speak_status();
                self.pump_tts_synth();
                self.try_play_next_ready();
            }
            TransportStatus::Paused => {
                self.status = format!(
                    "paused ({})",
                    self.transport.machine().format_time_label()
                );
                self.try_play_next_ready();
            }
        }
        if matches!(self.transport.status(), TransportStatus::Idle)
            && !self.tts.has_work()
            && self.tts.active_job.is_some()
        {
            self.finalize_tts_job_if_done();
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

    /// Stop mic and queue transcribe; keeps [`Self::recording_intent`] set at start
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
