//! mpv IPC playback backend + WAV duration helpers.

use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
        // Wait briefly for socket.
        let deadline = Instant::now() + Duration::from_millis(800);
        while Instant::now() < deadline {
            if ipc.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(Self {
            child,
            ipc_path: Some(ipc),
            fallback: false,
        })
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

    pub(crate) fn command_raw(&self, cmd: &str) -> Result<String> {
        if self.fallback {
            bail!("fallback player has no IPC");
        }
        let mut stream = self.connect()?;
        stream.set_read_timeout(Some(Duration::from_millis(300)))?;
        stream.set_write_timeout(Some(Duration::from_millis(300)))?;
        stream.write_all(cmd.as_bytes())?;
        stream.write_all(b"\n")?;
        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).unwrap_or(0);
        Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
    }

    pub(crate) fn pause(&mut self, paused: bool) -> Result<()> {
        if self.fallback {
            return Ok(());
        }
        let _ = self.command_raw(&format!(
            r#"{{"command":["set_property","pause",{}]}}"#,
            if paused { "true" } else { "false" }
        ));
        Ok(())
    }

    pub(crate) fn seek_absolute(&mut self, secs: f64) -> Result<()> {
        if self.fallback {
            return Ok(());
        }
        let _ = self.command_raw(&format!(
            r#"{{"command":["seek",{secs},"absolute"]}}"#
        ));
        Ok(())
    }

    pub(crate) fn set_volume(&mut self, vol: i32) -> Result<()> {
        if self.fallback {
            return Ok(());
        }
        let _ = self.command_raw(&format!(
            r#"{{"command":["set_property","volume",{vol}]}}"#
        ));
        Ok(())
    }

    pub(crate) fn time_pos(&mut self) -> Option<f64> {
        if self.fallback {
            return None;
        }
        let resp = self
            .command_raw(r#"{"command":["get_property","time-pos"]}"#)
            .ok()?;
        parse_mpv_data_f64(&resp)
    }

    pub(crate) fn duration(&mut self) -> Option<f64> {
        if self.fallback {
            return None;
        }
        let resp = self
            .command_raw(r#"{"command":["get_property","duration"]}"#)
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

pub(crate) fn parse_mpv_data_f64(resp: &str) -> Option<f64> {
    // {"data":12.3,"request_id":0,"error":"success"}
    let v: serde_json::Value = serde_json::from_str(resp.trim()).ok()?;
    if v.get("error").and_then(|e| e.as_str()) != Some("success") {
        return None;
    }
    v.get("data").and_then(|d| d.as_f64())
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

