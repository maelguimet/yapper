//! X11 helper tests (IsolatedX / pure plan). Included as `x11util::tests` via
//! `#[path]` so production `x11util.rs` stays under the hard line cap.
//!
//! Split by concern: [`support`] (Xvfb harness) + case modules.

mod support;
mod cases_plan;
mod cases_selection;
mod cases_insert;

// Private re-export so case modules can `use super::{IsolatedX, ...}`.
// (Child modules may access private parent names; pub(super) would not.)
#[allow(unused_imports)]
use support::*;
