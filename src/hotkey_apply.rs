//! Draft vs live hotkey apply policy (pure decisions; no X11).
//!
//! Settings editors bind to **draft** strings. Live [`HotkeysConfig`] (and disk)
//! update only after a successful Apply: validate → grab → commit.

use crate::config::HotkeysConfig;
use crate::hotkeys::canonicalize_hotkey_spec;

/// Outcome of an Apply attempt (validate + simulated/real grab).
#[derive(Debug, Clone, PartialEq)]
pub enum HotkeyApplyOutcome {
    /// Draft failed parse/canonicalize — live grabs, live config, and disk stay put.
    InvalidDraft {
        field: &'static str,
        message: String,
        status: String,
    },
    /// Grab failed after valid drafts — do not claim live/applied; keep previous live specs.
    GrabFailed {
        message: String,
        status: String,
        /// Previous applied specs (restore hub if possible; always keep on disk).
        restore: HotkeysConfig,
    },
    /// Validate + grab succeeded — commit these specs to live config and disk.
    Applied {
        live: HotkeysConfig,
        status: String,
    },
}

impl HotkeyApplyOutcome {
    /// True only when live config + disk may adopt the new hotkey specs.
    pub fn commits_live(&self) -> bool {
        matches!(self, Self::Applied { .. })
    }

    pub fn status_line(&self) -> &str {
        match self {
            Self::InvalidDraft { status, .. }
            | Self::GrabFailed { status, .. }
            | Self::Applied { status, .. } => status.as_str(),
        }
    }

    pub fn error_message(&self) -> Option<&str> {
        match self {
            Self::InvalidDraft { message, .. } | Self::GrabFailed { message, .. } => {
                Some(message.as_str())
            }
            Self::Applied { .. } => None,
        }
    }
}

/// Canonicalize both draft fields. No side effects.
pub fn validate_draft_hotkeys(
    draft_read: &str,
    draft_ptt: &str,
) -> Result<HotkeysConfig, HotkeyApplyOutcome> {
    let read_aloud = match canonicalize_hotkey_spec(draft_read) {
        Ok(s) => s,
        Err(e) => {
            return Err(HotkeyApplyOutcome::InvalidDraft {
                field: "read-aloud",
                message: format!("read-aloud invalid: {e:#}"),
                status: "hotkey update failed".into(),
            });
        }
    };
    let push_to_talk = match canonicalize_hotkey_spec(draft_ptt) {
        Ok(s) => s,
        Err(e) => {
            return Err(HotkeyApplyOutcome::InvalidDraft {
                field: "push-to-talk",
                message: format!("push-to-talk invalid: {e:#}"),
                status: "hotkey update failed".into(),
            });
        }
    };
    Ok(HotkeysConfig {
        read_aloud,
        push_to_talk,
    })
}

/// After validation, fold a grab Result into commit vs failure (no X11).
pub fn decide_after_grab(
    planned: HotkeysConfig,
    previous_live: HotkeysConfig,
    grab: Result<(), String>,
) -> HotkeyApplyOutcome {
    match grab {
        Ok(()) => {
            let status = format!(
                "hotkeys live: {} | {}",
                planned.read_aloud, planned.push_to_talk
            );
            HotkeyApplyOutcome::Applied {
                live: planned,
                status,
            }
        }
        Err(e) => HotkeyApplyOutcome::GrabFailed {
            message: format!(
                "hotkey grab failed (DE conflict or bad combo): {e}. \
                 Rebind or free the shortcut in system settings, then Apply again."
            ),
            status: "hotkey update failed — shortcuts inactive until fixed".into(),
            restore: previous_live,
        },
    }
}

/// Full Apply decision path without real X11: validate drafts, then use `grab_result`.
///
/// Production callers perform a real grab between `validate_draft_hotkeys` and
/// `decide_after_grab`; this combined helper exists for pure unit tests.
#[cfg(test)]
pub fn apply_hotkeys_decision(
    draft_read: &str,
    draft_ptt: &str,
    previous_live: &HotkeysConfig,
    grab_result: Result<(), String>,
) -> HotkeyApplyOutcome {
    match validate_draft_hotkeys(draft_read, draft_ptt) {
        Err(outcome) => outcome,
        Ok(planned) => decide_after_grab(planned, previous_live.clone(), grab_result),
    }
}

/// Live specs after an apply outcome (success → new; invalid/grab-fail → previous).
pub fn live_specs_after_outcome(
    previous: &HotkeysConfig,
    outcome: &HotkeyApplyOutcome,
) -> HotkeysConfig {
    match outcome {
        HotkeyApplyOutcome::Applied { live, .. } => live.clone(),
        HotkeyApplyOutcome::InvalidDraft { .. } => previous.clone(),
        HotkeyApplyOutcome::GrabFailed { restore, .. } => restore.clone(),
    }
}

/// Specs Save / `persist` may serialize for hotkeys: **live applied only**.
/// Draft editor text is intentionally ignored so garbage cannot poison disk.
pub fn hotkeys_persist_payload(
    live: &HotkeysConfig,
    _draft_read: &str,
    _draft_ptt: &str,
) -> HotkeysConfig {
    live.clone()
}

/// Whether a status/error string claims hotkeys are live (must not appear on failure).
#[cfg(test)]
pub fn claims_hotkeys_live(status: &str) -> bool {
    let lower = status.to_ascii_lowercase();
    lower.contains("hotkeys live") || lower.contains("settings saved")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn live_defaults() -> HotkeysConfig {
        HotkeysConfig {
            read_aloud: "Super+Shift+S".into(),
            push_to_talk: "Super+Shift+R".into(),
        }
    }

    #[test]
    fn invalid_draft_does_not_poison_live_or_persist_payload() {
        let previous = live_defaults();
        let draft_read = "not a real binding!!!";
        let draft_ptt = "Super+Shift+R";

        let outcome = apply_hotkeys_decision(draft_read, draft_ptt, &previous, Ok(()));
        assert!(!outcome.commits_live());
        assert!(outcome.error_message().is_some());
        assert_eq!(outcome.status_line(), "hotkey update failed");
        assert!(!claims_hotkeys_live(outcome.status_line()));

        let after = live_specs_after_outcome(&previous, &outcome);
        assert_eq!(after, previous);

        let disk = hotkeys_persist_payload(&after, draft_read, draft_ptt);
        assert_eq!(disk, previous);
        assert_ne!(disk.read_aloud, draft_read);
    }

    #[test]
    fn invalid_ptt_draft_leaves_read_aloud_live_unchanged() {
        let previous = live_defaults();
        let outcome = apply_hotkeys_decision("Alt+Shift+S", "garbage", &previous, Ok(()));
        match &outcome {
            HotkeyApplyOutcome::InvalidDraft { field, .. } => {
                assert_eq!(*field, "push-to-talk");
            }
            other => panic!("expected InvalidDraft, got {other:?}"),
        }
        assert_eq!(live_specs_after_outcome(&previous, &outcome), previous);
        assert_eq!(
            hotkeys_persist_payload(&previous, "Alt+Shift+S", "garbage"),
            previous
        );
    }

    #[test]
    fn failed_grab_does_not_claim_success_or_update_live() {
        let previous = live_defaults();
        let draft_read = "Alt+Shift+S";
        let draft_ptt = "Alt+Shift+Q";

        let outcome = apply_hotkeys_decision(
            draft_read,
            draft_ptt,
            &previous,
            Err("X11 grab conflict".into()),
        );

        assert!(!outcome.commits_live());
        assert!(!claims_hotkeys_live(outcome.status_line()));
        assert!(
            outcome.status_line().contains("inactive")
                || outcome.status_line().contains("failed"),
            "status={}",
            outcome.status_line()
        );
        match &outcome {
            HotkeyApplyOutcome::GrabFailed { restore, message, .. } => {
                assert_eq!(*restore, previous);
                assert!(message.contains("grab failed"));
            }
            other => panic!("expected GrabFailed, got {other:?}"),
        }

        let after = live_specs_after_outcome(&previous, &outcome);
        assert_eq!(after, previous);
        // Disk must keep previous applied specs, not the unapplied drafts.
        let disk = hotkeys_persist_payload(&after, draft_read, draft_ptt);
        assert_eq!(disk, previous);
    }

    #[test]
    fn successful_apply_commits_canonical_live_and_persist_payload() {
        let previous = live_defaults();
        let draft_read = "  Alt+Shift+S  ";
        let draft_ptt = "Alt+Shift+Q";

        let outcome = apply_hotkeys_decision(draft_read, draft_ptt, &previous, Ok(()));
        assert!(outcome.commits_live());
        assert!(claims_hotkeys_live(outcome.status_line()));
        assert!(outcome.error_message().is_none());

        let after = live_specs_after_outcome(&previous, &outcome);
        assert_eq!(after.read_aloud, "Alt+Shift+S");
        assert_eq!(after.push_to_talk, "Alt+Shift+Q");

        let disk = hotkeys_persist_payload(&after, draft_read, draft_ptt);
        assert_eq!(disk.read_aloud, "Alt+Shift+S");
        assert_eq!(disk.push_to_talk, "Alt+Shift+Q");

        match outcome {
            HotkeyApplyOutcome::Applied { live, status } => {
                assert_eq!(live, after);
                assert!(status.contains("Alt+Shift+S"));
                assert!(status.contains("Alt+Shift+Q"));
            }
            other => panic!("expected Applied, got {other:?}"),
        }
    }

    #[test]
    fn save_settings_path_never_writes_unapplied_draft_garbage() {
        let live = live_defaults();
        let garbage_read = "@@@not-a-key@@@";
        let garbage_ptt = "";
        // User typed garbage in editors then Save — payload ignores drafts.
        let payload = hotkeys_persist_payload(&live, garbage_read, garbage_ptt);
        assert_eq!(payload, live);
        assert!(canonicalize_hotkey_spec(&payload.read_aloud).is_ok());
        assert!(canonicalize_hotkey_spec(&payload.push_to_talk).is_ok());
    }

    #[test]
    fn applied_hotkeys_reload_after_config_round_trip() {
        let previous = live_defaults();
        let outcome = apply_hotkeys_decision(
            "Ctrl+Alt+S",
            "Ctrl+Alt+R",
            &previous,
            Ok(()),
        );
        assert!(outcome.commits_live());
        let live = live_specs_after_outcome(&previous, &outcome);
        let to_disk = hotkeys_persist_payload(&live, "Ctrl+Alt+S", "Ctrl+Alt+R");

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("yapper-hotkey-apply-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let mut cfg = Config::default();
        cfg.hotkeys = to_disk;
        cfg.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.hotkeys.read_aloud, "Ctrl+Alt+S");
        assert_eq!(loaded.hotkeys.push_to_talk, "Ctrl+Alt+R");
        // Startup register uses these live fields — same as load cycle.
        assert_eq!(
            validate_draft_hotkeys(
                &loaded.hotkeys.read_aloud,
                &loaded.hotkeys.push_to_talk
            )
            .unwrap(),
            loaded.hotkeys
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn grab_result_ignored_when_draft_invalid() {
        // Even if caller passes Ok(()), invalid draft must not commit.
        let previous = live_defaults();
        let outcome = apply_hotkeys_decision("", "Super+Shift+R", &previous, Ok(()));
        assert!(matches!(outcome, HotkeyApplyOutcome::InvalidDraft { .. }));
        assert!(!outcome.commits_live());
        assert_eq!(live_specs_after_outcome(&previous, &outcome), previous);
    }
}
