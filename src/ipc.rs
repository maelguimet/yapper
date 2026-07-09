//! JSON-lines client for Python workers (stdio) with request timeouts.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

static REQ_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize)]
pub struct Request {
    pub id: String,
    pub cmd: String,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub params: HashMap<String, Value>,
    pub proto: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    pub id: String,
    pub ok: bool,
    #[serde(default)]
    pub result: HashMap<String, Value>,
    pub error: Option<ErrorBody>,
}

/// Line-oriented JSON-RPC client. A dedicated reader thread feeds responses
/// so `request_timeout` can bound waits and the UI/job thread never blocks forever.
pub struct WorkerClient {
    child: Child,
    stdin: ChildStdin,
    /// Receives raw JSON lines (or reader-exit errors) from the stdout thread.
    line_rx: Receiver<Result<String, String>>,
    pub role: String,
}

/// Env vars injected into STT/TTS worker processes from ``Config.models``.
/// Python ``yapper_common.paths`` reads these ahead of pure XDG defaults.
pub const ENV_MODELS_DIR: &str = "YAPPER_MODELS_DIR";
pub const ENV_VOICES_DIR: &str = "YAPPER_VOICES_DIR";

/// Build path-related env pairs for a worker child.
/// Empty/whitespace values are skipped so pure XDG defaults still apply.
pub fn worker_path_env(models_dir: &str, voices_dir: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::with_capacity(2);
    let models = models_dir.trim();
    if !models.is_empty() {
        out.push((ENV_MODELS_DIR, models.to_string()));
    }
    let voices = voices_dir.trim();
    if !voices.is_empty() {
        out.push((ENV_VOICES_DIR, voices.to_string()));
    }
    out
}

impl WorkerClient {
    /// Spawn a worker, injecting models/voices roots from config (via env).
    pub fn spawn(
        role: &str,
        python_bin: &str,
        python_root: &str,
        models_dir: &str,
        voices_dir: &str,
    ) -> Result<Self> {
        let module = match role {
            "stt" => "yapper_stt",
            "tts" => "yapper_tts",
            other => bail!("unknown worker role: {other}"),
        };
        let mut cmd = Command::new(python_bin);
        cmd.arg("-m")
            .arg(module)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .env("PYTHONUNBUFFERED", "1");
        // Empty python_root → rely on interpreter site-packages (self-contained install).
        // Non-empty → dev checkout or a stable package tree on PYTHONPATH.
        let root = python_root.trim();
        if !root.is_empty() {
            cmd.env("PYTHONPATH", root);
        }
        for (key, val) in worker_path_env(models_dir, voices_dir) {
            cmd.env(key, val);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn {module} via {python_bin}"))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let (line_tx, line_rx): (Sender<Result<String, String>>, _) = mpsc::channel();
        let role_owned = role.to_string();
        thread::Builder::new()
            .name(format!("yapper-{role}-stdout"))
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut buf = String::new();
                loop {
                    buf.clear();
                    match reader.read_line(&mut buf) {
                        Ok(0) => {
                            let _ = line_tx.send(Err(format!(
                                "worker {role_owned} closed stdout"
                            )));
                            break;
                        }
                        Ok(_) => {
                            let trimmed = buf.trim();
                            if trimmed.is_empty() || !trimmed.starts_with('{') {
                                continue;
                            }
                            if line_tx.send(Ok(trimmed.to_string())).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = line_tx.send(Err(format!("read worker stdout: {e}")));
                            break;
                        }
                    }
                }
            })
            .context("spawn worker stdout reader")?;
        Ok(Self {
            child,
            stdin,
            line_rx,
            role: role.to_string(),
        })
    }

    pub fn request_timeout(
        &mut self,
        cmd: &str,
        params: HashMap<String, Value>,
        timeout: Duration,
    ) -> Result<Response> {
        let id = REQ_COUNTER.fetch_add(1, Ordering::Relaxed).to_string();
        let req = Request {
            id: id.clone(),
            cmd: cmd.to_string(),
            params,
            proto: 1,
        };
        let line = serde_json::to_string(&req)?;
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()?;

        let deadline = std::time::Instant::now() + timeout;
        // Short poll slices so out-of-band Stop (SIGKILL of child) is noticed
        // quickly even if the stdout reader is slow to deliver EOF.
        const POLL: Duration = Duration::from_millis(100);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                // Hard kill so the next call can restart a clean worker.
                let _ = self.child.kill();
                let _ = self.child.wait();
                bail!(
                    "worker {} timed out after {:?} on cmd={cmd}",
                    self.role,
                    timeout
                );
            }
            // Detect external kill (Stop mid-generate) without waiting full timeout.
            if let Ok(Some(status)) = self.child.try_wait() {
                bail!(
                    "worker {} exited during {cmd} (status={status}) — cancelled or crashed",
                    self.role
                );
            }
            let slice = remaining.min(POLL);
            match self.line_rx.recv_timeout(slice) {
                Ok(Ok(raw)) => {
                    let resp: Response = serde_json::from_str(&raw)
                        .with_context(|| format!("parse response: {raw}"))?;
                    if resp.id != id {
                        // Stale/unexpected id — keep reading until ours arrives.
                        continue;
                    }
                    return Ok(resp);
                }
                Ok(Err(e)) => bail!("{e}"),
                Err(RecvTimeoutError::Timeout) => {
                    // Slice timeout only — loop and re-check deadline + child.
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    bail!("worker {} reader disconnected", self.role);
                }
            }
        }
    }

    pub fn ping(&mut self) -> Result<Response> {
        self.request_timeout("ping", HashMap::new(), Duration::from_secs(10))
    }

    pub fn shutdown(&mut self) -> Result<()> {
        let _ = self.request_timeout("shutdown", HashMap::new(), Duration::from_secs(5));
        let _ = self.child.try_wait();
        std::thread::sleep(Duration::from_millis(50));
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }

    /// OS pid of the worker process (for out-of-band Stop kill).
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Immediate kill without graceful unload (cancel mid-generate).
    pub fn kill_now(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

/// SIGKILL an OS pid we own (Stop mid-generate). Does not reap; owner waits later.
pub fn kill_os_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    // Prefer direct kill(2) via `kill` binary — always available on target Linux hosts.
    let _ = Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

impl Drop for WorkerClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Build params map helper.
pub fn params(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    #[test]
    fn request_serializes_cmd() {
        let req = Request {
            id: "1".into(),
            cmd: "ping".into(),
            params: HashMap::new(),
            proto: 1,
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"cmd\":\"ping\""));
        assert!(s.contains("\"proto\":1"));
    }

    #[test]
    fn worker_path_env_propagates_custom_dirs() {
        let env = worker_path_env("/custom/models", "/custom/voices");
        assert_eq!(
            env,
            vec![
                (ENV_MODELS_DIR, "/custom/models".into()),
                (ENV_VOICES_DIR, "/custom/voices".into()),
            ]
        );
    }

    #[test]
    fn worker_path_env_skips_blank_dirs() {
        assert!(worker_path_env("", "  ").is_empty());
        assert_eq!(
            worker_path_env("/m", ""),
            vec![(ENV_MODELS_DIR, "/m".into())]
        );
        assert_eq!(
            worker_path_env("  ", "/v"),
            vec![(ENV_VOICES_DIR, "/v".into())]
        );
    }

    /// End-to-end: same env pairs spawn injects are visible inside a child process.
    #[test]
    fn worker_path_env_visible_to_child_process() {
        let pairs = worker_path_env("/custom/models-root", "/custom/voices-root");
        let mut cmd = Command::new("python3");
        cmd.args([
            "-c",
            "import os; print(os.environ.get('YAPPER_MODELS_DIR','')); print(os.environ.get('YAPPER_VOICES_DIR',''))",
        ]);
        for (key, val) in &pairs {
            cmd.env(key, val);
        }
        let out = cmd.output().expect("spawn python3");
        assert!(out.status.success(), "stderr={}", String::from_utf8_lossy(&out.stderr));
        let stdout = String::from_utf8_lossy(&out.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines.get(0).copied(), Some("/custom/models-root"));
        assert_eq!(lines.get(1).copied(), Some("/custom/voices-root"));
    }

    #[test]
    fn response_deserializes_ok() {
        let raw = r#"{"id":"1","ok":true,"result":{"role":"stt"}}"#;
        let r: Response = serde_json::from_str(raw).unwrap();
        assert!(r.ok);
        assert_eq!(r.result.get("role").unwrap(), "stt");
    }

    #[test]
    fn response_deserializes_error() {
        let raw = r#"{"id":"2","ok":false,"error":{"code":"not_loaded","message":"x"}}"#;
        let r: Response = serde_json::from_str(raw).unwrap();
        assert!(!r.ok);
        assert_eq!(r.error.as_ref().unwrap().code, "not_loaded");
    }

    fn hanging_client() -> WorkerClient {
        let mut child = Command::new("bash")
            .args(["-c", "while true; do sleep 60; done"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleeper");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (line_tx, line_rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut buf = String::new();
            loop {
                buf.clear();
                match reader.read_line(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        let t = buf.trim();
                        if t.starts_with('{') {
                            let _ = line_tx.send(Ok(t.to_string()));
                        }
                    }
                    Err(e) => {
                        let _ = line_tx.send(Err(e.to_string()));
                        break;
                    }
                }
            }
        });
        WorkerClient {
            child,
            stdin,
            line_rx,
            role: "test".into(),
        }
    }

    /// Fake hanging worker: never replies; request_timeout must return Err and kill.
    #[test]
    fn request_timeout_kills_hanging_worker() {
        let mut client = hanging_client();
        let err = client
            .request_timeout("ping", HashMap::new(), Duration::from_millis(200))
            .expect_err("must timeout");
        let msg = format!("{err:#}");
        assert!(
            msg.to_ascii_lowercase().contains("timed out"),
            "unexpected error: {msg}"
        );
        assert!(
            !client.is_running(),
            "child should not still be running after timeout"
        );
    }

    /// Stop mid-generate path: out-of-band kill unblocks a long request within ~1s
    /// (must not wait for the full request_timeout deadline).
    #[test]
    fn out_of_band_kill_interrupts_hanging_request_within_1s() {
        use std::sync::{Arc, Mutex};
        use std::time::Instant;

        let mut client = hanging_client();
        let pid = client.pid();
        let result: Arc<Mutex<Option<Result<Response, String>>>> =
            Arc::new(Mutex::new(None));
        let result_bg = Arc::clone(&result);
        let started = Instant::now();
        let handle = thread::spawn(move || {
            let r = client
                .request_timeout("synthesize", HashMap::new(), Duration::from_secs(120))
                .map_err(|e| format!("{e:#}"));
            *result_bg.lock().unwrap() = Some(r);
            // Client dropped here — reaps if still needed.
        });

        // Let the blocking request start.
        thread::sleep(Duration::from_millis(50));
        kill_os_pid(pid);

        handle.join().expect("request thread");
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "out-of-band kill must unblock within 1s, took {elapsed:?} (not full 120s timeout)"
        );
        let outcome = result.lock().unwrap().take().expect("result set");
        assert!(outcome.is_err(), "killed request must fail: {outcome:?}");
    }
}
