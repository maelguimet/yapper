//! Xvfb isolation harness and paste-sink helper for X11 tests.

use super::super::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as StdCommand, Stdio as ProcStdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Serialize X11 tests (selection is a global resource per display).
pub(super) fn x11_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

pub(super) fn scratch_dir() -> PathBuf {
    if let Ok(p) = std::env::var("YAPPER_SCRATCH") {
        return PathBuf::from(p);
    }
    PathBuf::from("/tmp/grok-goal-18fa6167e124/implementer")
}

pub(super) fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn tools_ok() -> bool {
    let (xclip, xdotool) = x11_tools_available();
    xclip && xdotool
}

fn which_bin(name: &str) -> bool {
    StdCommand::new("which")
        .arg(name)
        .stdout(ProcStdio::null())
        .stderr(ProcStdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Openbox from extracted deb under scratch (no root install).
fn openbox_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("YAPPER_OPENBOX") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    let candidate = scratch_dir().join("debroot/usr/bin/openbox");
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

fn openbox_lib_dir() -> Option<PathBuf> {
    let d = scratch_dir().join("debroot/usr/lib/x86_64-linux-gnu");
    if d.is_dir() {
        Some(d)
    } else {
        None
    }
}

/// Isolated Xvfb + optional openbox so paste never hits the user session.
pub(super) struct IsolatedX {
    xvfb: Child,
    wm: Option<Child>,
    prev_display: Option<String>,
    prev_ld: Option<String>,
    prev_home: Option<String>,
}

impl IsolatedX {
    pub(super) fn start() -> Option<Self> {
        if !which_bin("Xvfb") || !tools_ok() {
            return None;
        }
        // Pick a free display (avoid collisions with leftover Xvfb / stale sockets).
        let mut xvfb = None;
        let mut display = String::new();
        for n in 110..300 {
            let d = format!(":{n}");
            let sock = format!("/tmp/.X11-unix/X{n}");
            if Path::new(&sock).exists() {
                // Stale socket with no live process: drop it so the slot is reusable.
                let lock = format!("/tmp/.X{n}-lock");
                let live = StdCommand::new("pgrep")
                    .args(["-f", &format!("Xvfb {d}")])
                    .stdout(ProcStdio::null())
                    .stderr(ProcStdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if !live {
                    let _ = fs::remove_file(&sock);
                    let _ = fs::remove_file(&lock);
                } else {
                    continue;
                }
            }
            match StdCommand::new("Xvfb")
                .args([&d, "-screen", "0", "1280x720x24", "-nolisten", "tcp"])
                .stdout(ProcStdio::null())
                .stderr(ProcStdio::null())
                .spawn()
            {
                Ok(mut child) => {
                    // Bind failure exits immediately; only accept live Xvfb.
                    std::thread::sleep(Duration::from_millis(150));
                    match child.try_wait() {
                        Ok(Some(_)) => continue, // exited
                        Ok(None) if Path::new(&sock).exists() => {
                            display = d;
                            xvfb = Some(child);
                            break;
                        }
                        Ok(None) => {
                            let _ = child.kill();
                            let _ = child.wait();
                            continue;
                        }
                        Err(_) => {
                            let _ = child.kill();
                            let _ = child.wait();
                            continue;
                        }
                    }
                }
                Err(_) => continue,
            }
        }
        let xvfb = xvfb?;
        std::thread::sleep(Duration::from_millis(200));
        let prev_display = std::env::var("DISPLAY").ok();
        let prev_ld = std::env::var("LD_LIBRARY_PATH").ok();
        let prev_home = std::env::var("HOME").ok();
        // Isolated display for this test only; restored on Drop.
        std::env::set_var("DISPLAY", &display);

        // Start openbox when available (EWMH for xdotool windowactivate).
        let mut wm = None;
        if let Some(ob) = openbox_bin() {
            if let Some(lib) = openbox_lib_dir() {
                let mut ld = lib.display().to_string();
                if let Some(ref prev) = prev_ld {
                    ld = format!("{ld}:{prev}");
                }
                std::env::set_var("LD_LIBRARY_PATH", &ld);
                let themes = scratch_dir().join("debroot/usr/share/themes");
                if themes.is_dir() {
                    let home = scratch_dir().join("obhome");
                    let _ = fs::create_dir_all(home.join(".themes"));
                    let _ = fs::create_dir_all(home.join(".config/openbox"));
                    let _ = StdCommand::new("cp")
                        .args(["-a"])
                        .arg(format!("{}/.", themes.display()))
                        .arg(home.join(".themes"))
                        .status();
                    let rc = home.join(".config/openbox/rc.xml");
                    if !rc.is_file() {
                        let _ = fs::write(
                            &rc,
                            r#"<?xml version="1.0"?>
<openbox_config xmlns="http://openbox.org/3.4/rc">
  <theme><name>Clearlooks</name></theme>
</openbox_config>
"#,
                        );
                    }
                    std::env::set_var("HOME", home.display().to_string());
                }
            }
            if let Ok(child) = StdCommand::new(&ob)
                .stdout(ProcStdio::null())
                .stderr(ProcStdio::null())
                .spawn()
            {
                wm = Some(child);
                std::thread::sleep(Duration::from_millis(400));
            }
        }

        Some(Self {
            xvfb,
            wm,
            prev_display,
            prev_ld,
            prev_home,
        })
    }
}

impl Drop for IsolatedX {
    fn drop(&mut self) {
        if let Some(ref mut w) = self.wm {
            let _ = w.kill();
            let _ = w.wait();
        }
        let _ = self.xvfb.kill();
        let _ = self.xvfb.wait();
        match &self.prev_display {
            Some(d) => std::env::set_var("DISPLAY", d),
            None => std::env::remove_var("DISPLAY"),
        }
        match &self.prev_ld {
            Some(d) => std::env::set_var("LD_LIBRARY_PATH", d),
            None => std::env::remove_var("LD_LIBRARY_PATH"),
        }
        match &self.prev_home {
            Some(d) => std::env::set_var("HOME", d),
            None => {}
        }
    }
}

/// Spawn Tk sink, paste via shipped `paste_fn`, assert OUT contains marker.
/// Writes evidence to `proof_name` under scratch (unique per path to avoid races).
pub(super) fn assert_injected_via_sink(
    marker: &str,
    proof_name: &str,
    paste_fn: impl FnOnce(&str) -> Result<()>,
) {
    let sink_py = repo_root().join("scripts/x11_paste_sink.py");
    assert!(
        sink_py.is_file(),
        "missing paste sink script {}",
        sink_py.display()
    );
    let out = scratch_dir().join(format!("paste-inject-{}.txt", std::process::id()));
    let _ = fs::remove_file(&out);
    let _ = fs::create_dir_all(scratch_dir());

    let mut sink = StdCommand::new("python3")
        .arg(&sink_py)
        .arg(&out)
        .stdout(ProcStdio::null())
        .stderr(ProcStdio::null())
        .spawn()
        .expect("spawn x11_paste_sink.py");

    let ready_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < ready_deadline {
        if out.is_file() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(out.is_file(), "sink never created {}", out.display());

    let _ = StdCommand::new("xdotool")
        .args([
            "search",
            "--sync",
            "--name",
            "yapper-paste-sink",
            "windowactivate",
            "--sync",
            "windowfocus",
            "--sync",
        ])
        .status();
    if let Ok(outp) = StdCommand::new("xdotool")
        .args(["search", "--name", "yapper-paste-sink"])
        .output()
    {
        if let Some(wid) = String::from_utf8_lossy(&outp.stdout)
            .lines()
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let _ = StdCommand::new("xdotool")
                .args(["mousemove", "--window", wid, "80", "80", "click", "1"])
                .status();
        }
    }
    std::thread::sleep(Duration::from_millis(150));

    paste_fn(marker).expect("paste path failed");

    let deadline = Instant::now() + Duration::from_secs(4);
    let mut body = String::new();
    while Instant::now() < deadline {
        body = fs::read_to_string(&out).unwrap_or_default();
        if body.contains(marker) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = sink.kill();
    let _ = sink.wait();

    let proof = scratch_dir().join(proof_name);
    let _ = fs::write(
        &proof,
        format!("marker={marker}\nbody={body}\nout={}\n", out.display()),
    );

    assert!(
        body.contains(marker),
        "expected injected text {marker:?} in {} got {body:?}",
        out.display()
    );
}
