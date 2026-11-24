//! The `oxy-api-previews` integration group — one binary, like every crate's
//! `tests/integration/` (see the repo `CLAUDE.md`). Add a case as a `mod` here
//! rather than a new top-level `tests/*.rs`.
//!
//! These modules moved out of `oxy-app`'s `platform` group with the previews
//! surface (2026-11-14): `oxy-app` cannot depend on this crate, so its test
//! binaries can no longer name the handlers, the executors or the run sweep.
//!
//! They reuse `oxy-app`'s per-test harness through the `#[path]` below — the
//! same file the `oxy-app` groups include — so every test gets its own database
//! cloned from a per-run template. That puts this binary in nextest's
//! `db-per-test` group (`.config/nextest.toml`), as the `platform` binary was.
//! `previews_from_git` also borrows the commit-compile world `oxy-app`'s
//! `compile_from_git` and `compile_request` tests build (a GitHub commit
//! archive, a PAT-connected workspace, a worker's drive loop), included the
//! same way. `admin_staff_scope` includes the staff-scope fixture of
//! `oxy-app`'s `platform` group by path, for the compile console's cases
//! (admin split, PR 6).

#[path = "../../../test-utils/harness/mod.rs"]
mod common;

// Shared with `oxy-app`'s tests, which use more of each than
// `previews_from_git` does.
#[path = "../../../test-utils/harness/compile_from_git_support.rs"]
#[allow(dead_code)]
mod compile_from_git_support;
#[path = "../../../test-utils/harness/compile_request_fixture.rs"]
#[allow(dead_code)]
mod compile_request_fixture;

/// Under the names the included fixture and `previews_from_git` use.
mod compile_from_git {
    pub(crate) use crate::compile_from_git_support::*;
}
mod compile_request {
    pub(crate) use crate::compile_request_fixture as fixture;
}

mod admin_staff_scope;
mod preview_routes;
mod preview_runs;
mod preview_samples;
mod preview_sources;
mod preview_ttl;
mod previews_e2e;
mod previews_from_git;
mod workspace_previews_lifecycle;
mod workspace_previews_serving;
