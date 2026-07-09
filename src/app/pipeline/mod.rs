//! YapperApp pipeline: async load/speak/transcribe via JobHub; transport pump.
//!
//! Split by concern (mechanical; no behavior change):
//! - [`models`] — STT/TTS load/unload and file transcribe entry
//! - [`speak`] — cancel/cleanup/speak start and synth pump
//! - [`jobs`] — job message drain/handlers
//! - [`playback`] — transport drain, status, finalize
//! - [`record`] — mic capture, PTT, read-aloud

mod jobs;
mod models;
mod playback;
mod record;
mod speak;
