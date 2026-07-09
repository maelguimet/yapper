//! mpv IPC playback backend + WAV duration helpers.

use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static MPV_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) struct MpvBackend {
    child: Child,
    ipc_path: Option<PathBuf>,
    /// When true, no IPC — only kill support (ffplay fallback).
    fallback: bool,
}

impl MpvBackend {
    pub(crate) fn start(path: &Path, volume: f32) -> Result<Self> {
        if !which_bin("mpv") {
            bail!("mpv not found");
        }
        let ipc = ipc_socket_path();
        if let Some(parent) = ipc.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::remove_file(&ipc);
        let vol = (volume.clamp(0.0, 1.0) * 100.0).round() as i32;
        let child = Command::new("mpv")
            .args([
                "--no-video",
                "--really-quiet",
                "--force-window=no",
                &format!("--volume={vol}"),
                &format!("--input-ipc-server={}", ipc.display()),
                path.to_str().context("path utf-8")?,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn mpv")?;
        // Wait briefly for socket; fail if mpv dies or IPC never becomes usable.
        let deadline = Instant::now() + Duration::from_millis(800);
        let mut backend = Self {
            child,
            ipc_path: Some(ipc.clone()),
            fallback: false,
        };
        loop {
            if let Ok(Some(status)) = backend.child.try_wait() {
                let _ = std::fs::remove_file(&ipc);
                bail!("mpv exited before IPC ready (status={status})");
            }
            if ipc.exists() {
                match UnixStream::connect(&ipc) {
                    Ok(stream) => {
                        drop(stream);
                        return Ok(backend);
                    }
                    Err(_) => {
                        // Socket file appeared but not ready yet — keep waiting.
                    }
                }
            }
            if Instant::now() >= deadline {
                backend.kill();
                bail!("mpv IPC socket not ready within timeout: {}", ipc.display());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub(crate) fn has_ipc(&self) -> bool {
        !self.fallback && self.ipc_path.is_some()
    }

    pub(crate) fn fallback_child(child: Child) -> Self {
        Self {
            child,
            ipc_path: None,
            fallback: true,
        }
    }

    pub(crate) fn connect(&self) -> Result<UnixStream> {
        let path = self
            .ipc_path
            .as_ref()
            .context("no ipc socket (fallback player)")?;
        UnixStream::connect(path).with_context(|| format!("connect {}", path.display()))
    }

    /// Send a JSON command with a unique request_id; return matching response line.
    pub(crate) fn command_raw(&self, cmd_array_json: &str) -> Result<String> {
        if self.fallback {
            bail!("fallback player has no IPC");
        }
        let request_id = MPV_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        // cmd_array_json is the value of "command", e.g. ["get_property","time-pos"]
        let payload = format!(
            r#"{{"command":{cmd_array_json},"request_id":{request_id}}}"#
        );
        let mut stream = self.connect()?;
        stream.set_read_timeout(Some(Duration::from_millis(400)))?;
        stream.set_write_timeout(Some(Duration::from_millis(300)))?;
        stream.write_all(payload.as_bytes())?;
        stream.write_all(b"\n")?;

        let mut reader = BufReader::new(stream);
        let deadline = Instant::now() + Duration::from_millis(400);
        let mut line = String::new();
        loop {
            if Instant::now() >= deadline {
                bail!("mpv IPC response timeout for request_id={request_id}");
            }
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => bail!("mpv IPC EOF waiting for request_id={request_id}"),
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Some(resp) = match_mpv_response(trimmed, request_id) {
                        return Ok(resp.to_string());
                    }
                    // Event or unrelated response — keep reading.
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    bail!("mpv IPC read timeout for request_id={request_id}");
                }
                Err(e) => return Err(e).context("mpv IPC read"),
            }
        }
    }

    pub(crate) fn pause(&mut self, paused: bool) -> Result<()> {
        if self.fallback {
            return Ok(());
        }
        let flag = if paused { "true" } else { "false" };
        let resp = self.command_raw(&format!(r#"["set_property","pause",{flag}]"#))?;
        if !mpv_response_ok(&resp) {
            bail!("mpv pause failed: {resp}");
        }
        Ok(())
    }

    pub(crate) fn seek_absolute(&mut self, secs: f64) -> Result<()> {
        if self.fallback {
            return Ok(());
        }
        let resp = self.command_raw(&format!(r#"["seek",{secs},"absolute"]"#))?;
        if !mpv_response_ok(&resp) {
            bail!("mpv seek failed: {resp}");
        }
        Ok(())
    }

    pub(crate) fn set_volume(&mut self, vol: i32) -> Result<()> {
        if self.fallback {
            return Ok(());
        }
        let resp = self.command_raw(&format!(r#"["set_property","volume",{vol}]"#))?;
        if !mpv_response_ok(&resp) {
            bail!("mpv volume failed: {resp}");
        }
        Ok(())
    }

    /// Append a file to the running playlist (no kill/respawn). Gap-reducing path
    /// for multi-chunk Speak: next sentence joins the same mpv process.
    pub(crate) fn append_file(&mut self, path: &Path) -> Result<()> {
        if self.fallback {
            bail!("fallback player cannot append to playlist");
        }
        if self.is_ended() {
            bail!("mpv process already exited; cannot append");
        }
        let path_json = serde_json::to_string(path.to_str().context("path utf-8")?)
            .context("json-escape path")?;
        let resp = self.command_raw(&format!(r#"["loadfile",{path_json},"append"]"#))?;
        if !mpv_response_ok(&resp) {
            bail!("mpv loadfile append failed: {resp}");
        }
        Ok(())
    }

    /// Playlist length when IPC is available (`None` for fallback / errors).
    /// Used by continuity tests to prove appends land on one process.
    #[cfg(test)]
    pub(crate) fn playlist_count(&mut self) -> Option<i64> {
        if self.fallback {
            return None;
        }
        let resp = self
            .command_raw(r#"["get_property","playlist-count"]"#)
            .ok()?;
        parse_mpv_data_i64(&resp)
    }

    pub(crate) fn time_pos(&mut self) -> Option<f64> {
        if self.fallback {
            return None;
        }
        let resp = self
            .command_raw(r#"["get_property","time-pos"]"#)
            .ok()?;
        parse_mpv_data_f64(&resp)
    }

    pub(crate) fn duration(&mut self) -> Option<f64> {
        if self.fallback {
            return None;
        }
        let resp = self
            .command_raw(r#"["get_property","duration"]"#)
            .ok()?;
        parse_mpv_data_f64(&resp)
    }

    pub(crate) fn is_ended(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(_) => true,
        }
    }

    /// True when this backend can accept `append_file` (live IPC, process up).
    pub(crate) fn can_append(&mut self) -> bool {
        self.has_ipc() && !self.is_ended()
    }

    pub(crate) fn kill(&mut self) {
        crate::audio::kill_child_process(&mut self.child);
        if let Some(p) = self.ipc_path.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

impl Drop for MpvBackend {
    fn drop(&mut self) {
        self.kill();
    }
}

/// From a buffer that may contain multiple JSON lines / events, return the
/// line matching `request_id` (ignoring event objects).
pub fn match_mpv_response(line_or_buffer: &str, request_id: u64) -> Option<&str> {
    for line in line_or_buffer.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(trimmed).ok()?;
        // Skip pure events.
        if v.get("event").is_some() && v.get("request_id").is_none() {
            continue;
        }
        let rid = v
            .get("request_id")
            .and_then(|r| r.as_u64().or_else(|| r.as_i64().map(|i| i as u64)));
        if rid == Some(request_id) {
            return Some(trimmed);
        }
    }
    None
}

pub fn mpv_response_ok(resp: &str) -> bool {
    let v: serde_json::Value = match serde_json::from_str(resp.trim()) {
        Ok(v) => v,
        Err(_) => return false,
    };
    v.get("error").and_then(|e| e.as_str()) == Some("success")
}

fn parse_mpv_success_data(resp: &str) -> Option<serde_json::Value> {
    // Prefer matching a single response object; also tolerate multi-line buffers.
    let line = if resp.contains('\n') {
        // Without a known request_id, take the first success line with data.
        resp.lines().find(|l| {
            let t = l.trim();
            !t.is_empty()
                && serde_json::from_str::<serde_json::Value>(t)
                    .ok()
                    .is_some_and(|v| {
                        v.get("event").is_none()
                            && v.get("error").and_then(|e| e.as_str()) == Some("success")
                    })
        })?
    } else {
        resp
    };
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("error").and_then(|e| e.as_str()) != Some("success") {
        return None;
    }
    v.get("data").cloned()
}

pub(crate) fn parse_mpv_data_f64(resp: &str) -> Option<f64> {
    parse_mpv_success_data(resp)?.as_f64()
}

#[cfg(test)]
pub(crate) fn parse_mpv_data_i64(resp: &str) -> Option<i64> {
    let data = parse_mpv_success_data(resp)?;
    data.as_i64()
        .or_else(|| data.as_u64().map(|u| u as i64))
        .or_else(|| data.as_f64().map(|f| f as i64))
}

pub(crate) fn ipc_socket_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("yapper-mpv-{nanos}.sock"))
}

pub(crate) fn which_bin(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Best-effort duration from WAV header (PCM).
pub fn wav_duration_secs(path: &Path) -> Result<f64> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    wav_duration_from_bytes(&bytes)
}

pub fn wav_duration_from_bytes(bytes: &[u8]) -> Result<f64> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a WAV");
    }
    let mut offset = 12usize;
    let mut sample_rate = 0u32;
    let mut channels = 1u16;
    let mut bits = 16u16;
    let mut data_bytes = 0u32;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let data_start = offset + 8;
        let data_end = data_start.saturating_add(size).min(bytes.len());
        if id == b"fmt " && size >= 16 {
            channels = u16::from_le_bytes(bytes[data_start + 2..data_start + 4].try_into().unwrap());
            sample_rate =
                u32::from_le_bytes(bytes[data_start + 4..data_start + 8].try_into().unwrap());
            bits = u16::from_le_bytes(bytes[data_start + 14..data_start + 16].try_into().unwrap());
        } else if id == b"data" {
            data_bytes = size as u32;
            break;
        }
        offset = data_end + (size % 2);
        if size == 0 {
            break;
        }
    }
    if sample_rate == 0 || channels == 0 || bits == 0 {
        bail!("incomplete WAV fmt");
    }
    let bytes_per_frame = (channels as u32) * (bits as u32 / 8);
    if bytes_per_frame == 0 {
        bail!("bad frame size");
    }
    Ok(data_bytes as f64 / (sample_rate as f64 * bytes_per_frame as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_mpv_skips_events_and_finds_request_id() {
        let buf = concat!(
            r#"{"event":"file-loaded"}"#,
            "\n",
            r#"{"request_id":42,"error":"success","data":12.3}"#,
            "\n",
        );
        let line = match_mpv_response(buf, 42).expect("response");
        assert!(line.contains("12.3"));
        assert!(mpv_response_ok(line));
        assert_eq!(parse_mpv_data_f64(line), Some(12.3));
    }

    #[test]
    fn match_mpv_ignores_wrong_request_id() {
        let buf = r#"{"request_id":7,"error":"success","data":1.0}"#;
        assert!(match_mpv_response(buf, 42).is_none());
    }

    #[test]
    fn parse_mpv_multiline_picks_success_data() {
        let buf = concat!(
            r#"{"event":"start-file"}"#,
            "\n",
            r#"{"request_id":1,"error":"success","data":3.5}"#,
        );
        assert_eq!(parse_mpv_data_f64(buf), Some(3.5));
    }

    #[test]
    fn parse_mpv_playlist_count_int() {
        let buf = r#"{"request_id":9,"error":"success","data":3}"#;
        assert_eq!(parse_mpv_data_i64(buf), Some(3));
    }
}
