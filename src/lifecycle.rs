//! Pure shell lifecycle: hide-to-tray vs hard quit.
//!
//! Product rule: window close / minimize hides the process to the tray.
//! Only an explicit Quit intent unloads workers and ends the process.

/// What the shell should do with a window-close or tray-menu signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellIntent {
    /// Keep process + workers + hotkeys; hide the main window.
    HideToTray,
    /// Unload workers, stop playback, exit the process.
    HardQuit,
    /// Show and focus the main window (tray Open).
    ShowWindow,
    /// No window lifecycle change (e.g. load/unload menu items).
    Noop,
}

/// One-shot resolution for `--hidden` once tray creation has been attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitialVisibility {
    Noop,
    Hide,
    Reveal,
}

pub fn initial_visibility(
    start_hidden_pending: bool,
    tray_ready: bool,
    tray_attempted: bool,
) -> InitialVisibility {
    if !start_hidden_pending {
        InitialVisibility::Noop
    } else if tray_ready {
        InitialVisibility::Hide
    } else if tray_attempted {
        InitialVisibility::Reveal
    } else {
        InitialVisibility::Noop
    }
}

/// Window decoration close (X) or OS close request while we are always-on.
///
/// Always hide — never hard-quit from the title-bar close button.
pub fn close_request_intent() -> ShellIntent {
    ShellIntent::HideToTray
}

/// Minimize should match close: hide to tray, keep running.
pub fn minimize_request_intent() -> ShellIntent {
    ShellIntent::HideToTray
}

/// Map a tray menu action to a shell intent.
pub fn tray_menu_intent(action: crate::tray::TrayAction) -> ShellIntent {
    use crate::tray::TrayAction;
    match action {
        TrayAction::Open => ShellIntent::ShowWindow,
        TrayAction::Quit => ShellIntent::HardQuit,
        TrayAction::LoadStt
        | TrayAction::UnloadStt
        | TrayAction::LoadTts
        | TrayAction::UnloadTts => ShellIntent::Noop,
    }
}

/// Whether a hard quit should unload workers and allow process exit.
pub fn should_unload_on_exit(intent: ShellIntent) -> bool {
    matches!(intent, ShellIntent::HardQuit)
}

/// Whether the close frame should be cancelled (`CancelClose` + hide).
pub fn should_cancel_close(hard_quit_armed: bool) -> bool {
    !hard_quit_armed
}

/// After user clicks in-window Exit: require confirmation before HardQuit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitPromptState {
    Idle,
    AwaitingConfirm,
}

impl ExitPromptState {
    pub fn on_exit_clicked(self) -> Self {
        match self {
            Self::Idle => Self::AwaitingConfirm,
            Self::AwaitingConfirm => Self::AwaitingConfirm,
        }
    }

    pub fn on_confirm(self) -> (Self, Option<ShellIntent>) {
        match self {
            Self::AwaitingConfirm => (Self::Idle, Some(ShellIntent::HardQuit)),
            Self::Idle => (Self::Idle, None),
        }
    }

    pub fn on_cancel(self) -> Self {
        Self::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray::TrayAction;

    #[test]
    fn close_and_minimize_hide_not_quit() {
        assert_eq!(close_request_intent(), ShellIntent::HideToTray);
        assert_eq!(minimize_request_intent(), ShellIntent::HideToTray);
        assert!(!should_unload_on_exit(ShellIntent::HideToTray));
        assert!(should_cancel_close(false));
        assert!(!should_cancel_close(true));
    }

    #[test]
    fn hidden_start_requires_a_working_tray_or_reveals_recovery_ui() {
        assert_eq!(
            initial_visibility(true, true, true),
            InitialVisibility::Hide
        );
        assert_eq!(
            initial_visibility(true, false, true),
            InitialVisibility::Reveal
        );
        assert_eq!(
            initial_visibility(true, false, false),
            InitialVisibility::Noop
        );
        assert_eq!(
            initial_visibility(false, true, true),
            InitialVisibility::Noop
        );
    }

    #[test]
    fn tray_open_shows_quit_is_hard_exit() {
        assert_eq!(tray_menu_intent(TrayAction::Open), ShellIntent::ShowWindow);
        assert_eq!(tray_menu_intent(TrayAction::Quit), ShellIntent::HardQuit);
        assert!(should_unload_on_exit(ShellIntent::HardQuit));
        assert_eq!(tray_menu_intent(TrayAction::LoadStt), ShellIntent::Noop);
        assert_eq!(tray_menu_intent(TrayAction::UnloadTts), ShellIntent::Noop);
    }

    #[test]
    fn in_window_exit_requires_confirm() {
        let mut s = ExitPromptState::Idle;
        s = s.on_exit_clicked();
        assert_eq!(s, ExitPromptState::AwaitingConfirm);
        let (s2, intent) = s.on_confirm();
        assert_eq!(s2, ExitPromptState::Idle);
        assert_eq!(intent, Some(ShellIntent::HardQuit));
        let s3 = ExitPromptState::AwaitingConfirm.on_cancel();
        assert_eq!(s3, ExitPromptState::Idle);
        // Confirm without prompt does nothing
        let (s4, none) = ExitPromptState::Idle.on_confirm();
        assert_eq!(s4, ExitPromptState::Idle);
        assert_eq!(none, None);
    }
}
