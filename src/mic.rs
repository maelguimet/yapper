//! Pulse source listing and human mic labels.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// Pulse/PipeWire input source entry (from `pactl list sources short`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulseSource {
    pub index: u32,
    /// Stable name used as ffmpeg `-i` argument (or `"default"`).
    pub name: String,
    pub driver: String,
    pub sample_spec: String,
    pub state: String,
}

impl PulseSource {
    /// Human-readable label for UI dropdowns (product-style when possible).
    pub fn label(&self) -> String {
        human_mic_label(&self.name)
    }

    /// True for real capture inputs (not `*.monitor` / output-side sources).
    pub fn is_capture_input(&self) -> bool {
        is_capture_source_name(&self.name)
    }
}

/// Filter pactl sources to real microphones (no monitors / output loops).
pub fn is_capture_source_name(name: &str) -> bool {
    let n = name.trim();
    if n.is_empty() {
        return false;
    }
    let lower = n.to_ascii_lowercase();
    if lower.ends_with(".monitor") || lower.ends_with("_monitor") {
        return false;
    }
    true
}

/// Derive a short product-style name from a Pulse/ALSA source id.
pub fn human_mic_label(name: &str) -> String {
    let n = name.trim();
    if n.is_empty() || n == "default" {
        return "System default".into();
    }
    if n.ends_with(".monitor") {
        return format!("{} (monitor)", n);
    }
    let body = n
        .strip_prefix("alsa_input.")
        .or_else(|| n.strip_prefix("alsa_output."))
        .unwrap_or(n);
    let body = body.strip_prefix("usb-").unwrap_or(body);
    let mut parts: Vec<&str> = body
        .split(['.', '-', '_'])
        .filter(|p| !p.is_empty())
        .collect();
    const DROP: &[&str] = &[
        "mono", "fallback", "analog", "stereo", "iec958", "hdmi", "output",
        "input", "audio", "device", "usb", "pci", "pipewire", "alsa",
        // Corporate noise from USB product strings
        "ltd", "co", "inc", "corp", "llc", "gmbh", "sa", "bv",
    ];
    parts.retain(|p| {
        let l = p.to_ascii_lowercase();
        if DROP.contains(&l.as_str()) {
            return false;
        }
        if p.len() >= 8 && p.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
        if p.len() == 8 && p.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        true
    });
    if parts.is_empty() {
        return n.chars().take(48).collect();
    }
    let tokens: Vec<&str> = parts;
    let label = if tokens.len() > 4 {
        let interesting: Vec<&str> = tokens
            .iter()
            .copied()
            .filter(|t| {
                t.chars().any(|c| c.is_ascii_alphabetic())
                    && (t.chars().any(|c| c.is_ascii_digit()) || t.len() >= 3)
            })
            .collect();
        // Prefer brand/model tokens (e.g. TONOR + TC30) over USB vendor strings.
        let productish: Vec<&str> = interesting
            .iter()
            .copied()
            .filter(|t| {
                let u = t.to_ascii_uppercase();
                u.chars().any(|c| c.is_ascii_digit())
                    || u.contains("TONOR")
                    || u.contains("MIC")
                    || u.contains("WEBCAM")
            })
            .collect();
        if !productish.is_empty() {
            // Cap at last 3 product tokens so "TONOR TC30" not full vendor path.
            let start = productish.len().saturating_sub(3);
            productish[start..].join(" ")
        } else if interesting.len() >= 2 {
            interesting[interesting.len().saturating_sub(3)..].join(" ")
        } else {
            tokens[tokens.len().saturating_sub(3)..].join(" ")
        }
    } else {
        tokens.join(" ")
    };
    if label.is_empty() { n.chars().take(48).collect() } else { label }
}

/// Peak / RMS energy over PCM samples or a finished WAV.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WavEnergy {
    pub frames: usize,
    /// Max absolute sample (0..=32767 typical).
    pub peak: i16,
    pub rms: f64,
}

impl WavEnergy {
    /// True when capture has more than near-digital silence.
    pub fn is_non_silence(&self) -> bool {
        self.frames > 0 && (self.peak as i32 >= SILENCE_PEAK_FLOOR || self.rms >= SILENCE_RMS_FLOOR)
    }
}

/// Peak below this (and RMS below floor) counts as silence for doctor/probe.
const SILENCE_PEAK_FLOOR: i32 = 300;
const SILENCE_RMS_FLOOR: f64 = 40.0;

pub fn temp_wav_path(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("yapper-{prefix}-{nanos}.wav"))
}

/// Resolve configured mic to a Pulse source name. Empty → `"default"`.
pub fn resolve_mic_source(configured: &str) -> &str {
    let trimmed = configured.trim();
    if trimmed.is_empty() {
        "default"
    } else {
        trimmed
    }
}

/// Parse `pactl list sources short` output (tab-separated columns).
///
/// Columns: `index  name  driver  sample_spec  state`
pub fn parse_pactl_sources_short(text: &str) -> Vec<PulseSource> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 2 {
            // Some locales/tools use runs of spaces; try split_whitespace fallback
            let ws: Vec<&str> = line.split_whitespace().collect();
            if ws.len() < 2 {
                continue;
            }
            let index = ws[0].parse().unwrap_or(0);
            out.push(PulseSource {
                index,
                name: ws[1].to_string(),
                driver: ws.get(2).unwrap_or(&"").to_string(),
                sample_spec: ws.get(3).map(|s| s.to_string()).unwrap_or_default(),
                state: ws.get(4).map(|s| s.to_string()).unwrap_or_default(),
            });
            continue;
        }
        let index = cols[0].trim().parse().unwrap_or(0);
        out.push(PulseSource {
            index,
            name: cols[1].trim().to_string(),
            driver: cols.get(2).map(|s| s.trim().to_string()).unwrap_or_default(),
            sample_spec: cols
                .get(3)
                .map(|s| s.trim().to_string())
                .unwrap_or_default(),
            state: cols
                .get(4)
                .map(|s| s.trim().to_string())
                .unwrap_or_default(),
        });
    }
    out
}

/// Peak and RMS of signed 16-bit mono/interleaved PCM.
pub fn pcm_s16le_energy(samples: &[i16]) -> WavEnergy {
    if samples.is_empty() {
        return WavEnergy {
            frames: 0,
            peak: 0,
            rms: 0.0,
        };
    }
    let mut peak: i32 = 0;
    let mut sum_sq = 0.0f64;
    for &s in samples {
        let a = (s as i32).abs();
        if a > peak {
            peak = a;
        }
        let f = s as f64;
        sum_sq += f * f;
    }
    let peak_i16 = peak.min(i16::MAX as i32) as i16;
    let rms = (sum_sq / samples.len() as f64).sqrt();
    WavEnergy {
        frames: samples.len(),
        peak: peak_i16,
        rms,
    }
}

/// Extract PCM s16le samples from a RIFF/WAVE buffer (mono or multi-channel interleaved).
pub fn pcm_samples_from_wav_bytes(bytes: &[u8]) -> Result<Vec<i16>> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a RIFF/WAVE file");
    }
    let mut offset = 12usize;
    let mut data_range: Option<(usize, usize)> = None;
    let mut bits_per_sample: u16 = 16;
    let mut audio_format: u16 = 1;

    while offset + 8 <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap())
            as usize;
        let data_start = offset + 8;
        let data_end = data_start.saturating_add(chunk_size).min(bytes.len());
        if chunk_id == b"fmt " && chunk_size >= 16 && data_end >= data_start + 16 {
            audio_format = u16::from_le_bytes(bytes[data_start..data_start + 2].try_into().unwrap());
            bits_per_sample =
                u16::from_le_bytes(bytes[data_start + 14..data_start + 16].try_into().unwrap());
        } else if chunk_id == b"data" {
            data_range = Some((data_start, data_end));
            break;
        }
        // chunks are word-aligned
        offset = data_end + (chunk_size % 2);
        if chunk_size == 0 {
            break;
        }
    }

    let (start, end) = data_range.context("WAV missing data chunk")?;
    if audio_format != 1 {
        bail!("only PCM WAV supported (format={audio_format})");
    }
    if bits_per_sample != 16 {
        bail!("only 16-bit PCM supported (bits={bits_per_sample})");
    }
    let pcm = &bytes[start..end];
    if pcm.len() < 2 {
        return Ok(Vec::new());
    }
    // Drop trailing odd byte if present (incomplete write while recording).
    let usable = pcm.len() - (pcm.len() % 2);
    let mut samples = Vec::with_capacity(usable / 2);
    for i in (0..usable).step_by(2) {
        samples.push(i16::from_le_bytes([pcm[i], pcm[i + 1]]));
    }
    Ok(samples)
}

/// Energy of a WAV file on disk. Incomplete/growing files are OK if a data chunk exists.
pub fn wav_file_energy(path: &Path) -> Result<WavEnergy> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let samples = pcm_samples_from_wav_bytes(&bytes)?;
    Ok(pcm_s16le_energy(&samples))
}

/// Peak level in 0.0..=1.0 from a (possibly still-growing) WAV; `None` if unreadable.
pub fn wav_level_01(path: &Path) -> Option<f32> {
    let energy = wav_file_energy(path).ok()?;
    if energy.frames == 0 {
        return Some(0.0);
    }
    Some((energy.peak as f32 / 32768.0).clamp(0.0, 1.0))
}

/// List Pulse/PipeWire sources via `pactl` (includes monitors; prefer `list_capture_sources`).
pub fn list_pulse_sources() -> Result<Vec<PulseSource>> {
    if !which("pactl") {
        bail!("pactl not found (install pulseaudio-utils / pipewire-pulse)");
    }
    let output = Command::new("pactl")
        .args(["list", "sources", "short"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("run pactl list sources short")?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        bail!("pactl list sources short failed: {err}");
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_pactl_sources_short(&text))
}

/// Capture inputs only — no `*.monitor` / output-side sources (B22–B23).
pub fn list_capture_sources() -> Result<Vec<PulseSource>> {
    Ok(filter_capture_sources(list_pulse_sources()?))
}

/// Filter a source list to capture inputs (pure; unit-tested).
pub fn filter_capture_sources(sources: Vec<PulseSource>) -> Vec<PulseSource> {
    sources.into_iter().filter(|s| s.is_capture_input()).collect()
}

/// Current Pulse default source name, if available.
pub fn default_pulse_source() -> Result<Option<String>> {
    if !which("pactl") {
        bail!("pactl not found");
    }
    let output = Command::new("pactl")
        .args(["get-default-source"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("run pactl get-default-source")?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        bail!("pactl get-default-source failed: {err}");
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        Ok(None)
    } else {
        Ok(Some(name))
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
