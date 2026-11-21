//! The `oxy-api-metric-monitoring` integration group — one binary, like every
//! crate's `tests/integration/` (see the repo `CLAUDE.md`). Add a case as a
//! `mod` here rather than a new top-level `tests/*.rs`.
//!
//! `anomaly_bulk_status` moved out of `oxy-app`'s `platform` group with the
//! anomaly inbox (2026-11-15): `oxy-app` cannot depend on this crate, so its test
//! binaries can no longer name `apply_status_bulk`. `anomaly_notify` followed
//! the insights delivery's executor out of the same group (2026-11-21).
//!
//! It reuses `oxy-app`'s per-test harness through the `#[path]` below — the same
//! file the `oxy-app` groups include — so every test gets its own database
//! cloned from a per-run template. That puts this binary in nextest's
//! `db-per-test` group (`.config/nextest.toml`), as the `platform` binary was.

#[path = "../../../app/tests/common/mod.rs"]
mod common;

mod anomaly_bulk_status;
mod anomaly_notify;
