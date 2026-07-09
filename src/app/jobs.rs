//! Background job thread: owns WorkerManager; UI only sends JobCmd / drains AppMsg.
//!
//! **Stop mid-generate:** `kill_tts_now()` SIGKILLs the live TTS pid out-of-band so
//! the UI never waits for the serial job_loop to finish an in-flight synthesize.

use super::live_pids::{
    job_shutdown_join_exceeded, LiveWorkerPids, JOB_SHUTDOWN_JOIN_BUDGET,
};
use super::messages::{AppMsg, JobCmd};
use crate::config::Config;
use crate::mpv_backend::wav_duration_secs;
use crate::policy::Role;
use crate::workers::WorkerManager;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Handle for posting work and draining results from the UI thread.
pub struct JobHub {
    cmd_tx: Sender<JobCmd>,
    msg_rx: Receiver<AppMsg>,
    /// Out-of-band kill registry (shared with job_loop).
    live: Arc<LiveWorkerPids>,
    /// Jobs thread handle; taken once during bounded shutdown.
    join: Option<thread::JoinHandle<()>>,
}

impl JobHub {
    pub fn start(cfg: Config) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<JobCmd>();
        let (msg_tx, msg_rx) = mpsc::channel::<AppMsg>();
        let live = Arc::new(LiveWorkerPids::default());
        let live_bg = Arc::clone(&live);
        let join = thread::Builder::new()
            .name("yapper-jobs".into())
            .spawn(move || job_loop(cfg, cmd_rx, msg_tx, live_bg))
            .expect("spawn yapper-jobs thread");
        Self {
            cmd_tx,
            msg_rx,
            live,
            join: Some(join),
        }
    }

    pub fn send(&self, cmd: JobCmd) {
        // Ignore send errors after shutdown (app exit).
        let _ = self.cmd_tx.send(cmd);
    }

    /// Out-of-band TTS kill — unblocks an in-flight synthesize without waiting
    /// for the serial job_loop. Always pair with `JobCmd::CancelTtsWorker` so
    /// policy/client state is cleaned when the job thread resumes.
    pub fn kill_tts_now(&self) -> bool {
        self.live.kill_tts_now()
    }

    /// Out-of-band kill of STT + TTS worker processes (hard quit).
    pub fn kill_all_now(&self) -> bool {
        self.live.kill_all_now()
    }

    /// Hard quit path: OOB kill workers, unload, shutdown jobs thread, join
    /// with a short deadline. Returns true if the jobs thread joined in time.
    /// Never blocks longer than `budget` (plus negligible channel overhead).
    pub fn shutdown_bounded(&mut self, budget: Duration) -> bool {
        let _ = self.live.kill_all_now();
        let _ = self.cmd_tx.send(JobCmd::UnloadAll);
        let _ = self.cmd_tx.send(JobCmd::Shutdown);
        let Some(handle) = self.join.take() else {
            return true;
        };
        // std JoinHandle has no join_timeout; wait via side channel.
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = handle.join();
            let _ = done_tx.send(());
        });
        let t0 = Instant::now();
        match done_rx.recv_timeout(budget) {
            Ok(()) => true,
            Err(_) => {
                // Budget exhausted: do not hang. Worker PIDs already killed.
                debug_assert!(job_shutdown_join_exceeded(t0.elapsed(), budget));
                let _ = job_shutdown_join_exceeded(t0.elapsed(), budget);
                false
            }
        }
    }

    /// Non-blocking drain of all pending messages.
    pub fn try_recv(&self) -> Option<AppMsg> {
        match self.msg_rx.try_recv() {
            Ok(m) => Some(m),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }

    pub fn drain(&self) -> Vec<AppMsg> {
        let mut out = Vec::new();
        while let Some(m) = self.try_recv() {
            out.push(m);
        }
        out
    }
}

impl Drop for JobHub {
    fn drop(&mut self) {
        if self.join.is_some() {
            let _ = self.shutdown_bounded(JOB_SHUTDOWN_JOIN_BUDGET);
        } else {
            let _ = self.cmd_tx.send(JobCmd::Shutdown);
        }
    }
}

fn sync_live_pids(workers: &WorkerManager, live: &LiveWorkerPids) {
    live.set_tts(workers.worker_pid(Role::Tts));
    live.set_stt(workers.worker_pid(Role::Stt));
}

fn job_loop(
    cfg: Config,
    cmd_rx: Receiver<JobCmd>,
    msg_tx: Sender<AppMsg>,
    live: Arc<LiveWorkerPids>,
) {
    let mut workers = WorkerManager::new(cfg);
    while let Ok(cmd) = cmd_rx.recv() {
        if matches!(cmd, JobCmd::Shutdown) {
            workers.shutdown_all();
            live.set_tts(None);
            live.set_stt(None);
            break;
        }
        handle_cmd(&mut workers, cmd, &msg_tx, &live);
        sync_live_pids(&workers, &live);
    }
}

fn push_status(workers: &WorkerManager, msg_tx: &Sender<AppMsg>) {
    let _ = msg_tx.send(AppMsg::ModelStatus {
        stt_loaded: workers.stt_loaded(),
        stt_model: workers.policy.stt.model_id.clone(),
        tts_loaded: workers.tts_loaded(),
        tts_model: workers.policy.tts.model_id.clone(),
    });
}

fn handle_cmd(
    workers: &mut WorkerManager,
    cmd: JobCmd,
    msg_tx: &Sender<AppMsg>,
    live: &LiveWorkerPids,
) {
    match cmd {
        JobCmd::LoadStt { model, device } => {
            let result = workers
                .load(Role::Stt, &model, &device)
                .map_err(|e| format!("{e:#}"));
            sync_live_pids(workers, live);
            let _ = msg_tx.send(AppMsg::SttLoaded {
                model: model.clone(),
                result,
            });
            push_status(workers, msg_tx);
        }
        JobCmd::LoadTts { device } => {
            let result = workers
                .load(Role::Tts, "chatterbox-multilingual", &device)
                .map_err(|e| format!("{e:#}"));
            sync_live_pids(workers, live);
            let _ = msg_tx.send(AppMsg::TtsLoaded { result });
            push_status(workers, msg_tx);
        }
        JobCmd::Unload { role } => {
            let result = workers.unload(role).map_err(|e| format!("{e:#}"));
            sync_live_pids(workers, live);
            let _ = msg_tx.send(AppMsg::Unloaded {
                role: Some(role),
                result,
            });
            push_status(workers, msg_tx);
        }
        JobCmd::UnloadAll => {
            let result = workers.unload_all().map_err(|e| format!("{e:#}"));
            sync_live_pids(workers, live);
            let _ = msg_tx.send(AppMsg::Unloaded { role: None, result });
            push_status(workers, msg_tx);
        }
        JobCmd::Transcribe { path, language } => {
            if !workers.stt_loaded() {
                let _ = msg_tx.send(AppMsg::TranscribeFailed {
                    error: "STT not loaded".into(),
                    path,
                });
                return;
            }
            sync_live_pids(workers, live);
            match workers.transcribe(&path, &language) {
                Ok(text) => {
                    let _ = msg_tx.send(AppMsg::Transcribed { text });
                }
                Err(e) => {
                    let err = format!("{e:#}");
                    // Timeout path kills STT worker; always refresh badges from policy.
                    if err.to_ascii_lowercase().contains("timed out") {
                        let _ = msg_tx.send(AppMsg::WorkerTimedOut {
                            role: Role::Stt,
                            op: "transcribe".into(),
                            error: err.clone(),
                        });
                    }
                    push_status(workers, msg_tx);
                    let _ = msg_tx.send(AppMsg::TranscribeFailed { error: err, path });
                }
            }
            sync_live_pids(workers, live);
        }
        JobCmd::Synthesize {
            job_id,
            index,
            total,
            text,
            language,
            tone,
            voice,
            out_path,
        } => {
            if !workers.tts_loaded() {
                let _ = msg_tx.send(AppMsg::TtsChunkFailed {
                    job_id,
                    index,
                    error: "TTS not loaded".into(),
                });
                return;
            }
            let _ = total;
            // Publish pid *before* blocking so Stop / Restart can kill mid-generate.
            sync_live_pids(workers, live);
            match workers.synthesize(&text, &language, &tone, &voice, &out_path) {
                Ok(path) => {
                    let duration_secs = wav_duration_secs(&path).unwrap_or(0.0);
                    let _ = msg_tx.send(AppMsg::TtsChunkReady {
                        job_id,
                        index,
                        text,
                        path,
                        duration_secs,
                    });
                }
                Err(e) => {
                    let err = format!("{e:#}");
                    // synthesize_timeout kills TTS on *any* request Err (timeout,
                    // broken pipe, cancelled, crash). Always push ModelStatus.
                    let _ = msg_tx.send(AppMsg::WorkerTimedOut {
                        role: Role::Tts,
                        op: "synthesize".into(),
                        error: err.clone(),
                    });
                    push_status(workers, msg_tx);
                    let _ = msg_tx.send(AppMsg::TtsChunkFailed {
                        job_id,
                        index,
                        error: err,
                    });
                }
            }
            sync_live_pids(workers, live);
        }
        JobCmd::CancelTtsWorker => {
            // May already be dead from kill_tts_now(); still clear policy + client.
            workers.kill_worker(Role::Tts);
            live.set_tts(None);
            push_status(workers, msg_tx);
        }
        JobCmd::ListTones => {
            match workers.list_tones() {
                Ok(tones) if !tones.is_empty() => {
                    let _ = msg_tx.send(AppMsg::TonesListed { tones });
                }
                Ok(_) | Err(_) => {
                    // Keep UI fallback tones; no dramatic error.
                }
            }
            // list_tones may have spawned a worker without loading the model —
            // do not claim TTS loaded; policy only marks on successful load.
            push_status(workers, msg_tx);
            sync_live_pids(workers, live);
        }
        JobCmd::Shutdown => {
            workers.shutdown_all();
            live.set_tts(None);
            live.set_stt(None);
        }
    }
}

/// Pure classifier: any synth request failure that killed the worker must clear UI badges.
#[cfg(test)]
pub fn synth_error_clears_loaded_badge() -> bool {
    // WorkerManager::synthesize_timeout always kill_worker on Err.
    crate::ui::synth_error_resets_worker()
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::messages::{is_live_tts_job, AppMsg};
    use std::time::{Duration, Instant};

    #[test]
    fn drain_filter_stale_chunks_logic() {
        let active = Some(7u64);
        let live = AppMsg::TtsChunkReady {
            job_id: 7,
            index: 0,
            text: "hi".into(),
            path: "/tmp/a.wav".into(),
            duration_secs: 1.0,
        };
        let stale = AppMsg::TtsChunkReady {
            job_id: 6,
            index: 1,
            text: "bye".into(),
            path: "/tmp/b.wav".into(),
            duration_secs: 1.0,
        };
        match (&live, &stale) {
            (
                AppMsg::TtsChunkReady { job_id: a, .. },
                AppMsg::TtsChunkReady { job_id: b, .. },
            ) => {
                assert!(is_live_tts_job(active, *a));
                assert!(!is_live_tts_job(active, *b));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn restart_and_stop_share_oob_kill_path() {
        // Same LiveWorkerPids::kill_tts_now used by cancel_tts_pipeline and
        // start_speak_job when speak_restart_needs_oob_kill is true.
        assert!(crate::ui::speak_restart_needs_oob_kill(true));
        assert!(synth_error_clears_loaded_badge());
    }

    /// Idle JobHub must shut down and join well under the hard-quit budget.
    #[test]
    fn shutdown_bounded_joins_idle_hub() {
        let cfg = Config::default();
        let mut hub = JobHub::start(cfg);
        let t0 = Instant::now();
        let joined = hub.shutdown_bounded(JOB_SHUTDOWN_JOIN_BUDGET);
        let elapsed = t0.elapsed();
        assert!(joined, "idle jobs thread must join");
        assert!(
            elapsed < JOB_SHUTDOWN_JOIN_BUDGET,
            "join took {elapsed:?}, budget {JOB_SHUTDOWN_JOIN_BUDGET:?}"
        );
        // Second call is a no-op (handle already taken).
        assert!(hub.shutdown_bounded(Duration::from_millis(50)));
    }
}
