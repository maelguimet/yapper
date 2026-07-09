//! Background job thread: owns WorkerManager; UI only sends JobCmd / drains AppMsg.

use super::messages::{AppMsg, JobCmd};
use crate::config::Config;
use crate::mpv_backend::wav_duration_secs;
use crate::policy::Role;
use crate::workers::WorkerManager;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

/// Handle for posting work and draining results from the UI thread.
pub struct JobHub {
    cmd_tx: Sender<JobCmd>,
    msg_rx: Receiver<AppMsg>,
}

impl JobHub {
    pub fn start(cfg: Config) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<JobCmd>();
        let (msg_tx, msg_rx) = mpsc::channel::<AppMsg>();
        thread::Builder::new()
            .name("yapper-jobs".into())
            .spawn(move || job_loop(cfg, cmd_rx, msg_tx))
            .expect("spawn yapper-jobs thread");
        Self { cmd_tx, msg_rx }
    }

    pub fn send(&self, cmd: JobCmd) {
        // Ignore send errors after shutdown (app exit).
        let _ = self.cmd_tx.send(cmd);
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

fn job_loop(cfg: Config, cmd_rx: Receiver<JobCmd>, msg_tx: Sender<AppMsg>) {
    let mut workers = WorkerManager::new(cfg);
    while let Ok(cmd) = cmd_rx.recv() {
        if matches!(cmd, JobCmd::Shutdown) {
            workers.shutdown_all();
            break;
        }
        handle_cmd(&mut workers, cmd, &msg_tx);
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

fn handle_cmd(workers: &mut WorkerManager, cmd: JobCmd, msg_tx: &Sender<AppMsg>) {
    match cmd {
        JobCmd::LoadStt { model, device } => {
            let result = workers
                .load(Role::Stt, &model, &device)
                .map_err(|e| format!("{e:#}"));
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
            let _ = msg_tx.send(AppMsg::TtsLoaded { result });
            push_status(workers, msg_tx);
        }
        JobCmd::Unload { role } => {
            let result = workers.unload(role).map_err(|e| format!("{e:#}"));
            let _ = msg_tx.send(AppMsg::Unloaded {
                role: Some(role),
                result,
            });
            push_status(workers, msg_tx);
        }
        JobCmd::UnloadAll => {
            let result = workers.unload_all().map_err(|e| format!("{e:#}"));
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
            let _ = total; // reserved for future progress payloads
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
                    if err.to_ascii_lowercase().contains("timed out") {
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
        }
        JobCmd::CancelTtsWorker => {
            workers.kill_worker(Role::Tts);
            push_status(workers, msg_tx);
        }
        JobCmd::Shutdown => {
            workers.shutdown_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::messages::{is_live_tts_job, AppMsg};

    #[test]
    fn drain_filter_stale_chunks_logic() {
        // Simulates UI filter used after Stop / new Speak.
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
}
