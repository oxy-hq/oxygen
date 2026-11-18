//! The user directory (`/admin/users*`) under a platform grant bounded to
//! named orgs: it sees and touches only the people of those orgs.
//!
//! Moved from `oxy-app`'s `tests/platform/admin_staff_scope/` with the
//! handlers (admin split, PR 5); the header there states the rule these cases
//! pin. The fixture (two tenants, an org-less workspace, a bounded and an
//! unbounded `global_admin`, the Global Owner) is that suite's, included by
//! path so the two cannot drift. Each case owns its database
//! ([`crate::common::test_db_with`]).

#[path = "../../../app/tests/platform/admin_staff_scope/fixture.rs"]
#[allow(dead_code)]
mod fixture;

mod directories;
mod user_add_to_org;
mod user_detail;
mod user_directory;
mod user_status;
