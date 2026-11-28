//! The staff console's org and workspace sections (`/api/admin/orgs*`,
//! `/api/admin/workspaces*`), mounted through the admin seam — see
//! [`crate::admin_sections`]. Moved from `oxy-app`'s `admin::{orgs_admin,
//! workspaces_admin}`, and `org_subdomains` (an org's bare subdomain, the
//! staff half of `crate::org_subdomain`) from `oxy-app`'s
//! `admin::org_subdomains` (admin split, PR 1). The partner registry
//! (`/api/admin/partners*`, staff granting and governing partnerships — the
//! staff half of `crate::partner_console`) from `oxy-app`'s `admin::partners`
//! (admin split, PR 2), its own section behind `PlatformPartners`.
//!
//! The per-org data-plane provisioning fences (`/admin/oltp*`,
//! `/admin/orgs/{org_id}/oltp*`, `/admin/airhouse`,
//! `/admin/workspaces/{workspace_id}/airhouse/*`) were here from admin split
//! PR 4 until crate-depth B2 moved them, with the handlers they fence, to
//! `oxy_api_data_plane::admin`. `oxy-server` mounts those sections itself:
//! this crate may not depend on that sibling surface.

pub mod org_subdomains;
pub mod orgs;
pub mod partners;
pub mod workspace_transfer;
pub mod workspaces;
