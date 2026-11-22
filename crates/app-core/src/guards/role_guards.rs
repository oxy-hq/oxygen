//! The typed role-guard extractors now live in the `oxy-server-authz` crate:
//! they are pure `FromRequestParts` over request extensions plus the authz
//! model, with no `AppState` coupling, so they belong with the model rather
//! than in a transport crate. The DB-loading middleware that *populates*
//! those extensions is `org_middleware` (beside this, in `guards::org_context`)
//! and `oxy-app`'s `workspace_middleware` / `local_context_middleware`.
//!
//! `oxy-app` re-exports this module at its original path, so existing
//! `middlewares::role_guards::{OrgOwner, WorkspaceAdmin, ...}` call sites —
//! including the `oxy_app::api::middlewares::role_guards::*` path used by
//! integration tests — keep resolving unchanged.
pub use oxy_server_authz::role_guards::*;
