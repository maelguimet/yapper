//! Out-of-band worker PID registry + hard-quit join budget helpers.
//!
//! UI / hard-quit can SIGKILL without waiting on the serial jobs thread.

use crate::ipc::kill_os_pid;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// Default hard-quit join budget for the jobs thread. Short so exit never hangs.
pub const JOB_SHUTDOWN_JOIN_BUDGET: Duration = Duration::from_millis(1500);

/// Pure: whether a jobs-thread join wait has exhausted its budget.
pub fn job_shutdown_join_exceeded(elapsed: Duration, budget: Duration) -> bool {
    elapsed >= budget
}

/// Shared live worker pids. UI can SIGKILL without waiting on the job thread.
#[derive(Debug, Default)]
pub struct LiveWorkerPids {
    /// 0 = none
    tts: AtomicU32,
    stt: AtomicU32,
}

impl LiveWorkerPids {
    pub fn set_tts(&self, pid: Option<u32>) {
        self.tts.store(pid.unwrap_or(0), Ordering::SeqCst);
    }

    pub fn set_stt(&self, pid: Option<u32>) {
        self.stt.store(pid.unwrap_or(0), Ordering::SeqCst);
    }

    /// Immediately SIGKILL the TTS worker process (if registered).
    /// Returns true if a kill was attempted.
    pub fn kill_tts_now(&self) -> bool {
        let pid = self.tts.swap(0, Ordering::SeqCst);
        if pid == 0 {
            return false;
        }
        kill_os_pid(pid);
        true
    }

    /// Immediately SIGKILL the STT worker process (if registered).
    pub fn kill_stt_now(&self) -> bool {
        let pid = self.stt.swap(0, Ordering::SeqCst);
        if pid == 0 {
            return false;
        }
        kill_os_pid(pid);
        true
    }

    /// SIGKILL any registered worker pids (hard quit / bounded shutdown).
    pub fn kill_all_now(&self) -> bool {
        let t = self.kill_tts_now();
        let s = self.kill_stt_now();
        t || s
    }

    /// Peek registered TTS pid (used by tests; available for diagnostics).
    #[cfg(test)]
    pub fn peek_tts_pid(&self) -> Option<u32> {
        match self.tts.load(Ordering::SeqCst) {
            0 => None,
            p => Some(p),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    #[test]
    fn job_shutdown_join_budget_is_bounded() {
        assert!(JOB_SHUTDOWN_JOIN_BUDGET > Duration::from_millis(0));
        assert!(JOB_SHUTDOWN_JOIN_BUDGET <= Duration::from_secs(5));
        assert!(!job_shutdown_join_exceeded(
            Duration::from_millis(100),
            JOB_SHUTDOWN_JOIN_BUDGET
        ));
        assert!(job_shutdown_join_exceeded(
            JOB_SHUTDOWN_JOIN_BUDGET,
            JOB_SHUTDOWN_JOIN_BUDGET
        ));
        assert!(job_shutdown_join_exceeded(
            Duration::from_secs(10),
            JOB_SHUTDOWN_JOIN_BUDGET
        ));
    }

    #[test]
    fn live_worker_pids_kill_tts_interrupts_within_1s() {
        let mut child = Command::new("bash")
            .args(["-c", "sleep 120"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleeper");
        let pid = child.id();
        let reg = LiveWorkerPids::default();
        reg.set_tts(Some(pid));
        assert_eq!(reg.peek_tts_pid(), Some(pid));

        let t0 = Instant::now();
        assert!(reg.kill_tts_now(), "must attempt kill");
        assert!(reg.peek_tts_pid().is_none());

        let status = child.wait().expect("wait child");
        let elapsed = t0.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "kill must finish within 1s, took {elapsed:?}; status={status:?}"
        );
        let _ = status;
        assert!(!reg.kill_tts_now());
    }

    #[test]
    fn kill_all_now_clears_both_roles() {
        let mut tts = Command::new("bash")
            .args(["-c", "sleep 120"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn tts sleeper");
        let mut stt = Command::new("bash")
            .args(["-c", "sleep 120"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn stt sleeper");
        let reg = LiveWorkerPids::default();
        reg.set_tts(Some(tts.id()));
        reg.set_stt(Some(stt.id()));
        assert!(reg.kill_all_now());
        let _ = tts.wait();
        let _ = stt.wait();
        assert!(!reg.kill_all_now());
    }
}
