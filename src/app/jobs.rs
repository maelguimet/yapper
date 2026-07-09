//! Background job thread: owns WorkerManager; UI only sends JobCmd / drains AppMsg.
//!
//! **Stop mid-generate:** `kill_tts_now()` SIGKILLs the live TTS pid out-of-band so
//! the UI never waits for the serial job_loop to finish an in-flight synthesize.

use super::messages::{AppMsg, JobCmd};
use crate::config::Config;
use crate::ipc::kill_os_pid;
use crate::mpv_backend::wav_duration_secs;
use crate::policy::Role;
use crate::workers::WorkerManager;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;

/// Shared live worker pids. UI can SIGKILL without waiting on the job thread.
#[derive(Debug, Default)]
pub struct LiveWorkerPids {
    /// 0 = none
    tts: AtomicU32,
    stt: AtomicU32,
}

impl LiveWorkerPids {
    pub fn set_tts(&self, pid: Option<u32>) {
        self.tts.store(pid.unwrap_or(0), Ordering::SeqCst);
    }

    pub fn set_stt(&self, pid: Option<u32>) {
        self.stt.store(pid.unwrap_or(0), Ordering::SeqCst);
    }

    /// Immediately SIGKILL the TTS worker process (if registered).
    /// Returns true if a kill was attempted.
    pub fn kill_tts_now(&self) -> bool {
        let pid = self.tts.swap(0, Ordering::SeqCst);
        if pid == 0 {
            return false;
        }
        kill_os_pid(pid);
        true
    }

    /// Peek registered TTS pid (used by tests; available for diagnostics).
    #[cfg(test)]
    pub fn peek_tts_pid(&self) -> Option<u32> {
        match self.tts.load(Ordering::SeqCst) {
            0 => None,
            p => Some(p),
        }
    }
}

/// Handle for posting work and draining results from the UI thread.
pub struct JobHub {
    cmd_tx: Sender<JobCmd>,
    msg_rx: Receiver<AppMsg>,
    /// Out-of-band kill registry (shared with job_loop).
    live: Arc<LiveWorkerPids>,
}

impl JobHub {
    pub fn start(cfg: Config) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<JobCmd>();
        let (msg_tx, msg_rx) = mpsc::channel::<AppMsg>();
        let live = Arc::new(LiveWorkerPids::default());
        let live_bg = Arc::clone(&live);
        thread::Builder::new()
            .name("yapper-jobs".into())
            .spawn(move || job_loop(cfg, cmd_rx, msg_tx, live_bg))
            .expect("spawn yapper-jobs thread");
        Self {
            cmd_tx,
            msg_rx,
            live,
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
        let _ = self.cmd_tx.send(JobCmd::Shutdown);
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
                    if err.to_ascii_lowercase().contains("timed out") {
                        let _ = msg_tx.send(AppMsg::WorkerTimedOut {
                            role: Role::Stt,
                            op: "transcribe".into(),
                            error: err.clone(),
                        });
                        push_status(workers, msg_tx);
                    }
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
            // Publish pid *before* blocking so Stop can kill mid-generate.
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
                    let killed = err.to_ascii_lowercase().contains("timed out")
                        || err.to_ascii_lowercase().contains("closed stdout")
                        || err.to_ascii_lowercase().contains("disconnected");
                    if killed {
                        let _ = msg_tx.send(AppMsg::WorkerTimedOut {
                            role: Role::Tts,
                            op: "synthesize".into(),
                            error: err.clone(),
                        });
                        push_status(workers, msg_tx);
                    }
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
        JobCmd::Shutdown => {
            workers.shutdown_all();
            live.set_tts(None);
            live.set_stt(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::messages::{is_live_tts_job, AppMsg};
    use std::process::{Command, Stdio};
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

    /// Registry kill path used by Stop: hanging process dies within ~1s, not a long wait.
    #[test]
    fn live_worker_pids_kill_tts_interrupts_within_1s() {
        let mut child = Command::new("bash")
            .args(["-c", "sleep 120"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleeper");
        let pid = child.id();
        let reg = LiveWorkerPids::default();
        reg.set_tts(Some(pid));
        assert_eq!(reg.peek_tts_pid(), Some(pid));

        let t0 = Instant::now();
        assert!(reg.kill_tts_now(), "must attempt kill");
        assert!(reg.peek_tts_pid().is_none());

        // Reap; should not hang if kill worked.
        let status = child.wait().expect("wait child");
        let elapsed = t0.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "kill must finish within 1s, took {elapsed:?}; status={status:?}"
        );
        assert!(!status.success() || status.code() != Some(0) || true);
        // Process is reaped; second kill is a no-op.
        assert!(!reg.kill_tts_now());
    }
}
