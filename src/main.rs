//! Yapper — local tray STT/TTS shell (Whisper + Chatterbox).

mod app;
mod audio;
mod config;
mod hotkeys;
mod ipc;
mod policy;
mod tray;
mod workers;
mod x11util;

use clap::{Parser, Subcommand};
use config::Config;
use workers::{resolve_python_bin, resolve_python_root};

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

    println!("yapper doctor");
    println!("  version: {}", env!("CARGO_PKG_VERSION"));
    println!("  config: {}", cfg_path.display());
    println!("  data:   {}", data.display());
    println!("  models: {}", data.join("models").display());
    println!("  voices: {}", data.join("voices").display());
    println!("  python_root: {}", cfg.paths.python_root);
    println!("  python_bin: {}", cfg.paths.python_bin);
    println!("  display: {:?}", std::env::var("DISPLAY").ok());
    println!("  session: {:?}", std::env::var("XDG_SESSION_TYPE").ok());

    for bin in [
        "ffmpeg", "xclip", "xdotool", "nvidia-smi", "python3", "espeak-ng",
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

    let stt_mod = std::path::Path::new(&cfg.paths.python_root).join("yapper_stt");
    let tts_mod = std::path::Path::new(&cfg.paths.python_root).join("yapper_tts");
    println!(
        "  yapper_stt package: {}",
        if stt_mod.is_dir() { "ok" } else { "MISSING" }
    );
    println!(
        "  yapper_tts package: {}",
        if tts_mod.is_dir() { "ok" } else { "MISSING" }
    );

    let small = data.join("models/whisper/small.pt");
    let medium = data.join("models/whisper/medium.pt");
    println!(
        "  whisper small: {}",
        if small.is_file() { "ok" } else { "missing" }
    );
    println!(
        "  whisper medium: {}",
        if medium.is_file() { "ok" } else { "missing" }
    );
    let voices = data.join("voices");
    let n_voices = std::fs::read_dir(&voices)
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
    println!("  voice refs: {n_voices} wav(s) in {}", voices.display());

    // Worker ping smoke (no model load)
    match crate::ipc::WorkerClient::spawn("stt", &cfg.paths.python_bin, &cfg.paths.python_root) {
        Ok(mut w) => match w.ping() {
            Ok(r) if r.ok => println!("  stt worker ping: ok"),
            Ok(r) => println!("  stt worker ping: fail {:?}", r.error),
            Err(e) => println!("  stt worker ping: error {e:#}"),
        },
        Err(e) => println!("  stt worker spawn: error {e:#}"),
    }
    match crate::ipc::WorkerClient::spawn("tts", &cfg.paths.python_bin, &cfg.paths.python_root) {
        Ok(mut w) => match w.ping() {
            Ok(r) if r.ok => println!("  tts worker ping: ok"),
            Ok(r) => println!("  tts worker ping: fail {:?}", r.error),
            Err(e) => println!("  tts worker ping: error {e:#}"),
        },
        Err(e) => println!("  tts worker spawn: error {e:#}"),
    }

    Ok(())
}
