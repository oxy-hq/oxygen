//! The task fleet's operator console (`/admin/internal-jobs*`), the
//! platform audit log (`/admin/audit`) and the impersonation log
//! (`/assume/history`) under a platform grant bounded to named orgs: it sees
//! and touches only the tasks of those orgs' workspaces, and reads only those
//! orgs' audit trail and the sessions into them.
//!
//! Moved from `oxy-app`'s `tests/platform/admin_staff_scope/` with the
//! handlers (admin split, PR 7, the audit and the assume moves); the header there states
//! the rule these cases
//! pin. The fixture (two tenants, an org-less workspace, a bounded and an
//! unbounded `global_admin`, the Global Owner) is that suite's, included by
//! path so the copies cannot drift, as `oxy-api-identity` and
//! `oxy-api-previews` include it. Each case owns its database
//! ([`crate::common::test_db_with`]).

#[path = "../../../test-utils/harness/admin_staff_scope_fixture.rs"]
#[allow(dead_code)]
mod fixture;

mod assume;
mod audit;
mod internal_jobs;
