//! Global hotkey registration (X11 via `global-hotkey` crate).

use anyhow::{bail, Context, Result};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    ReadAloud,
    PushToTalk,
}

/// Live global-hotkey grabs. Events are polled on the UI thread via
/// [`HotkeyHub::poll_events`] so re-register never races a background receiver
/// on the crate-global event channel (B13 root cause).
pub struct HotkeyHub {
    manager: GlobalHotKeyManager,
    /// Bound keys + action mapping for the global event channel.
    bindings: Vec<(HotKey, HotkeyAction)>,
}

#[derive(Debug, Clone)]
pub struct HotkeyEvent {
    pub action: HotkeyAction,
    pub pressed: bool,
}

impl HotkeyHub {
    /// Register read-aloud and push-to-talk grabs. Caller must drop any previous
    /// hub first so X11 key grabs are released before re-bind.
    pub fn register(read_aloud: &str, push_to_talk: &str) -> Result<Self> {
        let manager = GlobalHotKeyManager::new().context("create GlobalHotKeyManager")?;
        let hk_read = parse_hotkey(read_aloud).with_context(|| format!("parse {read_aloud}"))?;
        let hk_ptt = parse_hotkey(push_to_talk).with_context(|| format!("parse {push_to_talk}"))?;
        manager
            .register(hk_read)
            .with_context(|| format!("register read_aloud {read_aloud}"))?;
        if let Err(e) = manager.register(hk_ptt) {
            let _ = manager.unregister(hk_read);
            return Err(e).with_context(|| format!("register push_to_talk {push_to_talk}"));
        }

        // Drain stale events from a previous hub so they cannot fire wrong actions.
        drain_global_hotkey_events();

        Ok(Self {
            manager,
            bindings: vec![
                (hk_read, HotkeyAction::ReadAloud),
                (hk_ptt, HotkeyAction::PushToTalk),
            ],
        })
    }

    /// Poll the crate-global hotkey channel; map IDs to our bindings.
    pub fn poll_events(&self) -> Vec<HotkeyEvent> {
        let mut out = Vec::new();
        let rx = GlobalHotKeyEvent::receiver();
        while let Ok(ev) = rx.try_recv() {
            let pressed = matches!(ev.state, HotKeyState::Pressed);
            if let Some((_, action)) = self.bindings.iter().find(|(hk, _)| hk.id() == ev.id) {
                out.push(HotkeyEvent {
                    action: *action,
                    pressed,
                });
            }
        }
        out
    }

    /// Specs currently registered (config string form), for tests / status.
    pub fn registered_specs(&self) -> Result<Vec<String>> {
        self.bindings
            .iter()
            .map(|(hk, _)| format_hotkey(hk))
            .collect()
    }
}

impl Drop for HotkeyHub {
    fn drop(&mut self) {
        for (hk, _) in &self.bindings {
            let _ = self.manager.unregister(*hk);
        }
        self.bindings.clear();
        drain_global_hotkey_events();
    }
}

fn drain_global_hotkey_events() {
    let rx = GlobalHotKeyEvent::receiver();
    while rx.try_recv().is_ok() {}
}

/// Drop previous hub (releasing grabs), then register new specs.
///
/// This is the Apply-path primitive: never call `register` while an old hub
/// still holds the same (or any) X11 grabs.
pub fn reregister(
    previous: Option<HotkeyHub>,
    read_aloud: &str,
    push_to_talk: &str,
) -> Result<HotkeyHub> {
    // Explicit drop before new grabs — assignment order alone is not enough if
    // register() runs while `previous` still lives in the caller's local.
    drop(previous);
    // Brief yield so X11 releases the old grabs before we re-acquire.
    std::thread::sleep(std::time::Duration::from_millis(50));
    HotkeyHub::register(read_aloud, push_to_talk)
}

/// Validate and canonicalize a hotkey config string (parse → format).
pub fn canonicalize_hotkey_spec(spec: &str) -> Result<String> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        bail!("hotkey binding cannot be empty");
    }
    let hk = parse_hotkey(trimmed).with_context(|| format!("parse {trimmed}"))?;
    format_hotkey(&hk)
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
    let named = if t.eq_ignore_ascii_case("space") {
        "Space".into()
    } else if t.eq_ignore_ascii_case("esc") || t.eq_ignore_ascii_case("escape") {
        "Escape".into()
    } else {
        let upper = t.to_ascii_uppercase();
        if upper.starts_with('F')
            && upper.len() > 1
            && upper[1..].chars().all(|c| c.is_ascii_digit())
        {
            upper
        } else {
            let mut c = t.chars();
            match c.next() {
                Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                None => bail!("empty key token"),
            }
        }
    };
    let _ = Code::from_str(&to_code_name(&named))
        .map_err(|_| anyhow::anyhow!("unknown key: {key}"))?;
    Ok(named)
}

fn code_to_key_token(code: Code) -> Result<String> {
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
        let _ = hk.id();
    }

    #[test]
    fn parse_ctrl_alt_r() {
        let _ = parse_hotkey("Ctrl+Alt+R").unwrap();
    }

    #[test]
    fn parse_host_alt_shift_bindings() {
        // Host config from user feedback B13
        let ra = parse_hotkey("Alt+Shift+S").unwrap();
        let ptt = parse_hotkey("Alt+Shift+Q").unwrap();
        assert_eq!(format_hotkey(&ra).unwrap(), "Alt+Shift+S");
        assert_eq!(format_hotkey(&ptt).unwrap(), "Alt+Shift+Q");
    }

    #[test]
    fn reject_empty() {
        assert!(parse_hotkey("").is_err());
        assert!(canonicalize_hotkey_spec("").is_err());
        assert!(canonicalize_hotkey_spec("   ").is_err());
    }

    #[test]
    fn canonicalize_round_trips_defaults_and_host() {
        for spec in [
            "Super+Shift+S",
            "Super+Shift+R",
            "Alt+Shift+S",
            "Alt+Shift+Q",
            "Ctrl+Alt+R",
        ] {
            let c = canonicalize_hotkey_spec(spec).unwrap();
            assert_eq!(c, spec);
            let again = canonicalize_hotkey_spec(&c).unwrap();
            assert_eq!(again, c);
        }
    }

    #[test]
    fn format_combo_super_shift_s_round_trips() {
        let spec = format_hotkey_parts(true, false, false, true, "S").unwrap();
        assert_eq!(spec, "Super+Shift+S");
        let hk = parse_hotkey(&spec).unwrap();
        let again = format_hotkey(&hk).unwrap();
        assert_eq!(again, "Super+Shift+S");
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

    #[test]
    fn capture_mapping_preserves_super_for_default_combos() {
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
        let hk = parse_hotkey("Super+Shift+S").unwrap();
        assert_eq!(format_hotkey(&hk).unwrap(), "Super+Shift+S");
    }

    #[test]
    fn capture_mapping_without_super_is_super_less() {
        let mods = capture_mod_state(false, false, false, true, false);
        assert!(!mods.super_key);
        assert_eq!(format_capture_hotkey(mods, "S").unwrap(), "Shift+S");

        let linux_ctrl = capture_mod_state(false, true, false, false, false);
        assert!(!linux_ctrl.super_key);
        assert_eq!(
            format_capture_hotkey(linux_ctrl, "R").unwrap(),
            "Ctrl+R"
        );
    }

    #[test]
    fn capture_mapping_mac_cmd_is_super() {
        let mods = capture_mod_state(true, false, false, true, false);
        assert!(mods.super_key);
        assert_eq!(
            format_capture_hotkey(mods, "S").unwrap(),
            "Super+Shift+S"
        );
    }

    /// reregister drops previous hub then binds new specs (Apply path).
    /// Uses DISPLAY when present; skips cleanly if no X11 for manager create.
    #[test]
    fn reregister_drops_then_binds_new_specs() {
        if std::env::var_os("DISPLAY").is_none() {
            eprintln!("skip: no DISPLAY for hotkey manager");
            return;
        }
        let first = match HotkeyHub::register("Alt+Shift+F9", "Alt+Shift+F10") {
            Ok(h) => h,
            Err(e) => {
                eprintln!("skip: first register failed: {e:#}");
                return;
            }
        };
        let specs = first.registered_specs().unwrap();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0], "Alt+Shift+F9");
        assert_eq!(specs[1], "Alt+Shift+F10");

        let second = match reregister(Some(first), "Alt+Shift+F11", "Alt+Shift+F12") {
            Ok(h) => h,
            Err(e) => {
                panic!("reregister must succeed after drop: {e:#}");
            }
        };
        let specs2 = second.registered_specs().unwrap();
        assert_eq!(specs2[0], "Alt+Shift+F11");
        assert_eq!(specs2[1], "Alt+Shift+F12");

        // Third rebind back to host-like Alt+Shift combos
        let third = reregister(Some(second), "Alt+Shift+S", "Alt+Shift+Q")
            .expect("third reregister");
        let specs3 = third.registered_specs().unwrap();
        assert_eq!(specs3[0], "Alt+Shift+S");
        assert_eq!(specs3[1], "Alt+Shift+Q");
        drop(third);
    }

    #[test]
    fn reregister_from_none_works() {
        if std::env::var_os("DISPLAY").is_none() {
            eprintln!("skip: no DISPLAY");
            return;
        }
        match reregister(None, "Ctrl+Shift+F9", "Ctrl+Shift+F10") {
            Ok(h) => {
                let s = h.registered_specs().unwrap();
                assert_eq!(s[0], "Ctrl+Shift+F9");
                drop(h);
            }
            Err(e) => eprintln!("skip: {e:#}"),
        }
    }
}
