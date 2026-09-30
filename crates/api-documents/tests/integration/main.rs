//! The `oxy-api-documents` integration group — one binary, like every crate's
//! `tests/integration/` (see the repo `CLAUDE.md`). Add a case as a `mod` here
//! rather than a new top-level `tests/*.rs`.
//!
//! These moved out of `oxy-app`'s `platform` group with the code they test:
//! `oxy-app` cannot depend on this crate, so its test binaries can no longer
//! name `oxy_api_documents::…`.
//!
//! The database-backed modules reuse `oxy-app`'s per-test harness through the
//! `#[path]` below — the same file the `oxy-app` groups include — so every test
//! gets its own database cloned from a per-run template. That puts this binary
//! in nextest's `db-per-test` group (`.config/nextest.toml`), exactly as the
//! `platform` binary was. None of it touches the shared `OXY_DATABASE_URL`.
//!
//! `route_roles` installs a process-wide declaration registry; nextest's
//! process-per-test model keeps that from leaking between tests.

#[path = "../../../app/tests/common/mod.rs"]
mod common;

mod document_ask;
mod document_ask_sessions;
mod document_compliance;
mod document_search;
mod document_shelf;
mod document_storage;
mod documents;
mod route_roles;
