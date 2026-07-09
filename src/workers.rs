//! Spawn/manage STT & TTS workers with dual-model LRU policy and timeouts.

use crate::config::Config;
use crate::timeouts;
use crate::ipc::{params, WorkerClient};
use crate::policy::{DualModelPolicy, Role};
use anyhow::{anyhow, bail, Result};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;

// Path resolution lives in `python_paths`; re-export for existing call sites.
pub use crate::python_paths::{resolve_python_bin, resolve_python_root, worker_package_status};

pub struct WorkerManager {
    cfg: Config,
    stt: Option<WorkerClient>,
    tts: Option<WorkerClient>,
    pub policy: DualModelPolicy,
}

impl WorkerManager {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            stt: None,
            tts: None,
            policy: DualModelPolicy::default(),
        }
    }

    pub fn ensure_worker(&mut self, role: Role) -> Result<()> {
        match role {
            Role::Stt => {
                if self.stt.as_mut().map(|w| w.is_running()).unwrap_or(false) {
                    return Ok(());
                }
                self.stt = Some(WorkerClient::spawn(
                    "stt",
                    &self.cfg.paths.python_bin,
                    &self.cfg.paths.python_root,
                )?);
            }
            Role::Tts => {
                if self.tts.as_mut().map(|w| w.is_running()).unwrap_or(false) {
                    return Ok(());
                }
                self.tts = Some(WorkerClient::spawn(
                    "tts",
                    &self.cfg.paths.python_bin,
                    &self.cfg.paths.python_root,
                )?);
            }
        }
        Ok(())
    }

    fn client_mut(&mut self, role: Role) -> Result<&mut WorkerClient> {
        match role {
            Role::Stt => self
                .stt
                .as_mut()
                .ok_or_else(|| anyhow!("STT worker not started")),
            Role::Tts => self
                .tts
                .as_mut()
                .ok_or_else(|| anyhow!("TTS worker not started")),
        }
    }

    /// Drop worker process immediately (cancel mid-generate / recovery after hang).
    pub fn kill_worker(&mut self, role: Role) {
        match role {
            Role::Stt => {
                if let Some(mut c) = self.stt.take() {
                    c.kill_now();
                }
                self.policy.mark_unloaded(Role::Stt);
            }
            Role::Tts => {
                if let Some(mut c) = self.tts.take() {
                    c.kill_now();
                }
                self.policy.mark_unloaded(Role::Tts);
            }
        }
    }

    /// Live OS pid for out-of-band Stop kill (None if worker not running).
    pub fn worker_pid(&self, role: Role) -> Option<u32> {
        match role {
            Role::Stt => self.stt.as_ref().map(|c| c.pid()),
            Role::Tts => self.tts.as_ref().map(|c| c.pid()),
        }
    }

    pub fn unload(&mut self, role: Role) -> Result<()> {
        self.unload_timeout(role, timeouts::unload())
    }

    pub fn unload_timeout(&mut self, role: Role, timeout: Duration) -> Result<()> {
        if let Ok(c) = self.client_mut(role) {
            let resp = match c.request_timeout("unload", Default::default(), timeout) {
                Ok(r) => r,
                Err(e) => {
                    // Hang on unload → hard kill and clear policy.
                    self.kill_worker(role);
                    return Err(e);
                }
            };
            if !resp.ok {
                let msg = resp
                    .error
                    .as_ref()
                    .map(|e| e.message.clone())
                    .unwrap_or_else(|| "unload failed".into());
                bail!("{msg}");
            }
        }
        self.policy.mark_unloaded(role);
        Ok(())
    }

    pub fn unload_all(&mut self) -> Result<()> {
        let _ = self.unload(Role::Stt);
        let _ = self.unload(Role::Tts);
        Ok(())
    }

    /// Load model for role; on OOM unload peer and retry once.
    pub fn load(&mut self, role: Role, model: &str, device: &str) -> Result<()> {
        let timeout = load_timeout(role, model);
        self.load_timeout(role, model, device, timeout)
    }

    pub fn load_timeout(
        &mut self,
        role: Role,
        model: &str,
        device: &str,
        timeout: Duration,
    ) -> Result<()> {
        if self.policy.already_loaded(role, model) {
            self.policy.touch(role);
            return Ok(());
        }
        if self.policy.slot_loaded(role) {
            self.unload_timeout(role, timeouts::unload())?;
        }
        self.ensure_worker(role)?;
        match self.try_load(role, model, device, timeout) {
            Ok(()) => Ok(()),
            Err(e) if is_oom_error(&e) => {
                let victim = self
                    .policy
                    .peer_to_unload_on_pressure(role)
                    .or_else(|| self.policy.lru_to_unload());
                if let Some(peer) = victim {
                    if peer != role {
                        self.unload_timeout(peer, timeouts::unload())?;
                        return self.try_load(role, model, device, timeout);
                    }
                }
                Err(e)
            }
            Err(e) if is_timeout_error(&e) => {
                self.kill_worker(role);
                Err(e)
            }
            Err(e) => Err(e),
        }
    }

    fn try_load(
        &mut self,
        role: Role,
        model: &str,
        device: &str,
        timeout: Duration,
    ) -> Result<()> {
        let c = self.client_mut(role)?;
        let resp = c.request_timeout(
            "load",
            params(&[("model", json!(model)), ("device", json!(device))]),
            timeout,
        )?;
        if !resp.ok {
            let code = resp
                .error
                .as_ref()
                .map(|e| e.code.as_str())
                .unwrap_or("internal");
            let msg = resp
                .error
                .as_ref()
                .map(|e| e.message.clone())
                .unwrap_or_else(|| "load failed".into());
            if code == "oom" {
                bail!("oom: {msg}");
            }
            bail!("{msg}");
        }
        self.policy.mark_loaded(role, model);
        Ok(())
    }

    pub fn transcribe(&mut self, path: &Path, language: &str) -> Result<String> {
        self.transcribe_timeout(path, language, timeouts::stt_transcribe())
    }

    pub fn transcribe_timeout(
        &mut self,
        path: &Path,
        language: &str,
        timeout: Duration,
    ) -> Result<String> {
        self.policy.touch(Role::Stt);
        let c = self.client_mut(Role::Stt)?;
        let resp = match c.request_timeout(
            "transcribe",
            params(&[
                ("path", json!(path.to_string_lossy())),
                ("language", json!(language)),
            ]),
            timeout,
        ) {
            Ok(r) => r,
            Err(e) => {
                if is_timeout_error(&e) {
                    self.kill_worker(Role::Stt);
                }
                return Err(e);
            }
        };
        if !resp.ok {
            bail!(
                "{}",
                resp.error
                    .as_ref()
                    .map(|e| e.message.as_str())
                    .unwrap_or("transcribe failed")
            );
        }
        debug_assert!(!self.policy.should_unload_after_job());
        Ok(resp
            .result
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string())
    }

    pub fn list_tones(&mut self) -> Result<Vec<String>> {
        self.ensure_worker(Role::Tts)?;
        let c = self.client_mut(Role::Tts)?;
        let resp = c.request_timeout("list_tones", Default::default(), Duration::from_secs(15))?;
        if !resp.ok {
            bail!("list_tones failed");
        }
        let tones = resp
            .result
            .get("tones")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        Ok(tones)
    }

    pub fn synthesize(
        &mut self,
        text: &str,
        language: &str,
        tone: &str,
        voice: &str,
        out_path: &Path,
    ) -> Result<PathBuf> {
        let timeout = timeouts::tts_synth_chunk(text.chars().count());
        self.synthesize_timeout(text, language, tone, voice, out_path, timeout)
    }

    pub fn synthesize_timeout(
        &mut self,
        text: &str,
        language: &str,
        tone: &str,
        voice: &str,
        out_path: &Path,
        timeout: Duration,
    ) -> Result<PathBuf> {
        self.policy.touch(Role::Tts);
        let c = self.client_mut(Role::Tts)?;
        let resp = match c.request_timeout(
            "synthesize",
            params(&[
                ("text", json!(text)),
                ("language", json!(language)),
                ("tone", json!(tone)),
                ("voice", json!(voice)),
                ("out_path", json!(out_path.to_string_lossy())),
            ]),
            timeout,
        ) {
            Ok(r) => r,
            Err(e) => {
                // Timeout *or* out-of-band Stop kill — drop dead worker + clear policy.
                self.kill_worker(Role::Tts);
                return Err(e);
            }
        };
        if !resp.ok {
            bail!(
                "{}",
                resp.error
                    .as_ref()
                    .map(|e| e.message.as_str())
                    .unwrap_or("synthesize failed")
            );
        }
        debug_assert!(!self.policy.should_unload_after_job());
        Ok(out_path.to_path_buf())
    }

    pub fn shutdown_all(&mut self) {
        if let Some(mut c) = self.stt.take() {
            let _ = c.shutdown();
        }
        if let Some(mut c) = self.tts.take() {
            let _ = c.shutdown();
        }
        self.policy = DualModelPolicy::default();
    }

    pub fn stt_loaded(&self) -> bool {
        self.policy.stt.loaded
    }

    pub fn tts_loaded(&self) -> bool {
        self.policy.tts.loaded
    }
}

impl DualModelPolicy {
    fn slot_loaded(&self, role: Role) -> bool {
        match role {
            Role::Stt => self.stt.loaded,
            Role::Tts => self.tts.loaded,
        }
    }
}

fn load_timeout(role: Role, model: &str) -> Duration {
    match role {
        Role::Stt if model == "medium" => timeouts::stt_load_medium(),
        Role::Stt => timeouts::stt_load_small(),
        Role::Tts => timeouts::tts_load(),
    }
}

fn is_oom_error(err: &anyhow::Error) -> bool {
    let s = format!("{err:#}").to_lowercase();
    s.contains("oom") || s.contains("out of memory")
}

fn is_timeout_error(err: &anyhow::Error) -> bool {
    format!("{err:#}").to_lowercase().contains("timed out")
}

#[cfg(test)]
mod tests {
    use crate::timeouts;

    #[test]
    fn synth_timeout_bounds() {
        assert_eq!(timeouts::tts_synth_chunk(0).as_secs(), 45);
        assert_eq!(timeouts::tts_synth_chunk(10_000).as_secs(), 180);
    }
}
