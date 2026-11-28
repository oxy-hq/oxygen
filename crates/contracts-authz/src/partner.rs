//! What a partner's standing over one client gives a caller, as publish
//! authorization reads it.

/// A partner's reach over one client org, for one capability: the two facts
/// publish authorization (`oxy_custom_apps::publish_authz::resolve_actor`)
/// reads from the partner's scope.
///
/// This is the partner's **ceiling capability**, with no token-ceiling term:
/// exactly what publish has always read. It is not the partner ring
/// (`partner_allows`), which also applies the token's rules, and it must not
/// stand in for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartnerReach {
    /// The partner's managed set, as the credential covers it, holds the org.
    pub manages_target: bool,
    /// The partner's ceiling holds the capability asked about.
    pub holds_cap: bool,
}
