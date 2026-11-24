//! The `oxy-api-airway` integration group — one binary, like every
//! crate's `tests/integration/` (see the repo `CLAUDE.md`). Add a case as a
//! `mod` here rather than a new top-level `tests/*.rs`.
//!
//! It reuses `oxy-app`'s per-test harness through the `#[path]` below, as
//! `oxy-api-automations`' group does, so every test that opens a database gets
//! its own, cloned from a per-run template. That puts this binary in nextest's
//! `db-per-test` group (`.config/nextest.toml`).

#[path = "../../../test-utils/harness/mod.rs"]
mod common;

mod admin_mount;
mod leases;
