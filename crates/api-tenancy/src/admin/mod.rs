//! The staff console's org and workspace sections (`/api/admin/orgs*`,
//! `/api/admin/workspaces*`), mounted through the admin seam — see
//! [`crate::admin_sections`]. Moved from `oxy-app`'s `admin::{orgs_admin,
//! workspaces_admin}`, and `org_subdomains` (an org's bare subdomain, the
//! staff half of `crate::org_subdomain`) from `oxy-app`'s
//! `admin::org_subdomains` (admin split, PR 1). The partner registry
//! (`/api/admin/partners*`, staff granting and governing partnerships — the
//! staff half of `crate::partner_console`) from `oxy-app`'s `admin::partners`
//! (admin split, PR 2), its own section behind `PlatformPartners`.

pub mod org_subdomains;
pub mod orgs;
pub mod partners;
pub mod workspaces;
