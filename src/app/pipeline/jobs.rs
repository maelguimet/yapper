//! JobHub message drain and AppMsg handlers.

use super::super::messages::{
    apply_tts_synth_timeout, is_live_tts_job, should_reload_tts_after_live_synth_failure, AppMsg,
    SynthTimeoutDisposition, TtsTimeoutUiState,
};
use super::super::state::{
    follow_up_after_transcribe, status_after_transcribe_success, RecordingIntent,
    TranscribeFollowUp,
};
use super::super::YapperApp;
use crate::policy::Role;
use crate::x11util;

impl YapperApp {
    /// Drain job messages (UI thread only).
    pub(crate) fn drain_job_messages(&mut self) {
        for msg in self.jobs.drain() {
            self.handle_app_msg(msg);
        }
    }

    pub(crate) fn handle_app_msg(&mut self, msg: AppMsg) {
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
            AppMsg::Unloaded { role, result } => match result {
                Ok(()) => {
                    self.status = match role {
                        Some(Role::Stt) => "STT unloaded".into(),
                        Some(Role::Tts) => "TTS unloaded".into(),
                        None => "all unloaded".into(),
                    };
                }
                Err(e) => self.status = format!("unload error: {e}"),
            },
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
}
