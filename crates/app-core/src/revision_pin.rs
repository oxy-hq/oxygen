//! Moved to `oxy::config::servability` (2026-11-27, crate depth stage C, PR D),
//! beside `Config` and compiled storage, so `oxy-previews` can apply the pin
//! rule without depending on `oxy-app-core`. Re-exported here at its old path
//! for one release.

pub use oxy::config::servability::{PinRefusal, check_pinnable, check_revision};
