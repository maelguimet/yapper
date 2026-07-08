//! Mic capture and playback helpers (ffmpeg/parec/aplay).

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn temp_wav_path(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("yapper-{prefix}-{nanos}.wav"))
}

/// Start recording default mic to WAV via ffmpeg (pulse).
pub fn start_recording(out: &Path) -> Result<Child> {
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Prefer pulse default source
    let child = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "pulse",
            "-i",
            "default",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            out.to_str().unwrap_or("/tmp/yapper-rec.wav"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn ffmpeg pulse record")?;
    Ok(child)
}

pub fn stop_recording(mut child: Child) -> Result<()> {
    // graceful: send 'q' not available without stdin; kill is fine for ffmpeg
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

/// Play a WAV file asynchronously (returns child).
pub fn play_wav(path: &Path) -> Result<Child> {
    if !path.is_file() {
        bail!("audio file missing: {}", path.display());
    }
    // ffplay is common with ffmpeg package; fall back to aplay/paplay
    if which("ffplay") {
        return Command::new("ffplay")
            .args(["-nodisp", "-autoexit", "-loglevel", "quiet"])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("ffplay");
    }
    if which("paplay") {
        return Command::new("paplay")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("paplay");
    }
    if which("aplay") {
        return Command::new("aplay")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("aplay");
    }
    bail!("no audio player found (ffplay/paplay/aplay)");
}

pub fn stop_playback(child: &mut Option<Child>) {
    if let Some(mut c) = child.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

fn which(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_path_has_prefix() {
        let p = temp_wav_path("test");
        assert!(p.to_string_lossy().contains("yapper-test-"));
        assert!(p.extension().and_then(|e| e.to_str()) == Some("wav"));
    }
}
