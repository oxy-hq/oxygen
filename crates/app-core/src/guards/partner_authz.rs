//! Shim: partner authorization now lives in the [`oxy_server_authz`] crate.
//!
//! Re-exported here, and by `oxy-app` at its original
//! `server::api::middlewares::partner_authz` path, so call sites keep using
//! `…::partner_authz::{PartnerCapability, PartnerScope, resolve_scope, …}` unchanged.

pub use oxy_server_authz::partner_authz::*;
