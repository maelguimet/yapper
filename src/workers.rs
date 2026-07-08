//! Spawn/manage STT & TTS workers with dual-model LRU policy.

use crate::config::Config;
use crate::ipc::{params, WorkerClient};
use crate::policy::{DualModelPolicy, Role};
use anyhow::{anyhow, bail, Result};
use serde_json::json;
use std::path::{Path, PathBuf};

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

    pub fn unload(&mut self, role: Role) -> Result<()> {
        if let Ok(c) = self.client_mut(role) {
            let resp = c.request("unload", Default::default())?;
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
        if self.policy.already_loaded(role, model) {
            self.policy.touch(role);
            return Ok(());
        }
        // different model already loaded → unload first
        if self.policy.slot_loaded(role) {
            self.unload(role)?;
        }
        self.ensure_worker(role)?;
        match self.try_load(role, model, device) {
            Ok(()) => Ok(()),
            Err(e) if is_oom_error(&e) => {
                if let Some(peer) = self.policy.peer_to_unload_on_pressure(role) {
                    self.unload(peer)?;
                    self.try_load(role, model, device)
                } else {
                    Err(e)
                }
            }
            Err(e) => Err(e),
        }
    }

    fn try_load(&mut self, role: Role, model: &str, device: &str) -> Result<()> {
        let c = self.client_mut(role)?;
        let resp = c.request(
            "load",
            params(&[("model", json!(model)), ("device", json!(device))]),
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
        self.policy.touch(Role::Stt);
        let c = self.client_mut(Role::Stt)?;
        let resp = c.request(
            "transcribe",
            params(&[
                ("path", json!(path.to_string_lossy())),
                ("language", json!(language)),
            ]),
        )?;
        if !resp.ok {
            bail!(
                "{}",
                resp.error
                    .as_ref()
                    .map(|e| e.message.as_str())
                    .unwrap_or("transcribe failed")
            );
        }
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
        let resp = c.request("list_tones", Default::default())?;
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
        self.policy.touch(Role::Tts);
        let c = self.client_mut(Role::Tts)?;
        let resp = c.request(
            "synthesize",
            params(&[
                ("text", json!(text)),
                ("language", json!(language)),
                ("tone", json!(tone)),
                ("voice", json!(voice)),
                ("out_path", json!(out_path.to_string_lossy())),
            ]),
        )?;
        if !resp.ok {
            bail!(
                "{}",
                resp.error
                    .as_ref()
                    .map(|e| e.message.as_str())
                    .unwrap_or("synthesize failed")
            );
        }
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

fn is_oom_error(err: &anyhow::Error) -> bool {
    let s = format!("{err:#}").to_lowercase();
    s.contains("oom") || s.contains("out of memory")
}

/// Resolve python root relative to executable / env / config.
pub fn resolve_python_root(cfg: &Config) -> PathBuf {
    if let Ok(p) = std::env::var("YAPPER_PYTHON_ROOT") {
        return PathBuf::from(p);
    }
    let from_cfg = PathBuf::from(&cfg.paths.python_root);
    if from_cfg.is_dir() {
        return from_cfg;
    }
    // try relative to CARGO_MANIFEST_DIR at compile for tests/dev
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("python");
    if manifest.is_dir() {
        return manifest;
    }
    from_cfg
}

pub fn resolve_python_bin(cfg: &Config) -> String {
    if let Ok(p) = std::env::var("YAPPER_PYTHON") {
        return p;
    }
    let venv = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".venv/bin/python");
    if venv.is_file() {
        return venv.to_string_lossy().into();
    }
    cfg.paths.python_bin.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_python_root_finds_repo() {
        let cfg = Config::default();
        let root = resolve_python_root(&cfg);
        assert!(
            root.join("yapper_stt").is_dir() || root.join("yapper_common").is_dir(),
            "python root should contain packages: {}",
            root.display()
        );
    }
}
