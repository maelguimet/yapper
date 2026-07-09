//! CLIPBOARD / PRIMARY round-trips and Super modifier query (Xvfb when needed).

use super::super::*;
use super::{x11_lock, IsolatedX};

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
    let down = query_super_modifier_down().expect("query Super/Mod4 via XQueryPointer");
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
    let path = super::scratch_dir().join("primary-read-aloud.txt");
    let _ = std::fs::create_dir_all(super::scratch_dir());
    let _ = std::fs::write(&path, format!("primary_ok={got}\n"));
}
