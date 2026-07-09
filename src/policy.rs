//! Dual-model LRU load policy: both if they fit; else keep most recently used.

use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Stt,
    Tts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSlot {
    pub loaded: bool,
    pub model_id: Option<String>,
    pub last_used: Option<Instant>,
}

impl Default for ModelSlot {
    fn default() -> Self {
        Self {
            loaded: false,
            model_id: None,
            last_used: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct DualModelPolicy {
    pub stt: ModelSlot,
    pub tts: ModelSlot,
}

impl DualModelPolicy {
    pub fn mark_loaded(&mut self, role: Role, model_id: &str) {
        let slot = self.slot_mut(role);
        slot.loaded = true;
        slot.model_id = Some(model_id.to_string());
        slot.last_used = Some(Instant::now());
    }

    pub fn mark_unloaded(&mut self, role: Role) {
        let slot = self.slot_mut(role);
        slot.loaded = false;
        slot.model_id = None;
        // keep last_used for history? clear for simplicity
        slot.last_used = None;
    }

    pub fn touch(&mut self, role: Role) {
        let slot = self.slot_mut(role);
        if slot.loaded {
            slot.last_used = Some(Instant::now());
        }
    }

    /// Which peer role should be unloaded before retrying a failed load, if any.
    pub fn peer_to_unload_on_pressure(&self, loading: Role) -> Option<Role> {
        let peer = match loading {
            Role::Stt => Role::Tts,
            Role::Tts => Role::Stt,
        };
        if self.slot(peer).loaded {
            Some(peer)
        } else {
            None
        }
    }

    /// Sticky warm default: never unload merely because a job finished.
    pub fn should_unload_after_job(&self) -> bool {
        crate::textprep::should_unload_after_successful_job()
    }

    /// If both loaded and we need to free one for space: unload least recently used.
    pub fn lru_to_unload(&self) -> Option<Role> {
        match (self.stt.loaded, self.tts.loaded) {
            (true, true) => match (self.stt.last_used, self.tts.last_used) {
                (Some(a), Some(b)) => {
                    if a <= b {
                        Some(Role::Stt)
                    } else {
                        Some(Role::Tts)
                    }
                }
                (None, Some(_)) => Some(Role::Stt),
                (Some(_), None) => Some(Role::Tts),
                (None, None) => Some(Role::Stt),
            },
            (true, false) => Some(Role::Stt),
            (false, true) => Some(Role::Tts),
            (false, false) => None,
        }
    }

    /// Whether load can be a no-op (same model already loaded).
    pub fn already_loaded(&self, role: Role, model_id: &str) -> bool {
        let s = self.slot(role);
        s.loaded && s.model_id.as_deref() == Some(model_id)
    }

    fn slot(&self, role: Role) -> &ModelSlot {
        match role {
            Role::Stt => &self.stt,
            Role::Tts => &self.tts,
        }
    }

    fn slot_mut(&mut self, role: Role) -> &mut ModelSlot {
        match role {
            Role::Stt => &mut self.stt,
            Role::Tts => &mut self.tts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn peer_unload_when_other_loaded() {
        let mut p = DualModelPolicy::default();
        p.mark_loaded(Role::Tts, "chatterbox-multilingual");
        assert_eq!(p.peer_to_unload_on_pressure(Role::Stt), Some(Role::Tts));
        assert_eq!(p.peer_to_unload_on_pressure(Role::Tts), None);
    }

    #[test]
    fn lru_picks_older() {
        let mut p = DualModelPolicy::default();
        p.mark_loaded(Role::Stt, "small");
        thread::sleep(Duration::from_millis(5));
        p.mark_loaded(Role::Tts, "chatterbox-multilingual");
        // STT is older
        assert_eq!(p.lru_to_unload(), Some(Role::Stt));
        p.touch(Role::Stt);
        thread::sleep(Duration::from_millis(5));
        // TTS is older now
        assert_eq!(p.lru_to_unload(), Some(Role::Tts));
    }

    #[test]
    fn already_loaded_same_model() {
        let mut p = DualModelPolicy::default();
        p.mark_loaded(Role::Stt, "small");
        assert!(p.already_loaded(Role::Stt, "small"));
        assert!(!p.already_loaded(Role::Stt, "medium"));
    }

    #[test]
    fn unload_clears() {
        let mut p = DualModelPolicy::default();
        p.mark_loaded(Role::Stt, "small");
        p.mark_unloaded(Role::Stt);
        assert!(!p.stt.loaded);
        assert!(p.stt.model_id.is_none());
    }

    #[test]
    fn sticky_after_job_never_unloads() {
        let mut p = DualModelPolicy::default();
        p.mark_loaded(Role::Stt, "small");
        p.mark_loaded(Role::Tts, "chatterbox-multilingual");
        p.touch(Role::Stt);
        p.touch(Role::Tts);
        assert!(!p.should_unload_after_job());
        assert!(p.stt.loaded);
        assert!(p.tts.loaded);
    }
}
