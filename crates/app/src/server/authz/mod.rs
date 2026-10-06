//! Shim: app-side authorization now lives in the [`oxy_server_authz`] crate.
//!
//! It was extracted verbatim so call sites keep using
//! `crate::server::authz::{Action, Resource, allows, enforce_guard, globals::…,
//! loader::…, …}` unchanged. The glob re-exports the crate root (the model via
//! `oxy_authz::*`, plus `request_facts` / `enforce_guard` / `enforce_for` / `cap_of`
//! / `partner_action` / `partner_allows` …); the explicit re-exports keep the
//! `globals::` and `loader::` module paths resolving.

pub use oxy_server_authz::*;
pub use oxy_server_authz::{globals, loader};

/// The [`Caller`] behind a request's actor: its user and the key or token it
/// authenticated with. What every email-keyed authorization door takes, so an
/// API token's narrowing reaches it.
pub fn caller_of(actor: &oxy_app_core::audit::RequestActor) -> Caller {
    Caller::of(&actor.user, actor.credential.as_ref())
}
