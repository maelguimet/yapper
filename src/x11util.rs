//! X11 selection/clipboard/paste helpers via xclip + xdotool.

use anyhow::{anyhow, bail, Context, Result};
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
        let err = String::from_utf8_lossy(&out.stderr);
        // empty selection often non-zero; treat as empty
        if err.to_lowercase().contains("error") && !err.is_empty() {
            // still return empty for missing selection
        }
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn write_clipboard(text: &str) -> Result<()> {
    let mut child = Command::new("xclip")
        .args(["-selection", "clipboard", "-i"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn xclip -i")?;
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().ok_or_else(|| anyhow!("xclip stdin"))?;
        stdin.write_all(text.as_bytes())?;
    }
    let status = child.wait()?;
    if !status.success() {
        bail!("xclip write failed: {status}");
    }
    Ok(())
}

/// Paste at focused window via clipboard + ctrl+v (Unicode-safe).
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

#[allow(dead_code)]
pub fn x11_tools_available() -> (bool, bool) {
    let xclip = Command::new("which")
        .arg("xclip")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let xdotool = Command::new("which")
        .arg("xdotool")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    (xclip, xdotool)
}

#[allow(dead_code)]
pub fn display_available() -> bool {
    std::env::var("DISPLAY").map(|d| !d.is_empty()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_detection_does_not_panic() {
        let _ = x11_tools_available();
        let _ = display_available();
    }

    #[test]
    fn clipboard_round_trip_when_display() {
        if !display_available() {
            eprintln!("skip clipboard: no DISPLAY");
            return;
        }
        let (xclip, _) = x11_tools_available();
        if !xclip {
            eprintln!("skip clipboard: no xclip");
            return;
        }
        let marker = format!("yapper-x11-test-{}", std::process::id());
        write_clipboard(&marker).expect("write");
        let got = read_selection(ClipboardSel::Clipboard).expect("read");
        assert_eq!(got, marker);
    }
}
