//! X11 selection/clipboard/paste helpers via xclip + xdotool.

use anyhow::{anyhow, bail, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardSel {
    Clipboard,
    Primary,
}

impl ClipboardSel {
    fn xclip_flag(self) -> &'static str {
        match self {
            ClipboardSel::Clipboard => "clipboard",
            ClipboardSel::Primary => "primary",
        }
    }
}

pub fn read_selection(sel: ClipboardSel) -> Result<String> {
    let out = Command::new("xclip")
        .args(["-selection", sel.xclip_flag(), "-o"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("run xclip -o")?;
    if !out.status.success() {
        // empty selection often non-zero; treat as empty
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Write text into CLIPBOARD or PRIMARY (xclip -i).
pub fn write_selection(sel: ClipboardSel, text: &str) -> Result<()> {
    let mut child = Command::new("xclip")
        .args(["-selection", sel.xclip_flag(), "-i"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn xclip -i")?;
    {
        let stdin = child.stdin.as_mut().ok_or_else(|| anyhow!("xclip stdin"))?;
        stdin.write_all(text.as_bytes())?;
    }
    let status = child.wait()?;
    if !status.success() {
        bail!("xclip write ({}) failed: {status}", sel.xclip_flag());
    }
    Ok(())
}

pub fn write_clipboard(text: &str) -> Result<()> {
    write_selection(ClipboardSel::Clipboard, text)
}

/// Paste at focused window via clipboard + ctrl+v (Unicode-safe).
///
/// Returns after xdotool reports success. Callers that need inject proof
/// should focus a window that accepts paste (see tests).
pub fn paste_at_cursor(text: &str) -> Result<()> {
    write_clipboard(text)?;
    // brief delay so target app sees clipboard
    std::thread::sleep(std::time::Duration::from_millis(30));
    let status = Command::new("xdotool")
        .args(["key", "--clearmodifiers", "ctrl+v"])
        .status()
        .context("xdotool key ctrl+v")?;
    if !status.success() {
        bail!("xdotool paste failed: {status}");
    }
    Ok(())
}

pub fn x11_tools_available() -> (bool, bool) {
    let xclip = Command::new("which")
        .arg("xclip")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let xdotool = Command::new("which")
        .arg("xdotool")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    (xclip, xdotool)
}

pub fn display_available() -> bool {
    std::env::var("DISPLAY").map(|d| !d.is_empty()).unwrap_or(false)
}

/// True if Super (X11 Mod4) is currently held.
///
/// egui-winit drops Super on non-macOS (`mac_cmd` is always false; `command` is
/// Ctrl). Hotkey Capture must call this so Super+Shift+S is not stored as Shift+S.
///
/// Returns `false` when DISPLAY is missing, libX11 cannot open, or the query fails
/// (callers may warn rather than silently treating that as "user released Super").
pub fn super_modifier_down() -> bool {
    match query_super_modifier_down() {
        Ok(v) => v,
        Err(_) => false,
    }
}

/// Result-bearing Super/Mod4 query for tests and diagnostics.
pub fn query_super_modifier_down() -> Result<bool> {
    #[cfg(target_os = "linux")]
    {
        query_x11_mod4_down()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(false)
    }
}

/// X11: Super is almost always Mod4 on GNOME/Pop/Ubuntu desktops.
#[cfg(target_os = "linux")]
fn query_x11_mod4_down() -> Result<bool> {
    use std::os::raw::{c_int, c_uint};
    use x11_dl::xlib::{self, Display, Mod4Mask, Window};

    if !display_available() {
        bail!("no DISPLAY for Super/Mod4 query");
    }
    let xlib = xlib::Xlib::open().context("load libX11 for Super/Mod4 query")?;
    unsafe {
        let dpy: *mut Display = (xlib.XOpenDisplay)(std::ptr::null());
        if dpy.is_null() {
            bail!("XOpenDisplay failed (Super/Mod4 query)");
        }
        let root: Window = (xlib.XDefaultRootWindow)(dpy);
        let mut root_ret: Window = 0;
        let mut child_ret: Window = 0;
        let mut root_x: c_int = 0;
        let mut root_y: c_int = 0;
        let mut win_x: c_int = 0;
        let mut win_y: c_int = 0;
        let mut mask: c_uint = 0;
        let ok = (xlib.XQueryPointer)(
            dpy,
            root,
            &mut root_ret,
            &mut child_ret,
            &mut root_x,
            &mut root_y,
            &mut win_x,
            &mut win_y,
            &mut mask,
        );
        (xlib.XCloseDisplay)(dpy);
        if ok == 0 {
            bail!("XQueryPointer failed (Super/Mod4 query)");
        }
        let _ = (root_ret, child_ret, root_x, root_y, win_x, win_y);
        Ok((mask & Mod4Mask as c_uint) != 0)
    }
}

/// Hold-to-talk insert path: put transcript on clipboard and paste (no Enter).
pub fn insert_transcript_at_cursor(text: &str, also_keep_clipboard: bool) -> Result<()> {
    paste_at_cursor(text)?;
    if !also_keep_clipboard {
        // leave clipboard as-is (paste already wrote it); nothing to clear
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command as StdCommand, Stdio as ProcStdio};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    /// Serialize X11 tests (selection is a global resource per display).
    fn x11_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn scratch_dir() -> PathBuf {
        if let Ok(p) = std::env::var("YAPPER_SCRATCH") {
            return PathBuf::from(p);
        }
        PathBuf::from("/tmp/grok-goal-29cc0bace209/implementer")
    }

    fn repo_root() -> PathBuf {
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
    struct IsolatedX {
        /// Xvfb display name (e.g. `:97`) kept for diagnostics/logs.
        #[allow(dead_code)]
        display: String,
        xvfb: Child,
        wm: Option<Child>,
        prev_display: Option<String>,
        prev_ld: Option<String>,
        prev_home: Option<String>,
    }

    impl IsolatedX {
        fn start() -> Option<Self> {
            if !which_bin("Xvfb") || !tools_ok() {
                return None;
            }
            // Pick a free display (avoid collisions with leftover Xvfb).
            let mut xvfb = None;
            let mut display = String::new();
            for n in 110..160 {
                let d = format!(":{n}");
                let sock = format!("/tmp/.X11-unix/X{n}");
                if Path::new(&sock).exists() {
                    continue;
                }
                match StdCommand::new("Xvfb")
                    .args([&d, "-screen", "0", "1280x720x24", "-nolisten", "tcp"])
                    .stdout(ProcStdio::null())
                    .stderr(ProcStdio::null())
                    .spawn()
                {
                    Ok(child) => {
                        display = d;
                        xvfb = Some(child);
                        break;
                    }
                    Err(_) => continue,
                }
            }
            let xvfb = xvfb?;
            std::thread::sleep(Duration::from_millis(350));
            let prev_display = std::env::var("DISPLAY").ok();
            let prev_ld = std::env::var("LD_LIBRARY_PATH").ok();
            let prev_home = std::env::var("HOME").ok();
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
                    // themes
                    let themes = scratch_dir().join("debroot/usr/share/themes");
                    if themes.is_dir() {
                        let home = scratch_dir().join("obhome");
                        let _ = fs::create_dir_all(home.join(".themes"));
                        let _ = fs::create_dir_all(home.join(".config/openbox"));
                        // best-effort copy themes once
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
                display,
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
    fn assert_injected_via_sink(
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

        // Wait until sink file exists (widget up)
        let ready_deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < ready_deadline {
            if out.is_file() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(out.is_file(), "sink never created {}", out.display());

        // Focus sink window (openbox provides EWMH when available)
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
        // Click into text area as belt-and-suspenders focus
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

    #[test]
    fn tools_detection_does_not_panic() {
        let _ = x11_tools_available();
        let _ = display_available();
        let _ = super_modifier_down();
    }

    #[test]
    fn super_modifier_query_when_display() {
        // Read-only query; safe on live DISPLAY (does not steal focus/selection).
        if !display_available() {
            let _iso = match IsolatedX::start() {
                Some(x) => x,
                None => {
                    eprintln!("skip super_modifier_query: no DISPLAY/Xvfb");
                    return;
                }
            };
            let down = query_super_modifier_down().expect("query Super on isolated X");
            assert!(!down, "idle Xvfb should not report Super held");
            return;
        }
        // Live or any session: API must succeed and return a bool (Super not held while test runs).
        let down = query_super_modifier_down().expect("query Super/Mod4 via XQueryPointer");
        // We cannot force Super up without injecting keys; only assert API works.
        let _ = down;
        let _ = super_modifier_down();
    }

    #[test]
    fn clipboard_round_trip_when_display() {
        let _guard = x11_lock();
        let _iso = match IsolatedX::start() {
            Some(x) => x,
            None => {
                eprintln!("skip clipboard: no Xvfb/tools");
                return;
            }
        };
        let marker = format!("yapper-clipboard-{}", std::process::id());
        write_clipboard(&marker).expect("write_clipboard");
        let got = read_selection(ClipboardSel::Clipboard).expect("read clipboard");
        assert_eq!(got, marker, "CLIPBOARD round-trip via write_clipboard/read_selection");
    }

    #[test]
    fn primary_selection_round_trip_when_display() {
        let _guard = x11_lock();
        let _iso = match IsolatedX::start() {
            Some(x) => x,
            None => {
                eprintln!("skip primary: no Xvfb/tools");
                return;
            }
        };
        let marker = format!("yapper-primary-{}", std::process::id());
        write_selection(ClipboardSel::Primary, &marker).expect("write PRIMARY");
        let got = read_selection(ClipboardSel::Primary).expect("read PRIMARY");
        assert_eq!(
            got, marker,
            "PRIMARY selection must round-trip (read-aloud default source)"
        );
    }

    #[test]
    fn paste_at_cursor_injects_text_into_focused_sink() {
        let _guard = x11_lock();
        let _iso = match IsolatedX::start() {
            Some(x) => x,
            None => {
                eprintln!("skip paste inject: no Xvfb/tools");
                return;
            }
        };
        let marker = format!("yapper-paste-{}", std::process::id());
        assert_injected_via_sink(&marker, "paste-inject-proof.txt", |m| paste_at_cursor(m));
        // Also confirm clipboard left as product path does
        let clip = read_selection(ClipboardSel::Clipboard).expect("clipboard");
        assert_eq!(clip, marker);
    }

    #[test]
    fn insert_transcript_injects_text_into_focused_sink() {
        let _guard = x11_lock();
        let _iso = match IsolatedX::start() {
            Some(x) => x,
            None => {
                eprintln!("skip insert inject: no Xvfb/tools");
                return;
            }
        };
        // Prefer real STT transcript from smoke when present
        let transcript_path = scratch_dir().join("hold-to-talk-transcript.txt");
        let from_file = fs::read_to_string(&transcript_path)
            .unwrap_or_default()
            .trim()
            .to_string();
        let require = std::env::var("YAPPER_REQUIRE_TRANSCRIPT").ok().as_deref() == Some("1");
        if require {
            assert!(
                !from_file.is_empty(),
                "YAPPER_REQUIRE_TRANSCRIPT=1 but {} empty/missing",
                transcript_path.display()
            );
        }
        let marker = if from_file.is_empty() {
            format!("yapper-insert-{}", std::process::id())
        } else {
            from_file
        };
        assert_injected_via_sink(&marker, "insert-transcript-proof.txt", |m| {
            insert_transcript_at_cursor(m, true)
        });
    }

    /// Select→speak data path: PRIMARY write/read (shipped) + marker for smoke log.
    #[test]
    fn primary_is_readable_for_read_aloud_source() {
        let _guard = x11_lock();
        let _iso = match IsolatedX::start() {
            Some(x) => x,
            None => {
                eprintln!("skip primary read-aloud path: no Xvfb/tools");
                return;
            }
        };
        let marker = format!("Yapper read aloud {}", std::process::id());
        write_selection(ClipboardSel::Primary, &marker).expect("write primary");
        let got = read_selection(ClipboardSel::Primary).expect("read primary");
        assert_eq!(got, marker);
        let path = scratch_dir().join("primary-read-aloud.txt");
        let _ = fs::create_dir_all(scratch_dir());
        let _ = fs::write(&path, format!("primary_ok={got}\n"));
    }

    #[allow(dead_code)]
    fn _path_used_for_lint(p: &Path) -> bool {
        p.exists()
    }
}
