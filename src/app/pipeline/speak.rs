//! TTS cancel/cleanup and speak-job start / synth pump.

use super::super::messages::JobCmd;
use super::super::YapperApp;
use crate::audio::temp_wav_path;
use crate::segment::split_for_tts;
use crate::textprep::sanitize_for_tts;
use crate::ui::{
    chunk_paths_retained_for_replay, chunk_paths_to_remove, resolve_replay_path,
    speak_restart_needs_oob_kill,
};

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

    pub(crate) fn do_speak(&mut self, text: &str) {
        let text = sanitize_for_tts(text);
        if text.is_empty() {
            self.status = "nothing to speak".into();
            return;
        }
        let voices = std::path::Path::new(self.cfg.models.voices_dir.trim());
        if !crate::ui::neutral_voice_present(voices) {
            self.status = crate::ui::voice_missing_guidance(false)
                .unwrap_or("missing eve_neutral.wav")
                .into();
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

    pub(crate) fn start_speak_job(&mut self, text: String) {
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
}
