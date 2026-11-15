//! The `oxy-api-automations` integration group — one binary, like every
//! crate's `tests/integration/` (see the repo `CLAUDE.md`). Add a case as a
//! `mod` here rather than a new top-level `tests/*.rs`.
//!
//! It reuses `oxy-app`'s per-test harness through the `#[path]` below — the same
//! file the `oxy-app` groups include — so every test gets its own database
//! cloned from a per-run template. That puts this binary in nextest's
//! `db-per-test` group (`.config/nextest.toml`).

#[path = "../../../app/tests/common/mod.rs"]
mod common;

// Rehomed from `oxy-api-custom-apps`' `env_schedules_tenant` with the schedule
// handlers it drives: a surface may not depend on another, not even for tests.
mod tenant_schedules;
