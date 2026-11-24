//! Authorization boundary + differential tests.
//!
//! One binary, not one per file. Each integration-test target statically links
//! the whole dep graph (DuckDB, DataFusion, Arrow, the AWS SDK), so a target is
//! a multi-hundred-MB link before a single assertion runs. Grouping by domain
//! keeps that cost proportional to the number of domains rather than the number
//! of files — add a case as a `mod` here, not as a new `tests/*.rs`.
//!
//! This binary is MIXED, and the split is load-bearing. Most of it is
//! in-process — source scanning, `oxy-authz` against hand-built facts — and runs
//! fully parallel. But `authz_loader_differential` and `audit_append_only` call
//! `establish_connection()` against the raw shared `OXY_DATABASE_URL` with no
//! per-test database, so `.config/nextest.toml` pins those two (and
//! `oxy-api-slack`'s six, `oxy-api-identity`'s `admin_membership_audit` and
//! `oxy-api-tenancy`'s `org_invitations`) into `serial-db`. They skip when the
//! var is unset, which is why a laptop run looks clean and only CI would show
//! the race. `shared_db_registry` fails the build if that list drifts.
//!
//! The API-token suite (`token_auth`) moved to `oxy-server`'s integration tests:
//! it drives the served flat tree, and `/orgs` is now `oxy-api-tenancy`'s.

#[path = "../../../test-utils/harness/mod.rs"]
mod common;

// `admin_membership_audit` moved to `oxy-api-identity`'s integration group
// with the user directory's handlers (admin split, PR 5).
mod app_scope_boundary;
mod audit_append_only;
mod authz_boundaries;
mod authz_loader_differential;
mod document_write_guards;
// `org_invitations` moved to `oxy-api-tenancy`'s integration group, beside
// the invitation handlers it pins (decompose T-small, 2026-11-24).
mod shared_db_registry;
// `thread_role_guards` moved to `oxy-api-threads`' integration group with
// the thread handlers it drives (2026-11-16).
