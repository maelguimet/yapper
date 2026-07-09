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

impl WorkerClient {
    pub fn spawn(role: &str, python_bin: &str, python_root: &str) -> Result<Self> {
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
            .env("PYTHONPATH", python_root)
            .env("PYTHONUNBUFFERED", "1");
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
            match self.line_rx.recv_timeout(remaining) {
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
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    bail!(
                        "worker {} timed out after {:?} on cmd={cmd}",
                        self.role,
                        timeout
                    );
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

    /// Immediate kill without graceful unload (cancel mid-generate).
    pub fn kill_now(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
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

    /// Fake hanging worker: never replies; request_timeout must return Err and kill.
    #[test]
    fn request_timeout_kills_hanging_worker() {
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
        let mut client = WorkerClient {
            child,
            stdin,
            line_rx,
            role: "test".into(),
        };
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
}
