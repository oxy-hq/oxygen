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
//! fully parallel. But `authz_loader_differential`, `org_invitations` and
//! `admin_membership_audit` call `establish_connection()` against the raw shared
//! `OXY_DATABASE_URL` with no per-test database, so `.config/nextest.toml` pins
//! those three (and six in `tests/slack/`) into `serial-db`. They skip when the
//! var is unset, which is why a laptop run looks clean and only CI would show
//! the race. `shared_db_registry` fails the build if that list drifts.

mod admin_membership_audit;
mod app_scope_boundary;
mod audit_append_only;
mod authz_boundaries;
mod authz_loader_differential;
mod document_write_guards;
mod frontline_device_guards;
mod org_invitations;
mod shared_db_registry;
mod thread_role_guards;
