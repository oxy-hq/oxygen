//! The staff console's row-level authorization: the half a capability gate
//! cannot answer, because `platform_cap_guard` decides on a nil-org
//! `Resource::platform()` and never sees the row.
//!
//! - [`scope`]: does a bounded grant reach this org (the fences, and a
//!   listing's scope)? The one reader of a grant's scope on the console.
//! - [`org_filter`]: the registry listings' older, lenient org filter.
//! - [`delegation`]: may the caller write this grant? The transport half of
//!   `oxy_authz::may_delegate`.
//! - [`stored_grant`]: a grant as stored, and the delegation bound over it.
//!
//! Down from `oxy-app`'s `server::api::admin` in the admin split's PR 3, so the
//! sections that use them can leave `oxy-app` for their contexts' surfaces.

pub mod delegation;
pub mod org_filter;
pub mod scope;
pub mod stored_grant;
