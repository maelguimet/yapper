//! Mic capture, Pulse/PipeWire source listing, playback, and level helpers.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

// Pulse sources, labels, and WAV energy helpers live in `mic`.
pub use crate::mic::{
    default_pulse_source, human_mic_label, list_capture_sources, list_pulse_sources,
    resolve_mic_source, temp_wav_path, wav_file_energy, wav_level_01, PulseSource,
};

// Test-facing re-exports (and API stability for callers using audio::).
#[cfg(test)]
pub use crate::mic::{
    filter_capture_sources, is_capture_source_name, parse_pactl_sources_short, pcm_s16le_energy,
    pcm_samples_from_wav_bytes,
};

/// Capture sample rate used for STT-friendly mono PCM.
pub const CAPTURE_SAMPLE_RATE: u32 = 16_000;

/// In-flight mic capture (arecord → WAV). ffmpeg continuous-record+kill writes
/// 0 frames on PipeWire hosts; arecord grows the file and flushes on stop.
pub struct RecordingSession {
    child: Child,
    path: PathBuf,
}

impl RecordingSession {
    /// Mid-record path for tests that assert WAV growth while capture is live.
    #[cfg(test)]
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

pub fn kill_child_process(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
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
    use std::time::{SystemTime, UNIX_EPOCH};

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

    #[test]
    fn filter_capture_drops_monitors() {
        let sources = parse_pactl_sources_short(PACTL_FIXTURE);
        let caps = filter_capture_sources(sources);
        assert!(caps.iter().all(|s| s.is_capture_input()));
        assert!(caps.iter().all(|s| !s.name.ends_with(".monitor")));
        assert_eq!(caps.len(), 2, "two real inputs in fixture: {caps:?}");
        assert!(caps.iter().any(|s| s.name.contains("TONOR")));
    }

    #[test]
    fn human_label_prefers_product_name() {
        let raw = "alsa_input.usb-FuZhou_Kingwayinfo_CO._LTD_TONOR_TC30_Audio_Device_20200707-00.mono-fallback";
        let label = human_mic_label(raw);
        assert!(
            label.to_ascii_lowercase().contains("tonor")
                || label.to_ascii_lowercase().contains("tc30"),
            "expected product-ish label, got {label}"
        );
        assert!(!label.starts_with("alsa_input"), "{label}");
    }

    #[test]
    fn capture_name_filter_edges() {
        assert!(is_capture_source_name("alsa_input.usb-mic"));
        assert!(!is_capture_source_name(
            "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor"
        ));
        assert!(!is_capture_source_name(""));
    }
}
