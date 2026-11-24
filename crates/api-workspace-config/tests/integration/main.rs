//! The `oxy-api-workspace-config` integration group — one binary, like every
//! crate's `tests/integration/` (see the repo `CLAUDE.md`). Add a case as a
//! `mod` here rather than a new top-level `tests/*.rs`.
//!
//! It reuses `oxy-app`'s per-test harness through the `#[path]` below — the same
//! file the `oxy-app` groups include — so every test gets its own database
//! cloned from a per-run template. That puts this binary in nextest's
//! `db-per-test` group (`.config/nextest.toml`).

// `seed_demo_with`'s future is deep enough to overflow rustc's default query
// depth in this binary, as it does in the crates whose libs carry the same line.
#![recursion_limit = "256"]

#[path = "../../../test-utils/harness/mod.rs"]
mod common;

// The tenant halves of `oxy-api-custom-apps`' `staging_secrets_admin` and
// `secrets_audit`, rehomed with the secrets handlers they drive: a surface may
// not depend on another, not even for tests.
mod tenant_secrets;
