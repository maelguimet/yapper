//! Yapper — local tray STT/TTS shell (Whisper + Chatterbox).

mod app;
mod audio;
mod config;
mod hotkey_apply;
mod hotkeys;
mod ipc;
mod lifecycle;
mod mic;
mod mpv_backend;
mod policy;
mod python_paths;
mod segment;
mod textprep;
mod timeouts;
mod transport;
mod transport_machine;
mod tray;
mod ui;
mod wavutil;
mod workers;
mod x11util;

use clap::{Parser, Subcommand};
use config::Config;
use workers::{resolve_python_bin, resolve_python_root, worker_package_status};

#[derive(Parser, Debug)]
#[command(name = "yapper", about = "Local tray STT + TTS (Whisper + Chatterbox)")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Print paths and dependency checks
    Doctor,
    /// Print version
    Version,
    /// Run GUI (default)
    Gui,
    /// Write default config.toml if missing
    InitConfig,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Commands::Gui) {
        Commands::Version => {
            println!("yapper {}", env!("CARGO_PKG_VERSION"));
        }
        Commands::Doctor => {
            run_doctor()?;
        }
        Commands::InitConfig => {
            let path = Config::config_path();
            if path.is_file() {
                println!("exists: {}", path.display());
            } else {
                let mut cfg = Config::default();
                cfg.paths.python_root = resolve_python_root(&cfg).to_string_lossy().into();
                cfg.paths.python_bin = resolve_python_bin(&cfg);
                cfg.save(&path)?;
                println!("wrote {}", path.display());
            }
        }
        Commands::Gui => {
            app::run_gui()?;
        }
    }
    Ok(())
}

fn run_doctor() -> anyhow::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let data = dirs::data_local_dir()
        .unwrap_or_else(|| home.join(".local/share"))
        .join("yapper");
    let cfg_path = Config::config_path();
    let mut cfg = Config::load_or_default()?;
    cfg.paths.python_root = resolve_python_root(&cfg).to_string_lossy().into();
    cfg.paths.python_bin = resolve_python_bin(&cfg);

    // Prefer config [models] paths (what workers actually use); fall back to XDG data.
    let models_root = if cfg.models.dir.trim().is_empty() {
        data.join("models")
    } else {
        std::path::PathBuf::from(cfg.models.dir.trim())
    };
    let voices_root = if cfg.models.voices_dir.trim().is_empty() {
        data.join("voices")
    } else {
        std::path::PathBuf::from(cfg.models.voices_dir.trim())
    };

    println!("yapper doctor");
    println!("  version: {}", env!("CARGO_PKG_VERSION"));
    println!("  config: {}", cfg_path.display());
    println!("  data:   {}", data.display());
    println!("  models: {} (config models.dir)", models_root.display());
    println!(
        "  voices: {} (config models.voices_dir)",
        voices_root.display()
    );
    println!("  python_root: {}", cfg.paths.python_root);
    println!("  python_bin: {}", cfg.paths.python_bin);
    println!("  display: {:?}", std::env::var("DISPLAY").ok());
    println!("  session: {:?}", std::env::var("XDG_SESSION_TYPE").ok());

    for bin in [
        "ffmpeg", "ffplay", "mpv", "arecord", "xclip", "xdotool", "nvidia-smi",
        "python3", "espeak-ng", "pactl",
    ] {
        let ok = std::process::Command::new("which")
            .arg(bin)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        println!("  {bin}: {}", if ok { "ok" } else { "MISSING" });
    }
    let (xclip_ok, xdotool_ok) = crate::x11util::x11_tools_available();
    println!(
        "  x11 tools: xclip={} xdotool={} display={}",
        if xclip_ok { "ok" } else { "MISSING" },
        if xdotool_ok { "ok" } else { "MISSING" },
        if crate::x11util::display_available() {
            "yes"
        } else {
            "no"
        }
    );

    let tray_report = crate::tray::tray_host_diagnostics();
    println!("  tray host: {}", tray_report.summary_line());
    if !tray_report.looks_ready() {
        println!("  tray: NOT READY — {}", tray_report.hint);
    } else {
        println!(
            "  tray: libs look OK (icon still needs a StatusNotifier host in the session)"
        );
    }
    println!(
        "  always-on: close/minimize hide to tray; Quit only from tray menu (or confirmed Exit)"
    );
    for (name, sample) in crate::textprep::regression_fixtures() {
        let cleaned = crate::textprep::sanitize_for_tts(sample);
        println!(
            "  tts fixture {name}: raw_chars={} sanitized_chars={}",
            sample.chars().count(),
            cleaned.chars().count()
        );
    }

    // Tree under python_root and/or importable via python_bin site-packages.
    println!(
        "  yapper_stt package: {}",
        worker_package_status(&cfg.paths.python_bin, &cfg.paths.python_root, "yapper_stt")
    );
    println!(
        "  yapper_tts package: {}",
        worker_package_status(&cfg.paths.python_bin, &cfg.paths.python_root, "yapper_tts")
    );

    let small = models_root.join("whisper/small.pt");
    let medium = models_root.join("whisper/medium.pt");
    println!(
        "  whisper small: {}",
        if small.is_file() { "ok" } else { "missing" }
    );
    println!(
        "  whisper medium: {}",
        if medium.is_file() { "ok" } else { "missing" }
    );
    let n_voices = std::fs::read_dir(&voices_root)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .and_then(|x| x.to_str())
                        .map(|x| x.eq_ignore_ascii_case("wav"))
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);
    println!(
        "  voice refs: {n_voices} wav(s) in {}",
        voices_root.display()
    );

    doctor_mic_probe(&cfg);

    // Worker ping smoke (no model load); path env matches GUI WorkerManager.
    match crate::ipc::WorkerClient::spawn(
        "stt",
        &cfg.paths.python_bin,
        &cfg.paths.python_root,
        &cfg.models.dir,
        &cfg.models.voices_dir,
    ) {
        Ok(mut w) => match w.ping() {
            Ok(r) if r.ok => println!("  stt worker ping: ok"),
            Ok(r) => println!("  stt worker ping: fail {:?}", r.error),
            Err(e) => println!("  stt worker ping: error {e:#}"),
        },
        Err(e) => println!("  stt worker spawn: error {e:#}"),
    }
    match crate::ipc::WorkerClient::spawn(
        "tts",
        &cfg.paths.python_bin,
        &cfg.paths.python_root,
        &cfg.models.dir,
        &cfg.models.voices_dir,
    ) {
        Ok(mut w) => match w.ping() {
            Ok(r) if r.ok => println!("  tts worker ping: ok"),
            Ok(r) => println!("  tts worker ping: fail {:?}", r.error),
            Err(e) => println!("  tts worker ping: error {e:#}"),
        },
        Err(e) => println!("  tts worker spawn: error {e:#}"),
    }

    Ok(())
}

/// Report Pulse default source + short capture energy probe.
fn doctor_mic_probe(cfg: &Config) {
    println!("  mic config: {}", {
        let s = cfg.audio.mic_source.trim();
        if s.is_empty() {
            "(system default)"
        } else {
            s
        }
    });

    match crate::audio::default_pulse_source() {
        Ok(Some(name)) => println!("  pulse default source: {name}"),
        Ok(None) => println!("  pulse default source: (empty)"),
        Err(e) => {
            println!("  pulse default source: skip ({e:#})");
            println!("  mic probe: skipped (no pulse default source)");
            return;
        }
    }

    match crate::audio::list_pulse_sources() {
        Ok(list) => {
            println!("  pulse sources: {} listed", list.len());
            for s in list.iter().take(8) {
                println!("    - {} [{}]", s.name, s.state);
            }
            if list.len() > 8 {
                println!("    … {} more", list.len() - 8);
            }
        }
        Err(e) => println!("  pulse sources: skip ({e:#})"),
    }

    let source = crate::audio::resolve_mic_source(&cfg.audio.mic_source);
    let probe_path = crate::audio::temp_wav_path("doctor-probe");
    match crate::audio::record_probe(&probe_path, source, 1.0) {
        Ok(()) => match crate::audio::wav_file_energy(&probe_path) {
            Ok(e) => {
                let verdict = if e.is_non_silence() {
                    "energy OK (non-silence)"
                } else if e.frames == 0 {
                    "silence / empty (0 frames)"
                } else {
                    "silence (low energy — check mic, mute, or wrong source)"
                };
                println!(
                    "  mic probe: {verdict} (source={source}, peak={}, rms={:.1}, frames={}, file={})",
                    e.peak,
                    e.rms,
                    e.frames,
                    probe_path.display()
                );
            }
            Err(e) => println!("  mic probe: fail (energy read: {e:#})"),
        },
        Err(e) => println!("  mic probe: skipped/fail ({e:#})"),
    }
    let _ = std::fs::remove_file(&probe_path);
}
