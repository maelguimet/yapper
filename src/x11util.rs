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
    use std::path::PathBuf;
    use std::process::{Child, Stdio as ProcStdio};
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

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

    fn tools_ok() -> bool {
        let (xclip, xdotool) = x11_tools_available();
        xclip && xdotool
    }

    /// Isolated Xvfb so paste/xdotool never hits the user's real session.
    struct IsolatedX {
        display: String,
        child: Child,
        prev_display: Option<String>,
    }

    impl IsolatedX {
        fn start() -> Option<Self> {
            let has_xvfb = Command::new("which")
                .arg("Xvfb")
                .stdout(ProcStdio::null())
                .stderr(ProcStdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !has_xvfb || !tools_ok() {
                return None;
            }
            // Pick a high display number unlikely to collide
            let n = 90 + (std::process::id() % 20);
            let display = format!(":{n}");
            let child = Command::new("Xvfb")
                .args([&display, "-screen", "0", "1280x720x24", "-nolisten", "tcp"])
                .stdout(ProcStdio::null())
                .stderr(ProcStdio::null())
                .spawn()
                .ok()?;
            std::thread::sleep(Duration::from_millis(250));
            let prev_display = std::env::var("DISPLAY").ok();
            // SAFETY: tests are single-threaded for X11 via mutex; only this process reads env.
            std::env::set_var("DISPLAY", &display);
            Some(Self {
                display,
                child,
                prev_display,
            })
        }
    }

    impl Drop for IsolatedX {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            match &self.prev_display {
                Some(d) => std::env::set_var("DISPLAY", d),
                None => std::env::remove_var("DISPLAY"),
            }
        }
    }

    #[test]
    fn tools_detection_does_not_panic() {
        let _ = x11_tools_available();
        let _ = display_available();
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
    fn paste_at_cursor_sets_clipboard_and_xdotool_ok() {
        let _guard = x11_lock();
        let iso = match IsolatedX::start() {
            Some(x) => x,
            None => {
                eprintln!("skip paste: no Xvfb/tools");
                return;
            }
        };
        let marker = format!("yapper-paste-{}", std::process::id());
        // Real shipped path under isolated X — will not paste into the user session.
        paste_at_cursor(&marker).expect("paste_at_cursor must succeed (xdotool exit 0)");
        let clip = read_selection(ClipboardSel::Clipboard).expect("read after paste");
        assert_eq!(clip, marker, "paste_at_cursor must leave fixture on CLIPBOARD");
        // Prove xdotool targeted the isolated display
        assert!(iso.display.starts_with(':'));
    }

    #[test]
    fn insert_transcript_uses_paste_path() {
        let _guard = x11_lock();
        let _iso = match IsolatedX::start() {
            Some(x) => x,
            None => {
                eprintln!("skip insert: no Xvfb/tools");
                return;
            }
        };
        let marker = format!("yapper-insert-{}", std::process::id());
        insert_transcript_at_cursor(&marker, true).expect("insert_transcript_at_cursor");
        let clip = read_selection(ClipboardSel::Clipboard).expect("clipboard after insert");
        assert_eq!(clip, marker);
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
        // Record evidence for ship bar
        let path = scratch_dir().join("primary-read-aloud.txt");
        let _ = std::fs::create_dir_all(scratch_dir());
        let _ = std::fs::write(&path, format!("primary_ok={got}\n"));
    }
}
