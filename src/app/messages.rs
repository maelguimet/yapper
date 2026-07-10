//! UI ↔ background job messages (pure types, unit-testable).

use crate::policy::Role;
use std::path::PathBuf;

/// Why the current (or next completed) mic/file transcribe was started.
///
/// GUI Record fills the Transcript panel only; global hold-to-dictate also
/// pastes at the focused cursor. File pick is always panel-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordingIntent {
    /// No insert pending (idle, or file-transcribe).
    #[default]
    Idle,
    /// Manual Dictate Record/Stop — panel + optional clipboard copy, never paste.
    GuiPanel,
    /// Global hold-to-dictate hotkey — paste at cursor after successful Transcribed.
    HotkeyInsert,
}

impl RecordingIntent {
    /// Only hotkey PTT requests insert-at-cursor after transcription.
    pub fn wants_insert_at_cursor(self) -> bool {
        matches!(self, Self::HotkeyInsert)
    }

    /// True while a mic session owns the hardware for this intent.
    pub fn is_open_mic(self) -> bool {
        matches!(self, Self::GuiPanel | Self::HotkeyInsert)
    }
}

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
    /// Transcribe one audio file. Stale results are filtered by `job_id`.
    Transcribe {
        job_id: u64,
        intent: RecordingIntent,
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
        job_id: u64,
        intent: RecordingIntent,
        text: String,
    },
    TranscribeFailed {
        job_id: u64,
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
    /// Worker killed or timed out mid-op. Synth and STT set `job_id` when known
    /// so Stop/Restart and concurrent sessions ignore stale events.
    WorkerTimedOut {
        role: Role,
        op: String,
        error: String,
        job_id: Option<u64>,
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

/// Pure filter: should the UI accept this STT result for the live transcribe job?
pub fn is_live_stt_job(active_job_id: Option<u64>, msg_job_id: u64) -> bool {
    active_job_id == Some(msg_job_id)
}

/// True when a recording release matches the open mic session (GUI vs PTT).
///
/// Mismatched releases are no-ops so GUI Stop cannot end hotkey PTT and vice versa.
pub fn release_matches_open_recording(
    open: RecordingIntent,
    release: RecordingIntent,
) -> bool {
    open.is_open_mic() && open == release
}

/// Shared Unload-all prep flags (Stop / Hard Quit parity for kill + cleanup).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnloadAllPlan {
    pub stop_playback: bool,
    pub cleanup_chunks: bool,
    pub oob_kill_tts_if_in_flight: bool,
    pub oob_kill_all_workers: bool,
    pub send_unload_all: bool,
}

/// Plan Settings **Unload all** must follow before/with model unload.
///
/// Same stop + OOB-kill + cleanup shape as Stop mid-synth and Hard Quit —
/// never a bare `UnloadAll` enqueue alone while synth/playback is live.
pub fn plan_unload_all(synth_in_flight: bool) -> UnloadAllPlan {
    UnloadAllPlan {
        stop_playback: true,
        cleanup_chunks: true,
        oob_kill_tts_if_in_flight: synth_in_flight,
        oob_kill_all_workers: true,
        send_unload_all: true,
    }
}

/// How the UI should treat a TTS synthesize worker-timeout / kill event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SynthTimeoutDisposition {
    /// Expected Stop/Restart kill or stale job — do not touch status or loading flags.
    SilentCleanup,
    /// Live unexpected failure — clear TTS badge and surface the error.
    ReportAndClearBadge,
}

/// Classify a TTS synthesize `WorkerTimedOut` against the UI's active TTS job.
///
/// - Live `job_id` → report error and clear the voice badge.
/// - Stale/cancelled `job_id` (or no active job after Stop/Restart) → silent cleanup.
/// - Missing `job_id` (legacy): report only while a job is still active.
pub fn classify_tts_synth_timeout(
    active_job_id: Option<u64>,
    msg_job_id: Option<u64>,
) -> SynthTimeoutDisposition {
    match msg_job_id {
        Some(id) if is_live_tts_job(active_job_id, id) => {
            SynthTimeoutDisposition::ReportAndClearBadge
        }
        Some(_) => SynthTimeoutDisposition::SilentCleanup,
        None => {
            if active_job_id.is_some() {
                SynthTimeoutDisposition::ReportAndClearBadge
            } else {
                SynthTimeoutDisposition::SilentCleanup
            }
        }
    }
}

/// Minimal UI fields touched by a TTS synthesize timeout (pure apply path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TtsTimeoutUiState {
    pub status: String,
    pub tts_loaded: bool,
    pub tts_model_id: Option<String>,
    pub tts_loading: bool,
}

/// Apply a TTS synthesize timeout to UI fields using the shipped classifier.
/// Returns the disposition so callers can branch (e.g. skip STT-style handling).
pub fn apply_tts_synth_timeout(
    state: &mut TtsTimeoutUiState,
    active_job_id: Option<u64>,
    msg_job_id: Option<u64>,
    op: &str,
    error: &str,
) -> SynthTimeoutDisposition {
    let disposition = classify_tts_synth_timeout(active_job_id, msg_job_id);
    if disposition == SynthTimeoutDisposition::ReportAndClearBadge {
        state.tts_loaded = false;
        state.tts_model_id = None;
        state.tts_loading = false;
        state.status = format!("Tts {op}: {error}");
    }
    disposition
}

/// True when a live-job synth failure should reload TTS (retry path still active).
pub fn should_reload_tts_after_live_synth_failure(
    tts_loaded: bool,
    active_job_id: Option<u64>,
) -> bool {
    !tts_loaded && active_job_id.is_some()
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

    /// Stop mid-synth: cancel clears active job; killed job's timeout must not
    /// overwrite the stop/cancel status string.
    #[test]
    fn stop_mid_synth_keeps_stop_status() {
        let mut state = TtsTimeoutUiState {
            status: "playback stopped (synth cancelled)".into(),
            tts_loaded: false,
            tts_model_id: None,
            tts_loading: false,
        };
        let killed_job = 7u64;
        // After cancel_tts_pipeline: active_job is None.
        let d = apply_tts_synth_timeout(
            &mut state,
            None,
            Some(killed_job),
            "synthesize",
            "worker exited during synthesize (status=signal: 9) — cancelled or crashed",
        );
        assert_eq!(d, SynthTimeoutDisposition::SilentCleanup);
        assert_eq!(state.status, "playback stopped (synth cancelled)");
        assert!(!state.status.to_ascii_lowercase().contains("cancelled or crashed"));
        assert!(!state.status.contains("worker exited during synthesize"));
        assert!(!state.tts_loaded);
        assert!(!state.tts_loading);
    }

    /// Restart mid-synth: status stays restart/load progress; loading flag intact.
    #[test]
    fn restart_mid_synth_keeps_restart_status() {
        let mut state = TtsTimeoutUiState {
            status: "restarting voice…".into(),
            tts_loaded: false,
            tts_model_id: None,
            tts_loading: true, // load_tts already posted
        };
        let prior_job = 11u64;
        let d = apply_tts_synth_timeout(
            &mut state,
            None, // cancel() cleared active before new job begins
            Some(prior_job),
            "synthesize",
            "worker exited during synthesize — cancelled or crashed",
        );
        assert_eq!(d, SynthTimeoutDisposition::SilentCleanup);
        assert_eq!(state.status, "restarting voice…");
        assert!(state.tts_loading, "must not abort in-flight reload");
        assert!(!state.status.contains("cancelled or crashed"));
        assert!(!state.status.contains("worker exited"));
    }

    /// Failure for a non-live job_id must not clobber the active job's status.
    #[test]
    fn stale_old_job_failure_does_not_clobber_status() {
        let mut state = TtsTimeoutUiState {
            status: "synthesizing 1/4…".into(),
            tts_loaded: true,
            tts_model_id: Some("chatterbox-multilingual".into()),
            tts_loading: false,
        };
        let active = Some(5u64);
        let d = apply_tts_synth_timeout(
            &mut state,
            active,
            Some(4), // old cancelled job
            "synthesize",
            "worker exited during synthesize — cancelled or crashed",
        );
        assert_eq!(d, SynthTimeoutDisposition::SilentCleanup);
        assert_eq!(state.status, "synthesizing 1/4…");
        assert!(state.tts_loaded, "stale cancel must not clear live badge");
        assert_eq!(
            state.tts_model_id.as_deref(),
            Some("chatterbox-multilingual")
        );
        assert!(!should_reload_tts_after_live_synth_failure(
            state.tts_loaded,
            active
        ));
    }

    /// Real timeout on the current job: useful error, clear badge, reload branch.
    #[test]
    fn current_job_timeout_reports_clears_badge_and_reloads() {
        let mut state = TtsTimeoutUiState {
            status: "synthesizing 2/5…".into(),
            tts_loaded: true,
            tts_model_id: Some("chatterbox-multilingual".into()),
            tts_loading: false,
        };
        let job = 9u64;
        let error = "TTS synthesize timed out after 120s";
        let d = apply_tts_synth_timeout(
            &mut state,
            Some(job),
            Some(job),
            "synthesize",
            error,
        );
        assert_eq!(d, SynthTimeoutDisposition::ReportAndClearBadge);
        assert!(
            state.status.contains(error) || state.status.contains("timed out"),
            "status={:?}",
            state.status
        );
        assert!(state.status.starts_with("Tts "), "status={:?}", state.status);
        assert!(!state.tts_loaded, "voice badge must clear");
        assert!(state.tts_model_id.is_none());
        assert!(!state.tts_loading);
        assert!(
            should_reload_tts_after_live_synth_failure(state.tts_loaded, Some(job)),
            "live job with unloaded TTS should reload"
        );
    }

    #[test]
    fn classify_legacy_none_job_id_reports_only_when_job_active() {
        assert_eq!(
            classify_tts_synth_timeout(Some(1), None),
            SynthTimeoutDisposition::ReportAndClearBadge
        );
        assert_eq!(
            classify_tts_synth_timeout(None, None),
            SynthTimeoutDisposition::SilentCleanup
        );
    }

    #[test]
    fn stt_job_id_and_intent_on_transcribe_messages() {
        let cmd = JobCmd::Transcribe {
            job_id: 42,
            intent: RecordingIntent::HotkeyInsert,
            path: "/tmp/a.wav".into(),
            language: "en".into(),
        };
        match cmd {
            JobCmd::Transcribe {
                job_id,
                intent,
                ..
            } => {
                assert_eq!(job_id, 42);
                assert_eq!(intent, RecordingIntent::HotkeyInsert);
                assert!(intent.wants_insert_at_cursor());
            }
            _ => panic!("expected Transcribe"),
        }
        let ok = AppMsg::Transcribed {
            job_id: 42,
            intent: RecordingIntent::HotkeyInsert,
            text: "hi".into(),
        };
        let fail = AppMsg::TranscribeFailed {
            job_id: 42,
            error: "boom".into(),
            path: "/tmp/a.wav".into(),
        };
        match (&ok, &fail) {
            (
                AppMsg::Transcribed {
                    job_id: a,
                    intent,
                    ..
                },
                AppMsg::TranscribeFailed { job_id: b, .. },
            ) => {
                assert_eq!(*a, *b);
                assert_eq!(*intent, RecordingIntent::HotkeyInsert);
            }
            _ => panic!("shape"),
        }
    }

    #[test]
    fn stale_stt_job_id_is_ignored() {
        assert!(!is_live_stt_job(Some(2), 1));
        assert!(!is_live_stt_job(None, 1));
        assert!(is_live_stt_job(Some(3), 3));
    }

    /// Applying a non-live STT result must not clear or drive the live session.
    #[test]
    fn non_live_stt_result_does_not_apply_follow_up() {
        let live = Some(9u64);
        let stale_ok = AppMsg::Transcribed {
            job_id: 8,
            intent: RecordingIntent::HotkeyInsert,
            text: "should not paste".into(),
        };
        match stale_ok {
            AppMsg::Transcribed { job_id, intent, text } => {
                assert!(!is_live_stt_job(live, job_id));
                // Live session remains HotkeyInsert-owned; stale message ignored.
                let _ = (intent, text);
                assert_eq!(live, Some(9));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn mismatched_recording_release_is_noop() {
        assert!(!release_matches_open_recording(
            RecordingIntent::GuiPanel,
            RecordingIntent::HotkeyInsert
        ));
        assert!(!release_matches_open_recording(
            RecordingIntent::HotkeyInsert,
            RecordingIntent::GuiPanel
        ));
        assert!(!release_matches_open_recording(
            RecordingIntent::Idle,
            RecordingIntent::GuiPanel
        ));
        assert!(release_matches_open_recording(
            RecordingIntent::GuiPanel,
            RecordingIntent::GuiPanel
        ));
        assert!(release_matches_open_recording(
            RecordingIntent::HotkeyInsert,
            RecordingIntent::HotkeyInsert
        ));
    }

    #[test]
    fn unload_all_plan_matches_stop_hard_quit_hooks() {
        let idle = plan_unload_all(false);
        assert!(idle.stop_playback);
        assert!(idle.cleanup_chunks);
        assert!(!idle.oob_kill_tts_if_in_flight);
        assert!(idle.oob_kill_all_workers);
        assert!(idle.send_unload_all);

        let busy = plan_unload_all(true);
        assert!(busy.oob_kill_tts_if_in_flight);
        assert!(busy.oob_kill_all_workers);
        assert!(busy.send_unload_all);
        // Same OOB condition as Speak restart / Stop mid-synth.
        assert_eq!(
            busy.oob_kill_tts_if_in_flight,
            crate::ui::speak_restart_needs_oob_kill(true)
        );
    }
}
