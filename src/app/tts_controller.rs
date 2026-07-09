//! Pure TTS prebuffer / job-id state machine (unit-tested, no I/O).

use std::collections::VecDeque;
use std::path::PathBuf;

/// Max synthesized chunks waiting for playback (disk/VRAM bound).
pub const MAX_READY_CHUNKS: usize = 2;
/// Retry a failed segment once before skip.
pub const MAX_CHUNK_RETRIES: u8 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct ReadyChunk {
    pub index: usize,
    pub text: String,
    pub path: PathBuf,
    pub duration_secs: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingSegment {
    pub index: usize,
    pub text: String,
    pub retries: u8,
}

/// Bounded prebuffer controller for multi-chunk TTS.
#[derive(Debug, Default)]
pub struct TtsController {
    pub job_id: u64,
    pub active_job: Option<u64>,
    pub total: usize,
    pub pending: VecDeque<PendingSegment>,
    pub ready: VecDeque<ReadyChunk>,
    pub synth_in_flight: bool,
    /// Paths produced by the current job (for full-utterance concat).
    pub chunk_paths: Vec<PathBuf>,
    pub finished: bool,
    /// Index of the chunk currently handed to the transport (for N/M chrome).
    pub playing_index: Option<usize>,
}

impl TtsController {
    pub fn next_job_id(&mut self) -> u64 {
        self.job_id = self.job_id.wrapping_add(1).max(1);
        self.job_id
    }

    /// Start a new multi-segment job; cancels prior logical job.
    pub fn begin_job(&mut self, segments: Vec<String>) -> u64 {
        let id = self.next_job_id();
        self.active_job = Some(id);
        self.total = segments.len();
        self.pending = segments
            .into_iter()
            .enumerate()
            .map(|(i, text)| PendingSegment {
                index: i,
                text,
                retries: 0,
            })
            .collect();
        self.ready.clear();
        self.synth_in_flight = false;
        self.chunk_paths.clear();
        self.finished = false;
        self.playing_index = None;
        id
    }

    pub fn cancel(&mut self) {
        // Bump generation so stale chunks are ignored; clear queues.
        let _ = self.next_job_id();
        self.active_job = None;
        self.pending.clear();
        self.ready.clear();
        self.synth_in_flight = false;
        self.total = 0;
        self.finished = true;
        self.playing_index = None;
        // Keep chunk_paths for possible partial replay handled by caller.
    }

    pub fn is_live(&self, job_id: u64) -> bool {
        self.active_job == Some(job_id)
    }

    /// Whether we should request the next synth (room in ready queue).
    pub fn should_request_synth(&self) -> bool {
        self.active_job.is_some()
            && !self.synth_in_flight
            && !self.pending.is_empty()
            && self.ready.len() < MAX_READY_CHUNKS
    }

    /// Peek next pending segment for synth request.
    pub fn peek_pending(&self) -> Option<&PendingSegment> {
        self.pending.front()
    }

    pub fn mark_synth_started(&mut self) {
        self.synth_in_flight = true;
    }

    /// On successful chunk: pop pending head if index matches, push ready.
    pub fn on_chunk_ready(
        &mut self,
        job_id: u64,
        index: usize,
        text: String,
        path: PathBuf,
        duration_secs: f64,
    ) -> bool {
        if !self.is_live(job_id) {
            return false;
        }
        self.synth_in_flight = false;
        if let Some(front) = self.pending.front() {
            if front.index == index {
                self.pending.pop_front();
            }
        }
        self.chunk_paths.push(path.clone());
        self.ready.push_back(ReadyChunk {
            index,
            text,
            path,
            duration_secs,
        });
        self.check_finished();
        true
    }

    /// Failed chunk: retry once then skip.
    /// Returns `Some(segment)` if a retry should be re-queued at front.
    pub fn on_chunk_failed(&mut self, job_id: u64, index: usize) -> Option<PendingSegment> {
        if !self.is_live(job_id) {
            return None;
        }
        self.synth_in_flight = false;
        let Some(front) = self.pending.front().cloned() else {
            return None;
        };
        if front.index != index {
            return None;
        }
        self.pending.pop_front();
        if front.retries < MAX_CHUNK_RETRIES {
            let mut retry = front;
            retry.retries += 1;
            self.pending.push_front(retry.clone());
            return Some(retry);
        }
        // Skipped after max retries.
        self.check_finished();
        None
    }

    pub fn pop_ready(&mut self) -> Option<ReadyChunk> {
        let chunk = self.ready.pop_front()?;
        self.playing_index = Some(chunk.index);
        Some(chunk)
    }

    pub fn has_work(&self) -> bool {
        self.active_job.is_some()
            && (!self.pending.is_empty() || !self.ready.is_empty() || self.synth_in_flight)
    }

    fn check_finished(&mut self) {
        if self.pending.is_empty() && self.ready.is_empty() && !self.synth_in_flight {
            self.finished = true;
        }
    }

    /// Progress label: sentence N/M (playing) or synthesized count.
    pub fn progress_label(&self) -> String {
        if self.total == 0 {
            return String::new();
        }
        if let Some(idx) = self.playing_index {
            return format!("{}/{}", idx + 1, self.total);
        }
        let synthesized = self.total.saturating_sub(self.pending.len());
        if self.ready.is_empty() {
            format!("{synthesized}/{}", self.total)
        } else {
            format!(
                "{}/{} ({} ready)",
                synthesized,
                self.total,
                self.ready.len()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_job_chunks_ignored() {
        let mut c = TtsController::default();
        let id = c.begin_job(vec!["a".into(), "b".into()]);
        c.cancel();
        assert!(!c.on_chunk_ready(id, 0, "a".into(), PathBuf::from("/tmp/a.wav"), 1.0));
        assert!(c.ready.is_empty());
    }

    #[test]
    fn prebuffer_caps_ready_and_requests() {
        let mut c = TtsController::default();
        let id = c.begin_job(vec!["a".into(), "b".into(), "c".into()]);
        assert!(c.should_request_synth());
        c.mark_synth_started();
        assert!(!c.should_request_synth());
        assert!(c.on_chunk_ready(id, 0, "a".into(), PathBuf::from("/a"), 1.0));
        assert!(c.on_chunk_ready(id, 1, "b".into(), PathBuf::from("/b"), 1.0));
        // ready has 2; pending still has c but cap blocks until pop
        assert_eq!(c.ready.len(), 2);
        assert!(!c.should_request_synth() || c.ready.len() >= MAX_READY_CHUNKS);
        // After fixing: synth_in_flight false, ready==2 → should NOT request
        assert!(!c.should_request_synth());
        let _ = c.pop_ready();
        assert!(c.should_request_synth());
    }

    #[test]
    fn retry_once_then_skip() {
        let mut c = TtsController::default();
        let id = c.begin_job(vec!["bad".into(), "ok".into()]);
        c.mark_synth_started();
        let retry = c.on_chunk_failed(id, 0);
        assert!(retry.is_some());
        assert_eq!(c.pending.front().unwrap().retries, 1);
        c.mark_synth_started();
        assert!(c.on_chunk_failed(id, 0).is_none());
        // skipped; next is "ok"
        assert_eq!(c.pending.front().unwrap().text, "ok");
    }

    #[test]
    fn full_job_paths_collected_for_replay() {
        let mut c = TtsController::default();
        let id = c.begin_job(vec!["a".into(), "b".into()]);
        c.mark_synth_started();
        c.on_chunk_ready(id, 0, "a".into(), PathBuf::from("/a.wav"), 1.0);
        c.mark_synth_started();
        c.on_chunk_ready(id, 1, "b".into(), PathBuf::from("/b.wav"), 1.0);
        assert_eq!(
            c.chunk_paths,
            vec![PathBuf::from("/a.wav"), PathBuf::from("/b.wav")]
        );
    }
}
