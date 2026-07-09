//! Model load/unload and file-transcribe entrypoints.

use super::super::messages::JobCmd;
use super::super::YapperApp;
use crate::policy::Role;
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
}
