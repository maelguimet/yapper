//! UI ↔ background job messages (pure types, unit-testable).

use crate::policy::Role;
use std::path::PathBuf;

/// Commands the UI posts to the background job thread.
#[derive(Debug, Clone)]
pub enum JobCmd {
    LoadStt {
        model: String,
        device: String,
    },
    LoadTts {
        device: String,
    },
    Unload {
        role: Role,
    },
    UnloadAll,
    Transcribe {
        path: PathBuf,
        language: String,
    },
    /// Synthesize one TTS segment. Stale results are filtered by `job_id`.
    Synthesize {
        job_id: u64,
        index: usize,
        total: usize,
        text: String,
        language: String,
        tone: String,
        voice: String,
        out_path: PathBuf,
    },
    /// Kill TTS worker mid-generate (Stop / cancel). Next synth restarts it.
    CancelTtsWorker,
    /// Lightweight tone list (no model load required if worker can list offline).
    ListTones,
    Shutdown,
}

/// Events the job thread posts back to the UI.
#[derive(Debug, Clone)]
pub enum AppMsg {
    SttLoaded {
        model: String,
        result: Result<(), String>,
    },
    TtsLoaded {
        result: Result<(), String>,
    },
    Unloaded {
        role: Option<Role>,
        result: Result<(), String>,
    },
    Transcribed {
        text: String,
    },
    TranscribeFailed {
        error: String,
        path: PathBuf,
    },
    TtsChunkReady {
        job_id: u64,
        index: usize,
        text: String,
        path: PathBuf,
        duration_secs: f64,
    },
    TtsChunkFailed {
        job_id: u64,
        index: usize,
        error: String,
    },
    WorkerTimedOut {
        role: Role,
        op: String,
        error: String,
    },
    /// Policy snapshot after load/unload so UI badges stay accurate.
    ModelStatus {
        stt_loaded: bool,
        stt_model: Option<String>,
        tts_loaded: bool,
        tts_model: Option<String>,
    },
    /// Async tone list refresh (fallback tones used until this arrives).
    TonesListed {
        tones: Vec<String>,
    },
}

/// Pure filter: should the UI accept this TTS chunk for the active job?
pub fn is_live_tts_job(active_job_id: Option<u64>, msg_job_id: u64) -> bool {
    active_job_id == Some(msg_job_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_tts_job_id_is_ignored() {
        assert!(!is_live_tts_job(Some(2), 1));
        assert!(!is_live_tts_job(None, 1));
        assert!(is_live_tts_job(Some(3), 3));
    }
}
