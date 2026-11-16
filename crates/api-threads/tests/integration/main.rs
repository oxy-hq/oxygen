//! The `oxy-api-threads` integration group — one binary, like every crate's
//! `tests/integration/` (see the repo `CLAUDE.md`). Add a case as a `mod`
//! here rather than a new top-level `tests/*.rs`.
//!
//! It reuses `oxy-app`'s per-test harness through the `#[path]` below — the same
//! file the `oxy-app` groups include — so a test that opens a database gets its
//! own, cloned from a per-run template. That puts this binary in nextest's
//! `db-per-test` group (`.config/nextest.toml`), all but `thread_role_guards`.

#[path = "../../../app/tests/common/mod.rs"]
mod common;

// Rehomed from `oxy-app`'s `platform` group with the staff console's Explorer
// (admin split, PR 1). Its own database per test.
mod admin_explorer_staff_scope;
// Rehomed from `oxy-app`'s `platform` group with the chat handlers it drives
// (2026-11-16). Its own database per test.
mod chat_org_standing;
// Rehomed from `oxy-app`'s `authz` group with the thread handlers it drives
// (2026-11-16). In-process: no database, fully parallel.
mod thread_role_guards;
