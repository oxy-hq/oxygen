//! Authorization boundary + differential tests.
//!
//! One binary, not one per file. Each integration-test target statically links
//! the whole dep graph (DuckDB, DataFusion, Arrow, the AWS SDK), so a target is
//! a multi-hundred-MB link before a single assertion runs. Grouping by domain
//! keeps that cost proportional to the number of domains rather than the number
//! of files — add a case as a `mod` here, not as a new `tests/*.rs`.
//!
//! The in-process half of this binary (the source scans) moved to `oxy-guards`
//! in decompose GRD (2026-11-24), and the two shared-database modules left,
//! `authz_loader_differential` and `audit_append_only`, to
//! `oxy-workspace-runtime`'s integration group in decompose R-d (2026-11-25),
//! with their `serial-db` pin. This binary has no tests left; decompose DEL
//! removes it.
//!
//! The API-token suite (`token_auth`) moved to `oxy-server`'s integration tests:
//! it drives the served flat tree, and `/orgs` is now `oxy-api-tenancy`'s.

#[path = "../../../test-utils/harness/mod.rs"]
mod common;

// `admin_membership_audit` moved to `oxy-api-identity`'s integration group
// with the user directory's handlers (admin split, PR 5).
// `app_scope_boundary`, `authz_boundaries`, `document_write_guards` and
// `shared_db_registry` (source scans, no database) moved to `oxy-guards`'
// integration group (decompose GRD, 2026-11-24).
// `audit_append_only` and `authz_loader_differential` moved to
// `oxy-workspace-runtime`'s integration group, and their `serial-db` pin with
// them (decompose R-d, 2026-11-25). Nothing is left here; decompose DEL
// removes this file.
// `org_invitations` moved to `oxy-api-tenancy`'s integration group, beside
// the invitation handlers it pins (decompose T-small, 2026-11-24).
// `thread_role_guards` moved to `oxy-api-threads`' integration group with
// the thread handlers it drives (2026-11-16).
