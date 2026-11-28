//! The enforcer a [`Caller`](crate::Caller) carries.
//!
//! A caller is built at the request boundary by authentication's projection
//! (`oxy_server_authz::Caller::of` and its siblings, or
//! [`Caller::from_parts`](crate::Caller::from_parts)), and every constructor
//! takes the enforcer. So any function that holds a caller can enforce through
//! [`Caller::authz`](crate::Caller::authz), and a caller with no enforcer cannot
//! be built: a missing wire is a compile error, never a runtime lookup (C3a,
//! 2026-11-28, which replaced a process-wide install).

use std::fmt;
use std::sync::Arc;

use crate::enforcer::AuthzEnforcer;

/// The handle. Not part of who a caller is: two callers built from the same
/// parts are equal whichever enforcer each carries, as [`GrantMemo`](crate::GrantMemo)
/// is not part of it either. There is one production implementation
/// (`oxy_server_authz::ServerAuthz`), so the handles of two real callers are the
/// same enforcer anyway; a test stand-in must not make equal callers differ.
#[derive(Clone)]
pub(crate) struct EnforcerHandle(pub(crate) Arc<dyn AuthzEnforcer>);

impl PartialEq for EnforcerHandle {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for EnforcerHandle {}

impl fmt::Debug for EnforcerHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnforcerHandle")
    }
}
