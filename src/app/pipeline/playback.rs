//! Transport pump: drain ready chunks, speak status, job finalize.

use super::super::YapperApp;
use crate::audio::temp_wav_path;
use crate::transport::TransportStatus;
use crate::ui::transport_status_line;
use crate::wavutil::concat_wav_files;

impl YapperApp {
    pub(crate) fn try_play_next_ready(&mut self) {
        let status = self.transport.status();
        let can_start = matches!(
            status,
            TransportStatus::Idle | TransportStatus::Buffering
        );
        // While speaking/paused on mpv IPC, append ready chunks to the live playlist
        // instead of waiting for end-of-file + respawn.
        let can_append = matches!(status, TransportStatus::Playing | TransportStatus::Paused)
            && self.transport.can_append();
        if !can_start && !can_append {
            return;
        }
        if self.tts.ready.is_empty() {
            if can_start
                && self.tts.pending.is_empty()
                && !self.tts.synth_in_flight
            {
                self.transport.set_pending_queue(false);
                self.finalize_tts_job_if_done();
            }
            return;
        }
        // Drain ready into one persistent player session (start once, then append).
        while !self.tts.ready.is_empty() {
            let still_start = matches!(
                self.transport.status(),
                TransportStatus::Idle | TransportStatus::Buffering
            );
            let still_append = matches!(
                self.transport.status(),
                TransportStatus::Playing | TransportStatus::Paused
            ) && self.transport.can_append();
            if !still_start && !still_append {
                break;
            }
            let Some(chunk) = self.tts.pop_ready() else {
                break;
            };
            self.transport.set_pending_queue(
                !self.tts.pending.is_empty()
                    || !self.tts.ready.is_empty()
                    || self.tts.synth_in_flight,
            );
            if let Err(e) = self.transport.enqueue_or_play(&chunk.path) {
                self.status = format!("playback error: {e:#}");
                return;
            }
        }
        self.update_speak_status();
        // Keep prebuffer full while playing.
        self.pump_tts_synth();
    }

    pub(crate) fn update_speak_status(&mut self) {
        let transport_idle = matches!(self.transport.status(), TransportStatus::Idle);
        let time = self.transport.machine().format_time_label();
        let mut line = transport_status_line(
            self.tts.active_job.is_some(),
            self.tts.playing_index,
            self.tts.total,
            transport_idle,
            &time,
            self.tts.synth_in_flight,
        );
        // Keep controller progress_label wired for N/M when playing_index lags.
        let prog = self.tts.progress_label();
        if !prog.is_empty() && self.tts.active_job.is_some() && !line.contains(&prog) {
            line = format!("{line} · {prog}");
        }
        self.status = line;
    }

    pub(crate) fn finalize_tts_job_if_done(&mut self) {
        if self.tts.active_job.is_none() {
            return;
        }
        if self.tts.has_work() {
            return;
        }
        if !matches!(self.transport.status(), TransportStatus::Idle) {
            return;
        }
        // Build full-utterance WAV for Replay.
        let paths = self.tts.chunk_paths.clone();
        if !paths.is_empty() {
            let out = temp_wav_path("tts-full");
            match concat_wav_files(&paths, &out) {
                Ok(()) => {
                    // Drop intermediate chunks; keep full only.
                    for p in &paths {
                        if p != &out {
                            let _ = std::fs::remove_file(p);
                        }
                    }
                    self.tts.chunk_paths = vec![out.clone()];
                    self.tts_last_full_path = Some(out.clone());
                    self.transport.remember_path(out);
                }
                Err(e) => {
                    // Fallback: last chunk only.
                    if let Some(last) = paths.last() {
                        self.tts_last_full_path = Some(last.clone());
                        self.transport.remember_path(last.clone());
                    }
                    self.status = format!("ready (full concat failed: {e:#})");
                    self.tts.active_job = None;
                    self.tts.total = 0;
                    return;
                }
            }
        }
        self.tts.active_job = None;
        self.tts.total = 0;
        self.status = "ready".into();
    }

    pub(crate) fn poll_transport(&mut self) {
        self.transport.tick();
        // Idle/buffering: start; playing/paused: append ready chunks to live playlist.
        match self.transport.status() {
            TransportStatus::Idle | TransportStatus::Buffering => {
                self.try_play_next_ready();
            }
            TransportStatus::Playing => {
                self.update_speak_status();
                self.pump_tts_synth();
                self.try_play_next_ready();
            }
            TransportStatus::Paused => {
                self.status = format!(
                    "paused ({})",
                    self.transport.machine().format_time_label()
                );
                self.try_play_next_ready();
            }
        }
        if matches!(self.transport.status(), TransportStatus::Idle)
            && !self.tts.has_work()
            && self.tts.active_job.is_some()
        {
            self.finalize_tts_job_if_done();
        }
    }
}
