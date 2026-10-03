//! The staff console's org and workspace sections (`/api/admin/orgs*`,
//! `/api/admin/workspaces*`), mounted through the admin seam — see
//! [`crate::admin_sections`]. Moved from `oxy-app`'s `admin::{orgs_admin,
//! workspaces_admin}`.

pub mod orgs;
pub mod workspaces;
