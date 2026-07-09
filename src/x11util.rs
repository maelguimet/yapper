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

/// How long to wait after ctrl+v before restoring CLIPBOARD so the target
/// app can finish reading the selection. Best-effort only — see design docs.
const CLIPBOARD_RESTORE_SETTLE: std::time::Duration = std::time::Duration::from_millis(50);

/// Ordered steps for hold-to-dictate clipboard insert (pure; unit-tested).
///
/// Paste always uses CLIPBOARD + ctrl+v. When `also_keep_clipboard` is false,
/// the plan includes a restore of the prior CLIPBOARD after paste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardInsertStep {
    SavePriorClipboard,
    WriteTranscriptAndPaste,
    RestorePriorClipboard,
}

/// Pure command sequence for insert: save → paste → optional restore.
pub fn clipboard_insert_plan(also_keep_clipboard: bool) -> &'static [ClipboardInsertStep] {
    if also_keep_clipboard {
        &[
            ClipboardInsertStep::SavePriorClipboard,
            ClipboardInsertStep::WriteTranscriptAndPaste,
        ]
    } else {
        &[
            ClipboardInsertStep::SavePriorClipboard,
            ClipboardInsertStep::WriteTranscriptAndPaste,
            ClipboardInsertStep::RestorePriorClipboard,
        ]
    }
}

/// Hold-to-talk insert: paste transcript at cursor via CLIPBOARD + ctrl+v (no Enter).
///
/// When `also_keep_clipboard` is true, leaves the transcript in CLIPBOARD after
/// paste (Copy transcript on). When false, restores the previous CLIPBOARD
/// contents after paste (best-effort on X11 — empty prior restores to empty).
pub fn insert_transcript_at_cursor(text: &str, also_keep_clipboard: bool) -> Result<()> {
    debug_assert_eq!(
        clipboard_insert_plan(also_keep_clipboard).first(),
        Some(&ClipboardInsertStep::SavePriorClipboard)
    );
    let prior = read_selection(ClipboardSel::Clipboard).unwrap_or_default();
    let paste_result = paste_at_cursor(text);
    if !also_keep_clipboard {
        // Let the focused app consume the paste before we reclaim CLIPBOARD.
        std::thread::sleep(CLIPBOARD_RESTORE_SETTLE);
        if let Err(restore_err) = write_clipboard(&prior) {
            if paste_result.is_ok() {
                return Err(restore_err).context("restore prior CLIPBOARD after insert paste");
            }
            // Prefer the paste error when both failed.
        }
    }
    paste_result
}

#[cfg(test)]
#[path = "x11util_tests/mod.rs"]
mod tests;
