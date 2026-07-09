//! Mic capture, Pulse/PipeWire source listing, playback, and level helpers.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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
    /// Short label for UI dropdowns.
    pub fn label(&self) -> String {
        if self.name.ends_with(".monitor") {
            format!("{} (monitor)", self.name)
        } else {
            self.name.clone()
        }
    }
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

/// List Pulse/PipeWire sources via `pactl`.
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

/// Capture sample rate used for STT-friendly mono PCM.
pub const CAPTURE_SAMPLE_RATE: u32 = 16_000;

/// In-flight mic capture (arecord → WAV). ffmpeg continuous-record+kill writes
/// 0 frames on PipeWire hosts; arecord grows the file and flushes on stop.
pub struct RecordingSession {
    child: Child,
    path: PathBuf,
}

impl RecordingSession {
    #[allow(dead_code)] // used by tests + future UI path reveal
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Live peak level 0.0..=1.0 from the growing WAV (works mid-record).
    pub fn level_01(&self) -> Option<f32> {
        wav_level_01(&self.path)
    }

    /// Stop capture, finalize WAV sizes, return output path.
    pub fn stop(mut self) -> Result<PathBuf> {
        stop_child(&mut self.child);
        // brief flush window for arecord to close the file
        std::thread::sleep(std::time::Duration::from_millis(50));
        finalize_pcm_wav_header(&self.path)?;
        if !self.path.is_file() {
            bail!("recording missing after stop: {}", self.path.display());
        }
        Ok(self.path)
    }
}

/// ALSA/Pulse device string for arecord (`pulse` or `pulse:<source>`).
pub fn arecord_device(source: &str) -> String {
    let resolved = resolve_mic_source(source);
    if resolved == "default" {
        "pulse".into()
    } else {
        format!("pulse:{resolved}")
    }
}

/// Start recording from a Pulse source to a mono 16 kHz PCM WAV.
///
/// Uses **arecord** (Pulse ALSA plugin). Continuous ffmpeg pulse capture leaves
/// empty files when killed without `-t` on this host class.
pub fn start_recording(out: &Path, source: &str) -> Result<RecordingSession> {
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let out_str = out
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("record path not UTF-8"))?;
    if !which("arecord") {
        bail!("arecord not found (install alsa-utils for mic capture)");
    }
    let device = arecord_device(source);
    // Remove stale file so growth starts clean.
    let _ = std::fs::remove_file(out);
    let child = Command::new("arecord")
        .args([
            "-D",
            &device,
            "-f",
            "S16_LE",
            "-r",
            &CAPTURE_SAMPLE_RATE.to_string(),
            "-c",
            "1",
            "-t",
            "wav",
            out_str,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn arecord -D {device}"))?;
    Ok(RecordingSession {
        child,
        path: out.to_path_buf(),
    })
}

/// Stop a session (same as [`RecordingSession::stop`]).
pub fn stop_recording(session: RecordingSession) -> Result<PathBuf> {
    session.stop()
}

fn stop_child(child: &mut Child) {
    // SIGINT lets arecord exit more cleanly than SIGKILL when available.
    #[cfg(unix)]
    {
        let pid = child.id();
        if pid > 0 {
            libc_kill(pid as i32, 2); // SIGINT
            // If still running shortly after, escalate.
            std::thread::sleep(std::time::Duration::from_millis(80));
            let _ = child.try_wait();
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn libc_kill(pid: i32, sig: i32) {
    // Avoid libc crate dep: raw kill(2).
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe {
        let _ = kill(pid, sig);
    }
}

/// Fix RIFF/data chunk sizes after arecord is interrupted (header often has
/// a huge placeholder size while PCM payload is already on disk).
pub fn finalize_pcm_wav_header(path: &Path) -> Result<()> {
    let mut bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a RIFF/WAVE file: {}", path.display());
    }
    // RIFF chunk size = file length − 8
    let riff_size = (bytes.len() as u32).saturating_sub(8);
    bytes[4..8].copy_from_slice(&riff_size.to_le_bytes());

    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let data_start = offset + 8;
        if id == b"data" {
            let pcm_len = bytes.len().saturating_sub(data_start);
            bytes[offset + 4..offset + 8].copy_from_slice(&(pcm_len as u32).to_le_bytes());
            // Update RIFF size again in case we only fixed data.
            let riff_size = (bytes.len() as u32).saturating_sub(8);
            bytes[4..8].copy_from_slice(&riff_size.to_le_bytes());
            std::fs::write(path, &bytes).with_context(|| format!("write {}", path.display()))?;
            return Ok(());
        }
        let data_end = data_start.saturating_add(size).min(bytes.len());
        offset = data_end + (size % 2);
        if size == 0 && id != b"data" {
            break;
        }
    }
    bail!("WAV missing data chunk: {}", path.display());
}

/// Record a fixed-duration probe using the **same** start/stop path as GUI/PTT.
pub fn record_probe(out: &Path, source: &str, duration_secs: f32) -> Result<()> {
    let session = start_recording(out, source)?;
    let ms = ((duration_secs.max(0.1)) * 1000.0) as u64;
    // Hard cap so probes never hang indefinitely.
    let ms = ms.min(10_000);
    std::thread::sleep(std::time::Duration::from_millis(ms));
    session.stop()?;
    let meta = std::fs::metadata(out).with_context(|| format!("stat {}", out.display()))?;
    if meta.len() < 1000 {
        bail!(
            "probe WAV too small ({} bytes) — check mic source / mute",
            meta.len()
        );
    }
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

    const PACTL_FIXTURE: &str = "\
63\talsa_output.pci-0000_00_1f.3.analog-stereo.monitor\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED
64\talsa_input.pci-0000_00_1f.3.analog-stereo\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED
65\talsa_input.usb-FuZhou_Kingwayinfo_CO._LTD_TONOR_TC30_Audio_Device_20200707-00.mono-fallback\tPipeWire\ts16le 1ch 48000Hz\tRUNNING
73\talsa_output.pci-0000_01_00.1.hdmi-stereo.monitor\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED
";

    #[test]
    fn temp_path_has_prefix() {
        let p = temp_wav_path("test");
        assert!(p.to_string_lossy().contains("yapper-test-"));
        assert!(p.extension().and_then(|e| e.to_str()) == Some("wav"));
    }

    #[test]
    fn parse_multi_source_pactl_short() {
        let sources = parse_pactl_sources_short(PACTL_FIXTURE);
        assert_eq!(sources.len(), 4);
        assert_eq!(sources[0].index, 63);
        assert!(sources[0].name.ends_with(".monitor"));
        assert_eq!(sources[2].index, 65);
        assert!(sources[2].name.contains("TONOR_TC30"));
        assert_eq!(sources[2].driver, "PipeWire");
        assert_eq!(sources[2].state, "RUNNING");
        assert!(sources[0].label().contains("(monitor)"));
        assert!(!sources[2].label().contains("(monitor)"));
    }

    #[test]
    fn parse_empty_and_whitespace_only() {
        assert!(parse_pactl_sources_short("").is_empty());
        assert!(parse_pactl_sources_short("\n\n  \n").is_empty());
    }

    #[test]
    fn parse_space_separated_fallback() {
        let text = "1 alsa_input.usb-mic PipeWire s16le 1ch 48000Hz RUNNING\n";
        let sources = parse_pactl_sources_short(text);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].name, "alsa_input.usb-mic");
    }

    #[test]
    fn resolve_mic_source_empty_is_default() {
        assert_eq!(resolve_mic_source(""), "default");
        assert_eq!(resolve_mic_source("  "), "default");
        assert_eq!(
            resolve_mic_source("alsa_input.usb-mic"),
            "alsa_input.usb-mic"
        );
    }

    #[test]
    fn pcm_energy_silence_and_tone() {
        let silence = vec![0i16; 1000];
        let e = pcm_s16le_energy(&silence);
        assert_eq!(e.peak, 0);
        assert!(e.rms < 0.001);
        assert!(!e.is_non_silence());

        let loud: Vec<i16> = (0..1000).map(|i| if i % 2 == 0 { 8000 } else { -8000 }).collect();
        let e = pcm_s16le_energy(&loud);
        assert!(e.peak >= 8000);
        assert!(e.rms > 1000.0);
        assert!(e.is_non_silence());
    }

    #[test]
    fn pcm_energy_empty() {
        let e = pcm_s16le_energy(&[]);
        assert_eq!(e.frames, 0);
        assert!(!e.is_non_silence());
    }

    #[test]
    fn wav_bytes_round_trip_energy() {
        // Minimal mono 16-bit PCM WAV: 4 samples [0, 1000, -1000, 500]
        let samples: [i16; 4] = [0, 1000, -1000, 500];
        let data = build_wav(&samples, /*bogus_data_size*/ None);
        let parsed = pcm_samples_from_wav_bytes(&data).unwrap();
        assert_eq!(parsed, samples);
        let e = pcm_s16le_energy(&parsed);
        assert_eq!(e.peak, 1000);
        assert!(e.is_non_silence());
    }

    fn build_wav(samples: &[i16], bogus_data_size: Option<u32>) -> Vec<u8> {
        let mut data = Vec::new();
        let data_chunk_size = samples.len() * 2;
        data.extend_from_slice(b"RIFF");
        data.extend_from_slice(&0u32.to_le_bytes()); // placeholder
        data.extend_from_slice(b"WAVE");
        data.extend_from_slice(b"fmt ");
        data.extend_from_slice(&16u32.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&16000u32.to_le_bytes());
        data.extend_from_slice(&32000u32.to_le_bytes());
        data.extend_from_slice(&2u16.to_le_bytes());
        data.extend_from_slice(&16u16.to_le_bytes());
        data.extend_from_slice(b"data");
        let size_field = bogus_data_size.unwrap_or(data_chunk_size as u32);
        data.extend_from_slice(&size_field.to_le_bytes());
        for s in samples {
            data.extend_from_slice(&s.to_le_bytes());
        }
        let riff_size = (data.len() as u32).saturating_sub(8);
        data[4..8].copy_from_slice(&riff_size.to_le_bytes());
        data
    }

    #[test]
    fn finalize_repairs_arecord_style_header() {
        let samples: Vec<i16> = (0..100).map(|i| (i * 30) as i16).collect();
        // arecord interrupted: data size field is a huge placeholder
        let bytes = build_wav(&samples, Some(0x8000_0000));
        let dir = std::env::temp_dir().join(format!(
            "yapper-wav-fix-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("broken.wav");
        std::fs::write(&path, &bytes).unwrap();
        finalize_pcm_wav_header(&path).unwrap();
        let fixed = std::fs::read(&path).unwrap();
        let parsed = pcm_samples_from_wav_bytes(&fixed).unwrap();
        assert_eq!(parsed.len(), samples.len());
        assert_eq!(parsed, samples);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn arecord_device_default_and_named() {
        assert_eq!(arecord_device(""), "pulse");
        assert_eq!(arecord_device("default"), "pulse");
        assert_eq!(
            arecord_device("alsa_input.usb-mic"),
            "pulse:alsa_input.usb-mic"
        );
    }

    /// Live path used by GUI/PTT: start_recording → sleep → stop_recording.
    /// Asserts non-empty PCM frames (energy may be low in a quiet room).
    #[test]
    fn start_stop_recording_writes_pcm_frames() {
        if !which("arecord") {
            eprintln!("skip: arecord missing");
            return;
        }
        if default_pulse_source().ok().flatten().is_none() && list_pulse_sources().ok().map(|s| s.is_empty()).unwrap_or(true)
        {
            eprintln!("skip: no pulse sources");
            return;
        }
        let out = temp_wav_path("start-stop-test");
        let session = match start_recording(&out, "") {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skip: start_recording failed: {e:#}");
                return;
            }
        };
        // Mid-record growth → level helper must see frames (or at least a growing file)
        std::thread::sleep(std::time::Duration::from_millis(400));
        let mid_size = std::fs::metadata(session.path()).map(|m| m.len()).unwrap_or(0);
        assert!(
            mid_size > 44,
            "WAV must grow during capture (got {mid_size} bytes) — live level depends on this"
        );
        let _mid_level = session.level_01();
        std::thread::sleep(std::time::Duration::from_millis(900));
        let path = session.stop().expect("stop_recording");
        assert_eq!(path, out);
        let meta = std::fs::metadata(&path).unwrap();
        assert!(
            meta.len() > 1000,
            "stopped WAV too small: {} bytes",
            meta.len()
        );
        let energy = wav_file_energy(&path).expect("energy");
        assert!(
            energy.frames > 8_000,
            "expected >0.5s of 16kHz mono frames, got {}",
            energy.frames
        );
        let _ = std::fs::remove_file(&path);
    }
}
