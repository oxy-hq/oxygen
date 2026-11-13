//! `oxy_server_authz`, under the path the token code has always used for it
//! (`authz::{Action, Caller, PrincipalFacts, globals, …}`), plus [`caller_of`].
//!
//! The same shim `oxy-app` keeps at `server::authz`: the model and the fact
//! loading are `oxy-server-authz`'s, which every surface may depend on.

pub use oxy_server_authz::*;
pub use oxy_server_authz::{globals, loader};

/// The [`Caller`] behind a request's actor: its user and the key or token it
/// authenticated with. What every email-keyed authorization door takes, so an
/// API token's narrowing reaches it.
pub fn caller_of(actor: &oxy_app_core::audit::RequestActor) -> Caller {
    Caller::of(&actor.user, actor.credential.as_ref())
}
