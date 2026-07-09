//! Pure clipboard-insert plan + tool detection (no X session required).

use super::super::*;

#[test]
fn clipboard_insert_plan_restore_when_copy_off() {
    use ClipboardInsertStep::*;
    assert_eq!(
        clipboard_insert_plan(false),
        &[
            SavePriorClipboard,
            WriteTranscriptAndPaste,
            RestorePriorClipboard,
        ],
        "Copy off must plan save→paste→restore"
    );
}

#[test]
fn clipboard_insert_plan_keep_when_copy_on() {
    use ClipboardInsertStep::*;
    assert_eq!(
        clipboard_insert_plan(true),
        &[SavePriorClipboard, WriteTranscriptAndPaste],
        "Copy on must plan save→paste with no restore"
    );
}

#[test]
fn tools_detection_does_not_panic() {
    let _ = x11_tools_available();
    let _ = display_available();
    let _ = super_modifier_down();
}
