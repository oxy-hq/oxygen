//! `oxy-api-identity`'s integration tests — one binary, per the workspace rule
//! (a new top-level `tests/*.rs` is another full link). Add a case as a `mod`.
//!
//! The first cases moved here from `oxy-app` with the staff console's user
//! directory (admin split, PR 5). The DB-backed ones keep using `oxy-app`'s
//! harness, included by path rather than copied (it owns the per-run template
//! database and the stray-database sweep), so `.config/nextest.toml` puts this
//! binary in `db-per-test`. The exception is `admin_membership_audit`, which
//! calls `establish_connection()` on the shared database: it is pinned into
//! `serial-db`, and `oxy-app`'s `authz::shared_db_registry` fails the build if
//! that pin drifts.

#[path = "../../../test-utils/harness/mod.rs"]
mod common;

mod admin_membership_audit;
mod admin_staff_scope;
mod admin_users_mount;
