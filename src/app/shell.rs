//! YapperApp shell: hotkeys, tray, hide/quit lifecycle.

use super::{egui_key_to_token, CaptureOutcome, HotkeyCaptureField, YapperApp};
use crate::audio::stop_recording;
use crate::hotkeys::{
    canonicalize_hotkey_spec, capture_mod_state, format_capture_hotkey, reregister, HotkeyAction,
};
use crate::lifecycle::{
    close_request_intent, minimize_request_intent, should_cancel_close, should_unload_on_exit,
    tray_menu_intent, ShellIntent,
};
use crate::tray::{tray_failure_hint, TrayAction, TrayHandle};
use crate::x11util;
use eframe::egui;
use std::time::{Duration, Instant};

impl YapperApp {
    pub(crate) fn apply_hotkeys(&mut self) {
        let read_canon = match canonicalize_hotkey_spec(&self.cfg.hotkeys.read_aloud) {
            Ok(s) => s,
            Err(e) => {
                self.hotkey_error = Some(format!("read-aloud invalid: {e:#}"));
                self.status = "hotkey update failed".into();
                return;
            }
        };
        let ptt_canon = match canonicalize_hotkey_spec(&self.cfg.hotkeys.push_to_talk) {
            Ok(s) => s,
            Err(e) => {
                self.hotkey_error = Some(format!("push-to-talk invalid: {e:#}"));
                self.status = "hotkey update failed".into();
                return;
            }
        };
        self.cfg.hotkeys.read_aloud = read_canon;
        self.cfg.hotkeys.push_to_talk = ptt_canon;
        self.persist();
        // Drop previous grabs *before* registering — double-register is the B13 bug.
        let previous = self.hotkeys.take();
        match reregister(
            previous,
            &self.cfg.hotkeys.read_aloud,
            &self.cfg.hotkeys.push_to_talk,
        ) {
            Ok(h) => {
                let specs = h.registered_specs().unwrap_or_default();
                self.hotkeys = Some(h);
                self.hotkey_error = None;
                self.hotkey_capture = None;
                self.status = if specs.is_empty() {
                    format!(
                        "hotkeys live: {} | {}",
                        self.cfg.hotkeys.read_aloud, self.cfg.hotkeys.push_to_talk
                    )
                } else {
                    format!("hotkeys live: {}", specs.join(" | "))
                };
            }
            Err(e) => {
                // Leave hub empty so we never claim grabs we do not hold.
                self.hotkeys = None;
                self.hotkey_error = Some(format!(
                    "hotkey grab failed (DE conflict or bad combo): {e:#}. \
                     Rebind or free the shortcut in system settings, then Apply again."
                ));
                self.status = "hotkey update failed — shortcuts inactive until fixed".into();
            }
        }
    }

    pub(crate) fn hide_to_tray(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        // Also clear minimized state so Open restores cleanly.
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        self.status = "hidden — tray menu: Open / Quit".into();
    }

    pub(crate) fn show_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        self.status = "window focused".into();
    }

    pub(crate) fn arm_hard_quit_and_close(&mut self, ctx: &egui::Context) {
        // Hide/open must never unload; only HardQuit.
        if !should_unload_on_exit(ShellIntent::HardQuit) {
            return;
        }
        self.hard_quit_armed = true;
        self.discard_all_tts_audio();
        // Immediate OOB kill so mid-generate cannot block unload; on_exit joins.
        let _ = self.jobs.kill_all_now();
        self.jobs.send(crate::app::messages::JobCmd::UnloadAll);
        self.jobs.send(crate::app::messages::JobCmd::Shutdown);
        if let Some(session) = self.recording.take() {
            let _ = stop_recording(session);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Stop queue + playback but **keep** last successful WAV for Replay.
    pub(crate) fn handle_viewport_lifecycle(&mut self, ctx: &egui::Context) {
        let (close_req, minimized) = ctx.input(|i| {
            let vp = i.viewport();
            (vp.close_requested(), vp.minimized.unwrap_or(false))
        });

        if close_req {
            if should_cancel_close(self.hard_quit_armed) {
                // Always-on product: close button hides, does not exit.
                let _ = close_request_intent();
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.hide_to_tray(ctx);
            }
            // else: hard quit armed — allow process exit
            return;
        }

        if minimized {
            let _ = minimize_request_intent();
            self.hide_to_tray(ctx);
        }
    }

    /// While a capture field is active, turn the next non-modifier key press into a binding.
    pub(crate) fn poll_hotkey_capture(&mut self, ctx: &egui::Context) {
        let Some(field) = self.hotkey_capture else {
            return;
        };

        let outcome = ctx.input(|i| {
            if i.key_pressed(egui::Key::Escape) {
                return Some(CaptureOutcome::Cancel);
            }
            for ev in &i.events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    repeat,
                    ..
                } = ev
                {
                    if *repeat {
                        continue;
                    }
                    if *key == egui::Key::Escape {
                        return Some(CaptureOutcome::Cancel);
                    }
                    let Some(token) = egui_key_to_token(*key) else {
                        continue;
                    };
                    // egui drops Super on Linux (mac_cmd always false; command=ctrl).
                    // Read Super/Mod4 from X11 session state during capture.
                    let platform_super = x11util::super_modifier_down();
                    let mods = capture_mod_state(
                        modifiers.mac_cmd,
                        modifiers.ctrl,
                        modifiers.alt,
                        modifiers.shift,
                        platform_super,
                    );
                    match format_capture_hotkey(mods, token) {
                        Ok(spec) => return Some(CaptureOutcome::Bound(spec)),
                        Err(e) => return Some(CaptureOutcome::Error(format!("{e:#}"))),
                    }
                }
            }
            None
        });

        match outcome {
            Some(CaptureOutcome::Cancel) => {
                self.hotkey_capture = None;
                self.status = "hotkey capture cancelled".into();
            }
            Some(CaptureOutcome::Bound(spec)) => {
                match field {
                    HotkeyCaptureField::ReadAloud => self.cfg.hotkeys.read_aloud = spec.clone(),
                    HotkeyCaptureField::PushToTalk => self.cfg.hotkeys.push_to_talk = spec.clone(),
                }
                self.hotkey_capture = None;
                self.hotkey_error = None;
                self.status = format!("captured {spec} (Apply to register)");
            }
            Some(CaptureOutcome::Error(msg)) => {
                self.hotkey_error = Some(msg);
                self.status = "hotkey capture failed".into();
                self.hotkey_capture = None;
            }
            None => {}
        }
    }

    pub(crate) fn poll_hotkeys(&mut self) {
        let events: Vec<_> = if let Some(hk) = self.hotkeys.as_ref() {
            hk.poll_events()
        } else {
            Vec::new()
        };
        for ev in events {
            match (ev.action, ev.pressed) {
                (HotkeyAction::ReadAloud, true) => self.read_aloud(),
                (HotkeyAction::PushToTalk, true) => self.ptt_press(),
                (HotkeyAction::PushToTalk, false) => self.ptt_release(),
                _ => {}
            }
        }
    }

    pub(crate) fn ensure_tray(&mut self) {
        if self.tray.is_some() {
            return;
        }
        // Retry with backoff while create fails (SNI host / display race).
        if let Some(at) = self.tray_retry_at {
            if Instant::now() < at {
                return;
            }
        }
        self.tray_tried = true;
        match TrayHandle::try_create() {
            Ok(t) => {
                self.tray = Some(t);
                self.tray_error = None;
                self.tray_retry_at = None;
            }
            Err(e) => {
                self.tray_error = Some(format!(
                    "TRAY MISSING — always-on shell is broken without an icon. {e:#}\n{}",
                    tray_failure_hint()
                ));
                // Keep retrying every 3s for a while after start.
                self.tray_retry_at = Some(Instant::now() + Duration::from_secs(3));
            }
        }
    }

    pub(crate) fn poll_tray(&mut self, ctx: &egui::Context) {
        self.ensure_tray();
        let actions: Vec<TrayAction> = if let Some(t) = self.tray.as_ref() {
            std::iter::from_fn(|| t.try_recv()).collect()
        } else {
            Vec::new()
        };
        for a in actions {
            match tray_menu_intent(a) {
                ShellIntent::ShowWindow => self.show_window(ctx),
                ShellIntent::HardQuit => self.arm_hard_quit_and_close(ctx),
                ShellIntent::HideToTray => self.hide_to_tray(ctx),
                ShellIntent::Noop => match a {
                    TrayAction::LoadStt => self.load_stt(),
                    TrayAction::UnloadStt => self.unload_stt(),
                    TrayAction::LoadTts => self.load_tts(),
                    TrayAction::UnloadTts => self.unload_tts(),
                    _ => {}
                },
            }
        }
    }

}
