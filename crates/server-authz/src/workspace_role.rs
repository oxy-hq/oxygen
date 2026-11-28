//! The caller's resolved workspace role, carried as a request extension.
//!
//! The type and its extractors moved to the port crate in C3a, so the middleware
//! that writes it can name it without this crate; this keeps
//! `oxy_server_authz::workspace_role::EffectiveWorkspaceRole` resolving for the
//! guards here and the surfaces that read it.

pub use contracts_authz::workspace_role::*;
