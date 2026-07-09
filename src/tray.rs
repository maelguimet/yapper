//! System tray menu (StatusNotifier / AppIndicator via tray-icon).

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;
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

/// How many create attempts and spacing between them (display / SNI host race).
const TRAY_CREATE_ATTEMPTS: u32 = 4;
const TRAY_CREATE_RETRY_MS: u64 = 250;

impl TrayHandle {
    /// Create the tray icon, retrying briefly if the first attempt fails.
    pub fn try_create() -> Result<Self> {
        let mut last_err = None;
        for attempt in 1..=TRAY_CREATE_ATTEMPTS {
            match Self::try_create_once() {
                Ok(h) => return Ok(h),
                Err(e) => {
                    last_err = Some(e);
                    if attempt < TRAY_CREATE_ATTEMPTS {
                        std::thread::sleep(Duration::from_millis(TRAY_CREATE_RETRY_MS));
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("tray create failed")))
            .context(tray_failure_hint())
    }

    fn try_create_once() -> Result<Self> {
        // libappindicator path goes through gtk; init if the display is ready.
        #[cfg(target_os = "linux")]
        {
            if gtk::init().is_err() {
                // Already initialized is OK; gtk::init errors on second call.
                // Only bail when we have no display at all.
                if std::env::var_os("DISPLAY").is_none()
                    && std::env::var_os("WAYLAND_DISPLAY").is_none()
                {
                    bail!("no DISPLAY/WAYLAND_DISPLAY for tray");
                }
            }
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

        let icon = load_tray_icon();
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Yapper — STT + TTS (right-click for menu)")
            .with_icon(icon)
            .with_title("Yapper")
            .build()
            .context("build tray icon (StatusNotifier/AppIndicator)")?;

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

/// Human-readable hint when tray create fails (also used by doctor).
pub fn tray_failure_hint() -> String {
    "Tray icon needs a StatusNotifier/AppIndicator host \
     (GNOME: enable AppIndicator extension / ubuntu-appindicators). \
     Without a tray host Yapper cannot hide-to-tray safely — \
     install gnome-shell-extension-appindicator or use a DE with SNI support."
        .into()
}

/// Best-effort runtime checks for tray host tooling (no live SNI probe).
pub fn tray_host_diagnostics() -> TrayHostReport {
    let display = std::env::var("DISPLAY").ok();
    let session = std::env::var("XDG_SESSION_TYPE").ok();
    let has_display = display.is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
    let ayatana = Path::new("/usr/lib/x86_64-linux-gnu/libayatana-appindicator3.so.1").is_file()
        || Path::new("/usr/lib/libayatana-appindicator3.so.1").is_file()
        || which_lib("libayatana-appindicator3.so.1");
    let appindicator_pkg = path_exists_any(&[
        "/usr/share/gnome-shell/extensions/ubuntu-appindicators@ubuntu.com",
        "/usr/share/gnome-shell/extensions/appindicatorsupport@rgcjonas.gmail.com",
    ]);
    TrayHostReport {
        has_display,
        display,
        session,
        ayatana_lib_present: ayatana,
        appindicator_extension_dir: appindicator_pkg,
        hint: tray_failure_hint(),
    }
}

#[derive(Debug, Clone)]
pub struct TrayHostReport {
    pub has_display: bool,
    pub display: Option<String>,
    pub session: Option<String>,
    pub ayatana_lib_present: bool,
    pub appindicator_extension_dir: bool,
    pub hint: String,
}

impl TrayHostReport {
    pub fn summary_line(&self) -> String {
        format!(
            "display={} session={} ayatana_lib={} appindicator_ext={}",
            if self.has_display { "yes" } else { "no" },
            self.session.as_deref().unwrap_or("?"),
            if self.ayatana_lib_present {
                "ok"
            } else {
                "MISSING"
            },
            if self.appindicator_extension_dir {
                "ok"
            } else {
                "missing/unknown"
            }
        )
    }

    /// True when basic tray prerequisites look present (not a guarantee of icon).
    pub fn looks_ready(&self) -> bool {
        self.has_display && self.ayatana_lib_present
    }
}

fn path_exists_any(paths: &[&str]) -> bool {
    paths.iter().any(|p| Path::new(p).is_dir() || Path::new(p).is_file())
}

fn which_lib(name: &str) -> bool {
    // ldconfig -p is slow; check common multiarch dirs only.
    let candidates = [
        format!("/usr/lib/x86_64-linux-gnu/{name}"),
        format!("/usr/lib/{name}"),
        format!("/lib/x86_64-linux-gnu/{name}"),
    ];
    candidates.iter().any(|p| Path::new(p).is_file())
}

/// Load icon from assets if present, else procedural RGBA.
fn load_tray_icon() -> Icon {
    if let Some(icon) = try_load_png_icon() {
        return icon;
    }
    procedural_icon()
}

fn try_load_png_icon() -> Option<Icon> {
    // Prefer installed / repo assets. PNG via image crate is optional — we ship a
    // simple raw RGBA file generator as fallback. For v0.2 we embed procedural
    // monochrome-friendly icon; if assets/yapper-tray.rgba exists (w h rgba…), load it.
    let candidates = [
        Path::new("assets/yapper-tray.png"),
        Path::new("/home/maelguimet/projects/yapper/assets/yapper-tray.png"),
    ];
    // Without an image decoder dep, only accept raw RGBA dump: 8-byte header
    // (width u32 LE, height u32 LE) + RGBA pixels. Written by scripts or tests.
    let raw_candidates = [
        Path::new("assets/yapper-tray.rgba"),
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/yapper-tray.rgba")),
    ];
    for path in raw_candidates {
        if let Ok(icon) = icon_from_rgba_file(path) {
            return Some(icon);
        }
    }
    let _ = candidates; // reserved for image-crate path later
    None
}

/// Read raw RGBA icon: 4-byte width LE, 4-byte height LE, then width*height*4 bytes.
pub fn icon_from_rgba_file(path: &Path) -> Result<Icon> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if bytes.len() < 8 {
        bail!("icon file too small");
    }
    let w = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let h = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let need = 8 + (w as usize) * (h as usize) * 4;
    if bytes.len() < need || w == 0 || h == 0 || w > 256 || h > 256 {
        bail!("invalid icon dimensions {w}x{h}");
    }
    let rgba = bytes[8..need].to_vec();
    Icon::from_rgba(rgba, w, h).context("Icon::from_rgba")
}

/// Write a raw RGBA icon file (for assets / tests).
pub fn write_rgba_icon_file(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    let expect = (width as usize) * (height as usize) * 4;
    if rgba.len() != expect {
        bail!("rgba len {} != {}", rgba.len(), expect);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = Vec::with_capacity(8 + rgba.len());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(rgba);
    std::fs::write(path, out).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// 32×32 monochrome-friendly mic glyph on transparent background.
fn procedural_icon() -> Icon {
    let size = 32u32;
    let rgba = build_mic_icon_rgba(size);
    Icon::from_rgba(rgba, size, size).expect("procedural tray icon")
}

/// Pure RGBA builder for the default tray glyph (tested without tray-icon display).
pub fn build_mic_icon_rgba(size: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let s = size as i32;
    let cx = s / 2;
    let cy = s / 2 - 2;
    // Mic capsule
    for y in 0..s {
        for x in 0..s {
            let i = ((y * s + x) * 4) as usize;
            let dx = x - cx;
            let dy = y - cy;
            // Capsule body
            let in_capsule = dx.abs() <= s / 6
                && dy >= -s / 4
                && dy <= s / 6
                || (dx * dx + (dy + s / 4) * (dy + s / 4) <= (s / 6) * (s / 6));
            // Stand
            let in_stand = dx.abs() <= 1 && dy > s / 6 && dy < s / 3;
            let in_base = dy >= s / 3 - 1 && dy <= s / 3 + 1 && dx.abs() <= s / 5;
            // Arc under capsule
            let arc_r = s / 4;
            let in_arc = {
                let adx = dx;
                let ady = dy - s / 10;
                let r2 = adx * adx + ady * ady;
                r2 <= arc_r * arc_r
                    && r2 >= (arc_r - 2) * (arc_r - 2)
                    && ady >= 0
            };
            if in_capsule || in_stand || in_base || in_arc {
                // Light blue, fully opaque (readable on light and dark panels)
                rgba[i] = 90;
                rgba[i + 1] = 170;
                rgba[i + 2] = 255;
                rgba[i + 3] = 255;
            }
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn mic_icon_rgba_has_opaque_pixels() {
        let rgba = build_mic_icon_rgba(32);
        assert_eq!(rgba.len(), 32 * 32 * 4);
        let opaque = rgba.chunks(4).filter(|c| c[3] > 0).count();
        assert!(opaque > 20, "expected mic glyph pixels, got {opaque}");
        assert!(opaque < 32 * 32, "should not be fully solid");
    }

    #[test]
    fn rgba_file_round_trip_loads() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("yapper-icon-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("icon.rgba");
        let rgba = build_mic_icon_rgba(16);
        write_rgba_icon_file(&path, 16, 16, &rgba).unwrap();
        let icon = icon_from_rgba_file(&path).unwrap();
        let _ = icon;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tray_host_report_summary_nonempty() {
        let r = tray_host_diagnostics();
        assert!(!r.summary_line().is_empty());
        assert!(!r.hint.is_empty());
    }

    #[test]
    fn failure_hint_mentions_appindicator() {
        let h = tray_failure_hint();
        assert!(h.to_ascii_lowercase().contains("appindicator") || h.contains("StatusNotifier"));
    }
}
