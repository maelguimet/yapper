//! Model load/unload and file-transcribe entrypoints.

use super::super::messages::{plan_unload_all, JobCmd, RecordingIntent};
use super::super::YapperApp;
use crate::audio::stop_recording;
use crate::policy::Role;
use crate::ui::speak_restart_needs_oob_kill;
use std::path::PathBuf;

impl YapperApp {
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

    /// Settings **Unload all**: stop playback, OOB-kill workers when needed,
    /// clean chunk temps, then enqueue UnloadAll — same kill/cleanup shape as
    /// Stop / Hard Quit (never a bare UnloadAll alone).
    pub(crate) fn unload_all_models(&mut self) {
        let synth_in_flight = self.tts.synth_in_flight;
        let plan = plan_unload_all(synth_in_flight);
        debug_assert!(plan.stop_playback && plan.cleanup_chunks && plan.send_unload_all);

        // Discard stops transport and clears chunk temps (cleanup_chunks).
        self.discard_all_tts_audio();
        if plan.oob_kill_tts_if_in_flight || speak_restart_needs_oob_kill(synth_in_flight) {
            let _ = self.jobs.kill_tts_now();
            self.jobs.send(JobCmd::CancelTtsWorker);
            self.tts_loaded = false;
            self.tts_model_id = None;
        }
        if plan.oob_kill_all_workers {
            let _ = self.jobs.kill_all_now();
        }
        if let Some(session) = self.recording.take() {
            let _ = stop_recording(session);
        }
        self.recording_intent = RecordingIntent::Idle;
        self.live_stt_job = None;
        self.pending_transcribe = None;
        self.pending_speak = None;
        self.jobs.send(JobCmd::UnloadAll);
        self.status = "unloading all…".into();
    }

    /// Queue a transcribe with a fresh job_id + intent (file pick uses Idle).
    pub(crate) fn do_transcribe_file(&mut self, path: PathBuf, intent: RecordingIntent) {
        // Honor Settings selector: if wrong size is loaded, reload first.
        if !self.stt_ready_for_selected_model() {
            self.pending_transcribe = Some((path, intent));
            self.load_stt();
            self.status = format!("loading dictation model {}…", self.stt_model);
            return;
        }
        let job_id = self.next_stt_job_id;
        self.next_stt_job_id = self.next_stt_job_id.wrapping_add(1).max(1);
        self.live_stt_job = Some(job_id);
        self.status = "transcribing…".into();
        self.jobs.send(JobCmd::Transcribe {
            job_id,
            intent,
            path,
            language: self.stt_language.clone(),
        });
    }
}
