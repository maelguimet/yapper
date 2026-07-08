//! JSON-lines client for Python workers (stdio).

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
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

pub struct WorkerClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
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
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            role: role.to_string(),
        })
    }

    pub fn request(&mut self, cmd: &str, params: HashMap<String, Value>) -> Result<Response> {
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

        let mut buf = String::new();
        // Workers are single-threaded; one line response expected.
        loop {
            buf.clear();
            let n = self
                .stdout
                .read_line(&mut buf)
                .context("read worker response")?;
            if n == 0 {
                bail!("worker {} closed stdout", self.role);
            }
            let trimmed = buf.trim();
            if trimmed.is_empty() || !trimmed.starts_with('{') {
                continue;
            }
            let resp: Response = serde_json::from_str(trimmed)
                .with_context(|| format!("parse response: {trimmed}"))?;
            if resp.id != id {
                // Unexpected; keep reading (should not happen for v1)
                continue;
            }
            return Ok(resp);
        }
    }

    pub fn ping(&mut self) -> Result<Response> {
        self.request("ping", HashMap::new())
    }

    #[allow(dead_code)]
    pub fn status(&mut self) -> Result<Response> {
        self.request("status", HashMap::new())
    }

    pub fn shutdown(&mut self) -> Result<()> {
        let _ = self.request("shutdown", HashMap::new());
        // Give process a moment then kill if needed
        let _ = self.child.try_wait();
        std::thread::sleep(Duration::from_millis(50));
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
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
}
