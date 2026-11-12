//! The `oxy-api-world-model` integration group — one binary, like every crate's
//! `tests/integration/` (see the repo `CLAUDE.md`). Add a case as a `mod` here
//! rather than a new top-level `tests/*.rs`.
//!
//! `world_model_cross_pod` moved out of `oxy-app`'s `platform` group with the
//! feed's read half: `oxy-app` cannot depend on this crate, so its test binaries
//! can no longer name the tailer or the bus.
//!
//! It reuses `oxy-app`'s per-test harness through the `#[path]` below — the same
//! file the `oxy-app` groups include — so every test gets its own database
//! cloned from a per-run template. That puts this binary in nextest's
//! `db-per-test` group (`.config/nextest.toml`), as the `platform` binary was.

#[path = "../../../app/tests/common/mod.rs"]
mod common;

mod world_model_cross_pod;
