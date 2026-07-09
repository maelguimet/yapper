//! Global hotkey registration (X11 via `global-hotkey` crate).

use anyhow::{bail, Context, Result};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::str::FromStr;
use std::sync::mpsc::{self, Receiver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    ReadAloud,
    PushToTalk,
}

pub struct HotkeyHub {
    _manager: GlobalHotKeyManager,
    pub rx: Receiver<HotkeyEvent>,
}

#[derive(Debug, Clone)]
pub struct HotkeyEvent {
    pub action: HotkeyAction,
    pub pressed: bool,
}

impl HotkeyHub {
    pub fn register(read_aloud: &str, push_to_talk: &str) -> Result<Self> {
        let manager = GlobalHotKeyManager::new().context("create GlobalHotKeyManager")?;
        let hk_read = parse_hotkey(read_aloud).with_context(|| format!("parse {read_aloud}"))?;
        let hk_ptt = parse_hotkey(push_to_talk).with_context(|| format!("parse {push_to_talk}"))?;
        manager
            .register(hk_read)
            .with_context(|| format!("register read_aloud {read_aloud}"))?;
        manager
            .register(hk_ptt)
            .with_context(|| format!("register push_to_talk {push_to_talk}"))?;

        let id_read = hk_read.id();
        let id_ptt = hk_ptt.id();
        let (tx, rx) = mpsc::channel();

        // Poll thread
        std::thread::spawn(move || {
            let event_rx = GlobalHotKeyEvent::receiver();
            while let Ok(ev) = event_rx.recv() {
                let pressed = matches!(ev.state, HotKeyState::Pressed);
                let action = if ev.id == id_read {
                    Some(HotkeyAction::ReadAloud)
                } else if ev.id == id_ptt {
                    Some(HotkeyAction::PushToTalk)
                } else {
                    None
                };
                if let Some(action) = action {
                    let _ = tx.send(HotkeyEvent { action, pressed });
                }
            }
        });

        Ok(Self {
            _manager: manager,
            rx,
        })
    }
}

/// Modifier state for the hotkey Capture picker (egui flags + platform Super).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptureModState {
    pub super_key: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Build capture modifiers from egui flags plus platform Super/Mod4.
///
/// egui-winit on Linux sets `mac_cmd = false` always and maps `command` to Ctrl,
/// discarding Super. Callers must pass `platform_super` from session state
/// (X11 Mod4 / Super) so defaults like Super+Shift+S survive Capture.
pub fn capture_mod_state(
    mac_cmd: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
    platform_super: bool,
) -> CaptureModState {
    CaptureModState {
        // Super = macOS Command (mac_cmd) OR Linux Super/Mod4 (platform_super).
        // Do not use egui `command` on Linux — it mirrors Ctrl, not Super.
        super_key: mac_cmd || platform_super,
        ctrl,
        alt,
        shift,
    }
}

/// Map capture modifiers + key token → config string (the real Capture→bind path).
pub fn format_capture_hotkey(mods: CaptureModState, key_token: &str) -> Result<String> {
    format_hotkey_parts(
        mods.super_key,
        mods.ctrl,
        mods.alt,
        mods.shift,
        key_token,
    )
}

/// Format modifier flags + key name into config string form (`Super+Shift+S`).
///
/// Order is fixed: Super, Ctrl, Alt, Shift, then key — matching parse expectations.
pub fn format_hotkey_parts(
    super_key: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
    key: &str,
) -> Result<String> {
    let key = key.trim();
    if key.is_empty() {
        bail!("no key in hotkey combo");
    }
    let mut parts: Vec<String> = Vec::new();
    if super_key {
        parts.push("Super".into());
    }
    if ctrl {
        parts.push("Ctrl".into());
    }
    if alt {
        parts.push("Alt".into());
    }
    if shift {
        parts.push("Shift".into());
    }
    parts.push(normalize_key_token(key)?);
    let spec = parts.join("+");
    let _ = parse_hotkey(&spec)?;
    Ok(spec)
}

/// Render a registered `HotKey` back to config string form (round-trip helper).
pub fn format_hotkey(hk: &HotKey) -> Result<String> {
    format_hotkey_parts(
        hk.mods.contains(Modifiers::SUPER),
        hk.mods.contains(Modifiers::CONTROL),
        hk.mods.contains(Modifiers::ALT),
        hk.mods.contains(Modifiers::SHIFT),
        &code_to_key_token(hk.key)?,
    )
}

/// Parse strings like `Super+Shift+S` into a HotKey.
pub fn parse_hotkey(spec: &str) -> Result<HotKey> {
    let mut mods = Modifiers::empty();
    let mut key: Option<Code> = None;
    for part in spec.split('+').map(str::trim).filter(|s| !s.is_empty()) {
        let lower = part.to_ascii_lowercase();
        match lower.as_str() {
            "super" | "meta" | "win" | "cmd" => mods |= Modifiers::SUPER,
            "shift" => mods |= Modifiers::SHIFT,
            "ctrl" | "control" => mods |= Modifiers::CONTROL,
            "alt" | "option" => mods |= Modifiers::ALT,
            other => {
                // single character or named key
                let code = if other.len() == 1 {
                    let ch = other.chars().next().unwrap().to_ascii_uppercase();
                    code_from_char(ch)?
                } else {
                    Code::from_str(&to_code_name(other))
                        .map_err(|_| anyhow::anyhow!("unknown key: {part}"))?
                };
                key = Some(code);
            }
        }
    }
    let key = key.ok_or_else(|| anyhow::anyhow!("no key in hotkey spec: {spec}"))?;
    Ok(HotKey::new(Some(mods), key))
}

fn to_code_name(s: &str) -> String {
    // global-hotkey uses KeyA, Digit1, etc.
    let u = s.to_ascii_uppercase();
    if u.len() == 1 && u.chars().next().unwrap().is_ascii_alphabetic() {
        return format!("Key{u}");
    }
    if u.len() == 1 && u.chars().next().unwrap().is_ascii_digit() {
        return format!("Digit{u}");
    }
    u
}

fn code_from_char(ch: char) -> Result<Code> {
    if ch.is_ascii_digit() {
        let name = format!("Digit{ch}");
        return Code::from_str(&name).map_err(|_| anyhow::anyhow!("unknown key char: {ch}"));
    }
    let name = format!("Key{ch}");
    Code::from_str(&name).map_err(|_| anyhow::anyhow!("unknown key char: {ch}"))
}

fn normalize_key_token(key: &str) -> Result<String> {
    let t = key.trim();
    if t.is_empty() {
        bail!("empty key token");
    }
    if t.chars().count() == 1 {
        let ch = t.chars().next().unwrap();
        if ch.is_ascii_alphabetic() {
            return Ok(ch.to_ascii_uppercase().to_string());
        }
        if ch.is_ascii_digit() {
            return Ok(ch.to_string());
        }
    }
    // Named keys: keep Pascal-ish single token (Space, Escape, F1…).
    let named = if t.eq_ignore_ascii_case("space") {
        "Space".into()
    } else if t.eq_ignore_ascii_case("esc") || t.eq_ignore_ascii_case("escape") {
        "Escape".into()
    } else {
        // Preserve F-keys casing (F1)
        let upper = t.to_ascii_uppercase();
        if upper.starts_with('F')
            && upper.len() > 1
            && upper[1..].chars().all(|c| c.is_ascii_digit())
        {
            upper
        } else {
            // Title-case first letter for Code names that FromStr accepts
            let mut c = t.chars();
            match c.next() {
                Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                None => bail!("empty key token"),
            }
        }
    };
    // Ensure parseable via Code path
    let _ = Code::from_str(&to_code_name(&named))
        .map_err(|_| anyhow::anyhow!("unknown key: {key}"))?;
    Ok(named)
}

fn code_to_key_token(code: Code) -> Result<String> {
    // Prefer stable Debug names from keyboard-types / global-hotkey (KeyA, Digit1, …).
    let dbg = format!("{code:?}");
    if let Some(rest) = dbg.strip_prefix("Key") {
        if rest.len() == 1 && rest.chars().next().unwrap().is_ascii_alphabetic() {
            return Ok(rest.to_string());
        }
    }
    if let Some(rest) = dbg.strip_prefix("Digit") {
        if rest.len() == 1 && rest.chars().next().unwrap().is_ascii_digit() {
            return Ok(rest.to_string());
        }
    }
    Ok(dbg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_super_shift_s() {
        let hk = parse_hotkey("Super+Shift+S").unwrap();
        // id is deterministic from mods+key
        let _ = hk.id();
    }

    #[test]
    fn parse_ctrl_alt_r() {
        let _ = parse_hotkey("Ctrl+Alt+R").unwrap();
    }

    #[test]
    fn reject_empty() {
        assert!(parse_hotkey("").is_err());
    }

    #[test]
    fn format_combo_super_shift_s_round_trips() {
        let spec = format_hotkey_parts(true, false, false, true, "S").unwrap();
        assert_eq!(spec, "Super+Shift+S");
        let hk = parse_hotkey(&spec).unwrap();
        let again = format_hotkey(&hk).unwrap();
        assert_eq!(again, "Super+Shift+S");
        // Same physical hotkey id for re-register
        assert_eq!(hk.id(), parse_hotkey(&again).unwrap().id());
    }

    #[test]
    fn format_combo_ctrl_alt_r_round_trips() {
        let spec = format_hotkey_parts(false, true, true, false, "R").unwrap();
        assert_eq!(spec, "Ctrl+Alt+R");
        let hk = parse_hotkey(&spec).unwrap();
        assert_eq!(format_hotkey(&hk).unwrap(), "Ctrl+Alt+R");
    }

    #[test]
    fn format_rejects_empty_key() {
        assert!(format_hotkey_parts(true, false, false, false, "").is_err());
        assert!(format_hotkey_parts(true, false, false, false, "   ").is_err());
    }

    #[test]
    fn format_rejects_unknown_key() {
        assert!(format_hotkey_parts(false, true, false, false, "NotAKeyXYZ").is_err());
    }

    /// Capture path with Super held (Linux Mod4 / platform_super=true).
    /// Product defaults Super+Shift+S and Super+Shift+R must keep Super.
    #[test]
    fn capture_mapping_preserves_super_for_default_combos() {
        // Linux: egui mac_cmd=false, Super only from platform_super.
        let mods_s = capture_mod_state(false, false, false, true, true);
        assert!(mods_s.super_key);
        assert_eq!(
            format_capture_hotkey(mods_s, "S").unwrap(),
            "Super+Shift+S"
        );
        let mods_r = capture_mod_state(false, false, false, true, true);
        assert_eq!(
            format_capture_hotkey(mods_r, "R").unwrap(),
            "Super+Shift+R"
        );
        // Round-trip through parse (Apply path).
        let hk = parse_hotkey("Super+Shift+S").unwrap();
        assert_eq!(format_hotkey(&hk).unwrap(), "Super+Shift+S");
    }

    /// Capture path without Super: must not invent Super from bare Shift/Ctrl.
    #[test]
    fn capture_mapping_without_super_is_super_less() {
        let mods = capture_mod_state(false, false, false, true, false);
        assert!(!mods.super_key);
        assert_eq!(format_capture_hotkey(mods, "S").unwrap(), "Shift+S");

        // Linux trap: egui command mirrors ctrl — must not treat as Super.
        let linux_ctrl = capture_mod_state(false, true, false, false, false);
        assert!(!linux_ctrl.super_key);
        assert_eq!(
            format_capture_hotkey(linux_ctrl, "R").unwrap(),
            "Ctrl+R"
        );
    }

    /// macOS path: mac_cmd alone means Super in our config strings.
    #[test]
    fn capture_mapping_mac_cmd_is_super() {
        let mods = capture_mod_state(true, false, false, true, false);
        assert!(mods.super_key);
        assert_eq!(
            format_capture_hotkey(mods, "S").unwrap(),
            "Super+Shift+S"
        );
    }
}
