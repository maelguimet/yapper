//! Yapper — local tray STT/TTS shell (stub).
//!
//! Real GUI/tray/hotkeys land in Phase 2. See AGENTS.md and TODO.md.

use clap::{Parser, Subcommand};

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
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Commands::Version) {
        Commands::Version => {
            println!("yapper {}", env!("CARGO_PKG_VERSION"));
            println!("stub build — see TODO.md");
        }
        Commands::Doctor => {
            run_doctor()?;
        }
    }
    Ok(())
}

fn run_doctor() -> anyhow::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let data = dirs::data_local_dir()
        .unwrap_or_else(|| home.join(".local/share"))
        .join("yapper");
    let cfg = dirs::config_dir()
        .unwrap_or_else(|| home.join(".config"))
        .join("yapper");

    println!("yapper doctor");
    println!("  config: {}", cfg.display());
    println!("  data:   {}", data.display());
    println!("  models: {}", data.join("models").display());
    println!("  voices: {}", data.join("voices").display());
    println!("  display: {:?}", std::env::var("DISPLAY").ok());
    println!("  session: {:?}", std::env::var("XDG_SESSION_TYPE").ok());

    for bin in ["ffmpeg", "xclip", "xdotool", "nvidia-smi", "python3"] {
        let ok = std::process::Command::new("which")
            .arg(bin)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        println!("  {bin}: {}", if ok { "ok" } else { "MISSING" });
    }
    Ok(())
}
