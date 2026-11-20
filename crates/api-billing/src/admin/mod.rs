//! The staff console's billing section (`/admin/orgs`, `/admin/billing/*`,
//! `/admin/orgs/{org_id}/billing/*`), out of `oxy-app`'s `api::admin::billing`
//! in the admin split's PR 9. Mounted as an `AdminSection` naming
//! `Action::PlatformOwnerOnly`; see [`admin_sections`](crate::admin_sections).

pub mod billing;
