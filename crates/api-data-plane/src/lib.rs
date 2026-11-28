//! The data-plane HTTP surface, mounted by `oxy-server`.
//!
//! - [`airhouse`] — a member's own Airhouse warehouse: `/airhouse/version` and
//!   `/airhouse/me/*`. Was `airhouse::api` (the `rest` feature) until the
//!   router left the `airhouse` crate, which is a T1 client and so may not
//!   depend on `oxy-auth` (`internal-docs/crate-tiers.md`).
//!
//! The staff console over the same tenants (`/admin/airhouse*`) is not here:
//! it is fenced and declared by `oxy_api_tenancy::admin::airhouse`.

// Every surface crate declares it (`oxy-server`'s `recursion_limit_guard`):
// the release build lays out deeper futures than any PR job compiles.
#![recursion_limit = "256"]

pub mod airhouse;
