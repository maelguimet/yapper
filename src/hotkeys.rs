//! Global hotkey registration (X11 via `global-hotkey` crate).

use anyhow::{Context, Result};
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
    u
}

fn code_from_char(ch: char) -> Result<Code> {
    let name = format!("Key{ch}");
    Code::from_str(&name).map_err(|_| anyhow::anyhow!("unknown key char: {ch}"))
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
}
