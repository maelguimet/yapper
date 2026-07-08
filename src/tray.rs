//! System tray menu (StatusNotifier / AppIndicator via tray-icon).

use anyhow::{Context, Result};
use std::sync::mpsc::{self, Receiver};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Open,
    LoadStt,
    UnloadStt,
    LoadTts,
    UnloadTts,
    Quit,
}

pub struct TrayHandle {
    _tray: TrayIcon,
    rx: Receiver<TrayAction>,
}

impl TrayHandle {
    pub fn try_create() -> Result<Self> {
        // libappindicator path goes through gtk; init if the display is ready.
        #[cfg(target_os = "linux")]
        {
            let _ = gtk::init();
        }
        let menu = Menu::new();
        let open = MenuItem::new("Open", true, None);
        let load_stt = MenuItem::new("Load STT", true, None);
        let unload_stt = MenuItem::new("Unload STT", true, None);
        let load_tts = MenuItem::new("Load TTS", true, None);
        let unload_tts = MenuItem::new("Unload TTS", true, None);
        let quit = MenuItem::new("Quit", true, None);
        menu.append(&open)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&load_stt)?;
        menu.append(&unload_stt)?;
        menu.append(&load_tts)?;
        menu.append(&unload_tts)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&quit)?;

        let icon = default_icon();
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Yapper")
            .with_icon(icon)
            .build()
            .context("build tray icon")?;

        let id_open = open.id().clone();
        let id_load_stt = load_stt.id().clone();
        let id_unload_stt = unload_stt.id().clone();
        let id_load_tts = load_tts.id().clone();
        let id_unload_tts = unload_tts.id().clone();
        let id_quit = quit.id().clone();

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let menu_rx = MenuEvent::receiver();
            while let Ok(ev) = menu_rx.recv() {
                let action = if ev.id == id_open {
                    Some(TrayAction::Open)
                } else if ev.id == id_load_stt {
                    Some(TrayAction::LoadStt)
                } else if ev.id == id_unload_stt {
                    Some(TrayAction::UnloadStt)
                } else if ev.id == id_load_tts {
                    Some(TrayAction::LoadTts)
                } else if ev.id == id_unload_tts {
                    Some(TrayAction::UnloadTts)
                } else if ev.id == id_quit {
                    Some(TrayAction::Quit)
                } else {
                    None
                };
                if let Some(a) = action {
                    if tx.send(a).is_err() {
                        break;
                    }
                }
            }
        });

        Ok(Self { _tray: tray, rx })
    }

    pub fn try_recv(&self) -> Option<TrayAction> {
        self.rx.try_recv().ok()
    }
}

fn default_icon() -> Icon {
    // 32x32 simple blue-ish solid with a white center square (mic-ish blob)
    let size = 32u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            let cx = x as i32 - 16;
            let cy = y as i32 - 16;
            let r2 = cx * cx + cy * cy;
            if r2 < 14 * 14 {
                rgba[i] = 40;
                rgba[i + 1] = 120;
                rgba[i + 2] = 220;
                rgba[i + 3] = 255;
            } else {
                rgba[i + 3] = 0;
            }
        }
    }
    Icon::from_rgba(rgba, size, size).expect("icon")
}
