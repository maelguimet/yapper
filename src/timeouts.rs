//! Bounded waits for worker load/synth/transcribe (shared by jobs + workers).

use std::time::Duration;

pub fn stt_load_small() -> Duration {
    Duration::from_secs(180)
}
pub fn stt_load_medium() -> Duration {
    Duration::from_secs(300)
}
pub fn tts_load() -> Duration {
    Duration::from_secs(300)
}
pub fn stt_transcribe() -> Duration {
    Duration::from_secs(120)
}
/// Per-chunk synth: floor 45s, scale with text length, cap 180s.
pub fn tts_synth_chunk(text_chars: usize) -> Duration {
    let scaled = 45u64.saturating_add((text_chars as u64) / 4);
    Duration::from_secs(scaled.min(180).max(45))
}
pub fn unload() -> Duration {
    Duration::from_secs(30)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_timeout_scales_and_caps() {
        assert_eq!(tts_synth_chunk(0).as_secs(), 45);
        assert_eq!(tts_synth_chunk(100).as_secs(), 70);
        assert_eq!(tts_synth_chunk(10_000).as_secs(), 180);
    }
}
