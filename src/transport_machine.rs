//! Pure transport intent / transition table (no I/O).
//!
//! Kept separate from [`crate::transport::AudioTransport`] so multi-chunk queue
//! transitions stay unit-testable without a live player.

use std::path::PathBuf;

/// High-level playback status shown in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportStatus {
    Idle,
    Buffering,
    Playing,
    Paused,
}

impl TransportStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Buffering => "buffering",
            Self::Playing => "speaking",
            Self::Paused => "paused",
        }
    }
}

/// Pure transport state machine (no I/O).
#[derive(Debug, Clone, PartialEq)]
pub struct TransportMachine {
    pub status: TransportStatus,
    /// Seconds into the current timeline (whole utterance or active chunk).
    pub position_secs: f64,
    pub duration_secs: f64,
    /// Path of last successfully loaded audio (for Replay without re-synth).
    pub last_path: Option<PathBuf>,
    /// True when a queue is still synthesizing more segments.
    pub has_pending_queue: bool,
}

impl Default for TransportMachine {
    fn default() -> Self {
        Self {
            status: TransportStatus::Idle,
            position_secs: 0.0,
            duration_secs: 0.0,
            last_path: None,
            has_pending_queue: false,
        }
    }
}

impl TransportMachine {
    pub fn begin_buffering(&mut self) {
        self.status = TransportStatus::Buffering;
        self.position_secs = 0.0;
        self.duration_secs = 0.0;
    }

    pub fn on_audio_ready(&mut self, path: PathBuf, duration_secs: f64) {
        self.last_path = Some(path);
        self.duration_secs = duration_secs.max(0.0);
        self.position_secs = 0.0;
        self.status = TransportStatus::Playing;
    }

    /// Subsequent chunk joined the live playlist (no new process). Stay Playing
    /// or resume from Buffering when the previous playlist drained.
    pub fn on_chunk_appended(&mut self, path: PathBuf, duration_secs: f64) {
        self.last_path = Some(path);
        match self.status {
            TransportStatus::Idle | TransportStatus::Buffering => {
                self.duration_secs = duration_secs.max(0.0);
                self.position_secs = 0.0;
                self.status = TransportStatus::Playing;
            }
            TransportStatus::Playing | TransportStatus::Paused => {
                // Keep current timeline; mpv advances playlist items itself.
            }
        }
    }

    pub fn pause(&mut self) {
        if self.status == TransportStatus::Playing {
            self.status = TransportStatus::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.status == TransportStatus::Paused {
            self.status = TransportStatus::Playing;
        }
    }

    pub fn stop(&mut self) {
        self.status = TransportStatus::Idle;
        self.position_secs = 0.0;
        // Keep last_path for replay.
        self.has_pending_queue = false;
    }

    /// Replay last path if any; returns the path to play or None.
    pub fn replay_request(&mut self) -> Option<PathBuf> {
        let path = self.last_path.clone()?;
        self.position_secs = 0.0;
        self.status = TransportStatus::Playing;
        Some(path)
    }

    /// Clamp seek into `[0, duration]`. Returns the clamped position.
    pub fn seek_to(&mut self, secs: f64) -> f64 {
        if self.duration_secs <= 0.0 {
            self.position_secs = 0.0;
            return 0.0;
        }
        let pos = secs.clamp(0.0, self.duration_secs);
        self.position_secs = pos;
        if self.status == TransportStatus::Idle && self.last_path.is_some() {
            self.status = TransportStatus::Paused;
        }
        pos
    }

    pub fn set_position(&mut self, secs: f64) {
        if self.duration_secs > 0.0 {
            self.position_secs = secs.clamp(0.0, self.duration_secs);
        } else {
            self.position_secs = secs.max(0.0);
        }
    }

    pub fn on_playback_ended(&mut self) {
        if self.has_pending_queue {
            self.status = TransportStatus::Buffering;
            self.position_secs = 0.0;
            self.duration_secs = 0.0;
        } else {
            self.status = TransportStatus::Idle;
            self.position_secs = self.duration_secs;
        }
    }

    pub fn progress_01(&self) -> f32 {
        if self.duration_secs <= 0.0 {
            return 0.0;
        }
        (self.position_secs / self.duration_secs).clamp(0.0, 1.0) as f32
    }

    pub fn format_time_label(&self) -> String {
        format!(
            "{} / {}",
            format_mmss(self.position_secs),
            format_mmss(self.duration_secs)
        )
    }
}

pub fn format_mmss(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "0:00".into();
    }
    let total = secs.floor() as u64;
    let m = total / 60;
    let s = total % 60;
    format!("{m}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_play_pause_resume_stop() {
        let mut m = TransportMachine::default();
        assert_eq!(m.status, TransportStatus::Idle);
        m.begin_buffering();
        assert_eq!(m.status, TransportStatus::Buffering);
        m.on_audio_ready(PathBuf::from("/tmp/a.wav"), 10.0);
        assert_eq!(m.status, TransportStatus::Playing);
        assert_eq!(m.duration_secs, 10.0);
        m.pause();
        assert_eq!(m.status, TransportStatus::Paused);
        m.resume();
        assert_eq!(m.status, TransportStatus::Playing);
        m.stop();
        assert_eq!(m.status, TransportStatus::Idle);
        assert!(m.last_path.is_some());
    }

    #[test]
    fn machine_seek_bounds() {
        let mut m = TransportMachine::default();
        m.on_audio_ready(PathBuf::from("/tmp/a.wav"), 12.0);
        assert_eq!(m.seek_to(-5.0), 0.0);
        assert_eq!(m.seek_to(100.0), 12.0);
        assert_eq!(m.seek_to(3.5), 3.5);
        assert!((m.progress_01() - (3.5 / 12.0) as f32).abs() < 0.001);
    }

    #[test]
    fn machine_replay_without_resynth() {
        let mut m = TransportMachine::default();
        assert!(m.replay_request().is_none());
        m.on_audio_ready(PathBuf::from("/tmp/last.wav"), 5.0);
        m.stop();
        // last_path survives stop for Replay without re-synth
        assert_eq!(m.last_path, Some(PathBuf::from("/tmp/last.wav")));
        let p = m.replay_request().unwrap();
        assert_eq!(p, PathBuf::from("/tmp/last.wav"));
        assert_eq!(m.status, TransportStatus::Playing);
        assert_eq!(m.position_secs, 0.0);
    }

    #[test]
    fn machine_end_with_pending_goes_buffering() {
        let mut m = TransportMachine::default();
        m.on_audio_ready(PathBuf::from("/tmp/a.wav"), 2.0);
        m.has_pending_queue = true;
        m.on_playback_ended();
        assert_eq!(m.status, TransportStatus::Buffering);
    }

    #[test]
    fn multi_chunk_ready_play_ordering_stays_playing_on_append() {
        let mut m = TransportMachine::default();
        m.has_pending_queue = true;
        m.begin_buffering();
        m.on_audio_ready(PathBuf::from("/tmp/c0.wav"), 1.0);
        assert_eq!(m.status, TransportStatus::Playing);
        // Next segment joins playlist — no process spawn, status stays Playing.
        m.on_chunk_appended(PathBuf::from("/tmp/c1.wav"), 1.0);
        assert_eq!(m.status, TransportStatus::Playing);
        m.on_chunk_appended(PathBuf::from("/tmp/c2.wav"), 1.0);
        assert_eq!(m.status, TransportStatus::Playing);
        assert_eq!(m.last_path, Some(PathBuf::from("/tmp/c2.wav")));
    }

    #[test]
    fn multi_chunk_playlist_drain_then_buffer_then_append_resumes() {
        let mut m = TransportMachine::default();
        m.has_pending_queue = true;
        m.on_audio_ready(PathBuf::from("/tmp/c0.wav"), 1.0);
        m.on_playback_ended();
        assert_eq!(m.status, TransportStatus::Buffering);
        // Late chunk after underrun: append path treats Buffering → Playing.
        m.on_chunk_appended(PathBuf::from("/tmp/c1.wav"), 1.2);
        assert_eq!(m.status, TransportStatus::Playing);
        assert!((m.duration_secs - 1.2).abs() < 0.001);
        m.has_pending_queue = false;
        m.on_playback_ended();
        assert_eq!(m.status, TransportStatus::Idle);
    }

    #[test]
    fn stop_mid_queue_clears_pending_keeps_last_for_replay() {
        let mut m = TransportMachine::default();
        m.has_pending_queue = true;
        m.on_audio_ready(PathBuf::from("/tmp/c0.wav"), 1.0);
        m.on_chunk_appended(PathBuf::from("/tmp/c1.wav"), 1.0);
        m.stop();
        assert_eq!(m.status, TransportStatus::Idle);
        assert!(!m.has_pending_queue, "stop must clear pending queue flag");
        assert_eq!(
            m.last_path,
            Some(PathBuf::from("/tmp/c1.wav")),
            "last_path survives for Replay"
        );
        // Stale ready chunks must not auto-advance after stop (caller clears them).
        m.on_playback_ended();
        assert_eq!(
            m.status,
            TransportStatus::Idle,
            "without pending, end stays idle — no stale advance"
        );
    }

    #[test]
    fn replay_request_points_at_full_utterance_path() {
        let mut m = TransportMachine::default();
        // Multi-chunk job finished: durable concat replaces per-chunk last_path.
        m.on_audio_ready(PathBuf::from("/tmp/chunk-last.wav"), 1.0);
        m.stop();
        m.last_path = Some(PathBuf::from("/tmp/full-utterance.wav"));
        let p = m.replay_request().expect("replay path");
        assert_eq!(p, PathBuf::from("/tmp/full-utterance.wav"));
        assert_eq!(m.status, TransportStatus::Playing);
        assert_eq!(m.position_secs, 0.0);
    }

    #[test]
    fn format_mmss_values() {
        assert_eq!(format_mmss(0.0), "0:00");
        assert_eq!(format_mmss(65.2), "1:05");
        assert_eq!(format_mmss(-1.0), "0:00");
    }

    #[test]
    fn status_labels() {
        assert_eq!(TransportStatus::Playing.as_str(), "speaking");
        assert_eq!(TransportStatus::Paused.as_str(), "paused");
    }

    #[test]
    fn time_label_format() {
        let mut m = TransportMachine::default();
        m.on_audio_ready(PathBuf::from("/tmp/a.wav"), 90.0);
        m.set_position(45.0);
        assert_eq!(m.format_time_label(), "0:45 / 1:30");
    }
}
