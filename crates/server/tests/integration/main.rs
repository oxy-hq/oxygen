//! `oxy-server`'s integration tests — one binary, per the workspace rule (a new
//! top-level `tests/*.rs` is another full link). Add a case as a `mod`.
//!
//! They live in the composition root because they drive the API **as served**:
//! `oxy-app`'s routes together with the extracted surface crates (`/orgs` and the
//! rest of tenancy is `oxy-api-tenancy`'s), behind the one `/api` auth stack.
//! Only this crate depends on both.
//!
//! `token_auth` owns a database cloned from the per-run template, using
//! `oxy-app`'s harness included by path rather than copied (it owns the
//! template database and the stray-database sweep), so `.config/nextest.toml`
//! puts this binary in `db-per-test`.

#[path = "../../../app/tests/common/mod.rs"]
mod common;

mod token_auth;
