//! The staff console's custom-app handlers: `/api/customer-apps`, and the CI
//! publish tokens, `/api/admin/app-publish-tokens` (`app_publish_tokens`, out
//! of `oxy-app`'s `admin::app_publish_tokens` in the admin split, PR 2),
//! mounted through the admin seam — see [`crate::admin_sections`].

pub mod app_publish_tokens;
pub mod apps;
pub mod oxy_access;
