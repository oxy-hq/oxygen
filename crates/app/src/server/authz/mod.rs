//! Shim: app-side authorization now lives in the [`oxy_server_authz`] crate.
//!
//! It was extracted verbatim so call sites keep using
//! `crate::server::authz::{Action, Resource, allows, enforce_guard, globals::…,
//! loader::…, …}` unchanged. The glob re-exports the crate root (the model via
//! `oxy_authz::*`, plus `request_facts` / `enforce_guard` / `enforce_for` / `cap_of`
//! / `partner_action` / `partner_allows` …); the explicit re-exports keep the
//! `globals::` and `loader::` module paths resolving. (`caller_of` had no caller
//! left here and was deleted in decompose SHM, 2026-11-23; `oxy-api-admin`'s
//! `assume` keeps its own copy.)

pub use oxy_server_authz::*;
pub use oxy_server_authz::{globals, loader};
