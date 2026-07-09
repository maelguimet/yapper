//! Controllable TTS playback transport (pure state machine + mpv IPC backend).
//!
//! Multi-chunk Speak prefers **one** mpv process: the first segment starts the
//! player; later segments `loadfile … append` onto the same playlist so
//! inter-sentence gaps are not driven by per-chunk process spawn.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use crate::mpv_backend::{which_bin, wav_duration_secs, MpvBackend};

// Re-export pure machine types for existing callers.
pub use crate::transport_machine::{TransportMachine, TransportStatus};

// Re-export for callers/tests that timed WAV duration via transport path.
#[cfg(test)]
pub use crate::mpv_backend::wav_duration_from_bytes;

/// Controllable player using `mpv --input-ipc-server` when available.
pub struct AudioTransport {
    machine: TransportMachine,
    backend: Option<MpvBackend>,
    /// Volume 0.0..=1.0
    volume: f32,
    /// Throttle position/duration IPC polls (not every frame).
    last_pos_poll: Option<std::time::Instant>,
    /// Cached duration for current file (avoid re-query every tick).
    duration_known: bool,
    /// How many files were started or appended on the live backend this session
    /// (tests: proves multi-chunk did not respawn per sentence).
    #[cfg(test)]
    files_on_session: u32,
}

impl Default for AudioTransport {
    fn default() -> Self {
        Self {
            machine: TransportMachine::default(),
            backend: None,
            volume: 1.0,
            last_pos_poll: None,
            duration_known: false,
            #[cfg(test)]
            files_on_session: 0,
        }
    }
}

/// Minimum interval between mpv position polls (~8 Hz).
const POS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(125);

impl AudioTransport {
    pub fn machine(&self) -> &TransportMachine {
        &self.machine
    }

    pub fn status(&self) -> TransportStatus {
        self.machine.status
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        if let Some(b) = self.backend.as_mut() {
            let _ = b.set_volume((self.volume * 100.0) as i32);
        }
    }

    /// True when pause/seek are backed by mpv IPC (not ffplay/paplay fallback).
    pub fn supports_transport_controls(&self) -> bool {
        self.backend
            .as_ref()
            .map(|b| b.has_ipc())
            .unwrap_or(false)
    }

    /// Live IPC backend that can accept playlist-append (no respawn).
    pub fn can_append(&mut self) -> bool {
        self.backend
            .as_mut()
            .map(|b| b.can_append())
            .unwrap_or(false)
    }

    /// Files loaded onto the current backend session (1 = single start; N = appends).
    #[cfg(test)]
    pub fn files_on_session(&self) -> u32 {
        self.files_on_session
    }

    /// Play a single WAV/audio file (stops any current playback first).
    ///
    /// Used by Replay and any explicit replace. Multi-chunk Speak prefers
    /// [`Self::enqueue_or_play`] so successive sentences share one mpv process.
    pub fn play_file(&mut self, path: &Path) -> Result<()> {
        self.stop_internal(false);
        self.duration_known = false;
        #[cfg(test)]
        {
            self.files_on_session = 0;
        }
        if !path.is_file() {
            bail!("audio file missing: {}", path.display());
        }
        let duration = wav_duration_secs(path).unwrap_or(0.0);
        self.machine.begin_buffering();
        match MpvBackend::start(path, self.volume) {
            Ok(backend) => {
                self.backend = Some(backend);
                #[cfg(test)]
                {
                    self.files_on_session = 1;
                }
                self.machine
                    .on_audio_ready(path.to_path_buf(), duration);
                self.duration_known = duration > 0.0;
                Ok(())
            }
            Err(e) => {
                // Fallback: legacy fire-and-forget if mpv missing.
                if which_bin("ffplay") || which_bin("paplay") || which_bin("aplay") {
                    let child = crate::audio::play_wav(path)?;
                    self.backend = Some(MpvBackend::fallback_child(child));
                    #[cfg(test)]
                    {
                        self.files_on_session = 1;
                    }
                    self.machine
                        .on_audio_ready(path.to_path_buf(), duration);
                    self.duration_known = duration > 0.0;
                    Ok(())
                } else {
                    self.machine.stop();
                    Err(e)
                }
            }
        }
    }

    /// Start playback or append onto the live mpv playlist.
    ///
    /// When an IPC-backed session is already running, the file is appended
    /// (`loadfile … append`) — no kill/respawn. Fallback players cannot append
    /// and only accept work when idle/buffering (caller should wait for end).
    pub fn enqueue_or_play(&mut self, path: &Path) -> Result<()> {
        if !path.is_file() {
            bail!("audio file missing: {}", path.display());
        }
        let duration = wav_duration_secs(path).unwrap_or(0.0);

        if self.can_append() {
            let backend = self.backend.as_mut().expect("can_append implies backend");
            match backend.append_file(path) {
                Ok(()) => {
                    #[cfg(test)]
                    {
                        self.files_on_session = self.files_on_session.saturating_add(1);
                    }
                    self.machine
                        .on_chunk_appended(path.to_path_buf(), duration);
                    return Ok(());
                }
                Err(e) => {
                    // Process may have raced to exit; fall through to fresh start.
                    eprintln!("yapper: playlist append failed ({e:#}); restarting player");
                    if let Some(mut b) = self.backend.take() {
                        b.kill();
                    }
                }
            }
        }

        // Fallback (no IPC): only start when not already holding a child.
        if let Some(b) = self.backend.as_mut() {
            if !b.has_ipc() && !b.is_ended() {
                bail!("fallback player busy; wait for end before next chunk");
            }
        }

        self.play_file(path)
    }

    pub fn pause(&mut self) {
        if let Some(b) = self.backend.as_mut() {
            let _ = b.pause(true);
        }
        self.machine.pause();
    }

    pub fn resume(&mut self) {
        if let Some(b) = self.backend.as_mut() {
            let _ = b.pause(false);
        }
        self.machine.resume();
    }

    pub fn toggle_pause(&mut self) {
        match self.machine.status {
            TransportStatus::Playing => self.pause(),
            TransportStatus::Paused => self.resume(),
            _ => {}
        }
    }

    pub fn stop(&mut self) {
        self.stop_internal(true);
    }

    /// Clear Replay path (unload / full discard).
    pub fn clear_last_path(&mut self) {
        self.machine.last_path = None;
    }

    fn stop_internal(&mut self, keep_last: bool) {
        if let Some(mut b) = self.backend.take() {
            b.kill();
        }
        let last = self.machine.last_path.clone();
        self.machine.stop();
        self.last_pos_poll = None;
        self.duration_known = false;
        #[cfg(test)]
        {
            self.files_on_session = 0;
        }
        if keep_last {
            self.machine.last_path = last;
        } else {
            // Drop last_path only when caller wants a clean slate (new play replaces it).
            // play_file uses keep_last=false then sets a new path via on_audio_ready.
            self.machine.last_path = None;
        }
    }

    /// Replay last successful file without re-synthesis.
    ///
    /// Returns `Ok(false)` when there is no last path, or the file was deleted.
    pub fn replay(&mut self) -> Result<bool> {
        let Some(path) = self.machine.replay_request() else {
            return Ok(false);
        };
        if !path.is_file() {
            self.machine.last_path = None;
            self.machine.stop();
            return Ok(false);
        }
        self.play_file(&path)?;
        Ok(true)
    }

    /// Point Replay at a durable file without playing yet (e.g. after Stop cleanup).
    pub fn remember_path(&mut self, path: PathBuf) {
        if path.is_file() {
            self.machine.last_path = Some(path);
        }
    }

    pub fn seek_secs(&mut self, secs: f64) {
        let pos = self.machine.seek_to(secs);
        if let Some(b) = self.backend.as_mut() {
            let _ = b.seek_absolute(pos);
        }
    }

    pub fn seek_progress(&mut self, progress_01: f32) {
        let dur = self.machine.duration_secs;
        self.seek_secs(dur * progress_01.clamp(0.0, 1.0) as f64);
    }

    /// Poll backend; update position (throttled); detect end-of-playlist/process.
    pub fn tick(&mut self) {
        let now = std::time::Instant::now();
        let should_poll_pos = self
            .last_pos_poll
            .map(|t| now.duration_since(t) >= POS_POLL_INTERVAL)
            .unwrap_or(true);

        let ended = match self.backend.as_mut() {
            Some(b) => {
                if should_poll_pos {
                    self.last_pos_poll = Some(now);
                    if let Some(pos) = b.time_pos() {
                        self.machine.set_position(pos);
                    }
                    // Always refresh duration from mpv. With playlist-append the
                    // current item changes without a new process; caching only the
                    // first WAV's duration would clamp Seek to chunk-0 bounds.
                    if let Some(dur) = b.duration() {
                        if dur > 0.0 {
                            self.machine.duration_secs = dur;
                            self.duration_known = true;
                        }
                    }
                }
                b.is_ended()
            }
            None => false,
        };
        if ended {
            if let Some(mut b) = self.backend.take() {
                b.kill();
            }
            #[cfg(test)]
            {
                self.files_on_session = 0;
            }
            self.machine.on_playback_ended();
            self.last_pos_poll = None;
        }
    }

    pub fn set_pending_queue(&mut self, pending: bool) {
        self.machine.has_pending_queue = pending;
        if pending && self.machine.status == TransportStatus::Idle {
            self.machine.begin_buffering();
        }
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
