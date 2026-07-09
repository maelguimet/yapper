//! Paste / insert-at-cursor and clipboard keep/restore (Xvfb + sink).

use super::super::*;
use super::{assert_injected_via_sink, scratch_dir, x11_lock, IsolatedX};
use std::fs;

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
    // paste_at_cursor intentionally leaves text in CLIPBOARD
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

/// Copy on: after insert, CLIPBOARD holds the transcript (product "Copy transcript").
#[test]
fn insert_keep_clipboard_leaves_transcript() {
    let _guard = x11_lock();
    let _iso = match IsolatedX::start() {
        Some(x) => x,
        None => {
            eprintln!("skip insert keep: no Xvfb/tools");
            return;
        }
    };
    let prior = format!("yapper-prior-keep-{}", std::process::id());
    let transcript = format!("yapper-transcript-keep-{}", std::process::id());
    write_clipboard(&prior).expect("seed prior CLIPBOARD");
    assert_eq!(
        read_selection(ClipboardSel::Clipboard).expect("read prior"),
        prior
    );

    assert_injected_via_sink(&transcript, "clipboard-keep-inject-proof.txt", |m| {
        insert_transcript_at_cursor(m, true)
    });

    let clip = read_selection(ClipboardSel::Clipboard).expect("clipboard after keep insert");
    assert_eq!(
        clip, transcript,
        "Copy on must leave transcript in CLIPBOARD, not prior {prior:?}"
    );

    let _ = fs::create_dir_all(scratch_dir());
    let log = scratch_dir().join("clipboard-keep.log");
    let _ = fs::write(
        &log,
        format!("prior={prior}\ntranscript={transcript}\nfinal_clipboard={clip}\nkeep=true\n"),
    );
}

/// Copy off: after insert, CLIPBOARD is restored to prior (not left as transcript).
#[test]
fn insert_restore_clipboard_when_copy_off() {
    let _guard = x11_lock();
    let _iso = match IsolatedX::start() {
        Some(x) => x,
        None => {
            eprintln!("skip insert restore: no Xvfb/tools");
            return;
        }
    };
    let prior = format!("yapper-prior-restore-{}", std::process::id());
    let transcript = format!("yapper-transcript-restore-{}", std::process::id());
    write_clipboard(&prior).expect("seed prior CLIPBOARD");
    assert_eq!(
        read_selection(ClipboardSel::Clipboard).expect("read prior"),
        prior
    );

    assert_injected_via_sink(&transcript, "clipboard-restore-inject-proof.txt", |m| {
        insert_transcript_at_cursor(m, false)
    });

    let clip = read_selection(ClipboardSel::Clipboard).expect("clipboard after restore insert");
    assert_eq!(
        clip, prior,
        "Copy off must restore prior CLIPBOARD; got transcript-like {clip:?}"
    );
    assert_ne!(clip, transcript, "must not leave transcript when Copy off");

    let _ = fs::create_dir_all(scratch_dir());
    let log = scratch_dir().join("clipboard-restore.log");
    let _ = fs::write(
        &log,
        format!("prior={prior}\ntranscript={transcript}\nfinal_clipboard={clip}\nkeep=false\n"),
    );
}

/// Empty prior + Copy off restores to empty (not leftover transcript).
#[test]
fn insert_restore_empty_prior_when_copy_off() {
    let _guard = x11_lock();
    let _iso = match IsolatedX::start() {
        Some(x) => x,
        None => {
            eprintln!("skip insert empty restore: no Xvfb/tools");
            return;
        }
    };
    // Clear CLIPBOARD as best-effort empty prior.
    write_clipboard("").expect("clear CLIPBOARD");
    let transcript = format!("yapper-transcript-empty-prior-{}", std::process::id());
    insert_transcript_at_cursor(&transcript, false).expect("insert with restore");
    let clip = read_selection(ClipboardSel::Clipboard).expect("clipboard");
    assert_eq!(
        clip, "",
        "empty prior must restore to empty; got {clip:?}"
    );
    assert_ne!(clip, transcript);
}
