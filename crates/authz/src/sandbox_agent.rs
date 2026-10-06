//! The sandbox agent token (`oxy_sbx_`): the narrowing fact, and what it
//! covers (sandbox agent credential design §3.2).
//!
//! A sandbox agent token is its minter — a staff member — seen through the
//! narrowest credential there is. It adds **no** [`Action`] and no ring: for
//! every caller without a token a new action would decide exactly as
//! [`Action::AppNonProduction`] does, and two names for one decision is how
//! drift restarts. The difference is a property of the *credential*, so it is
//! a fact on [`crate::TokenReach`] that subtracts, checked first in
//! [`crate::allows`].
//!
//! [`covers`] states the whole of it: four actions, and for the two tenant
//! ones an **environment** the call site must name ([`EnvFacet`]). A decision
//! that names no environment is not covered, so a call site that has not been
//! taught to say which environment it is about fails closed for this token and
//! decides for everyone else exactly as it did.

use uuid::Uuid;

use crate::{Action, Resource, ResourceKind};

/// Which environment of a custom app a decision is about.
///
/// Read by nothing but [`covers`], so it can only subtract: a session, a
/// legacy key and every other token decide the same with any facet as with
/// none.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EnvFacet {
    Production,
    Staging,
    /// A `dev-*` sandbox that exists, and the sandbox agent token that created
    /// it (`app_environments.created_by_token_id`) — `None` for one a person
    /// created.
    Sandbox {
        created_by_token: Option<Uuid>,
    },
    /// No one sandbox yet: creating one, or listing an app's.
    NewSandbox,
}

/// One app a sandbox agent token is granted, with the org that owns it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SandboxApp {
    pub app_id: Uuid,
    pub org_id: Uuid,
}

/// What a sandbox agent token reaches: the sandboxes it created itself, of the
/// apps its live grants name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxAgentReach {
    /// `api_tokens.id` — what "its own sandbox" is compared against.
    pub token_id: Uuid,
    pub apps: Vec<SandboxApp>,
}

impl SandboxAgentReach {
    /// Is `resource` one of the granted apps? The org is part of the match, so
    /// a resource built with the right app id under another org is not.
    pub fn grants_app(&self, resource: &Resource) -> bool {
        resource.kind == ResourceKind::App
            && self
                .apps
                .iter()
                .any(|app| app.app_id == resource.id && app.org_id == resource.org_id)
    }

    /// Is `resource` a sandbox this token created, of an app it is granted?
    pub fn owns_sandbox(&self, resource: &Resource) -> bool {
        let own = matches!(
            resource.environment,
            Some(EnvFacet::Sandbox { created_by_token: Some(token) }) if token == self.token_id
        );
        own && self.grants_app(resource)
    }
}

/// Does a sandbox agent token cover this decision at all? `false` ends the
/// decision; `true` hands it to the ring, which still asks whether the
/// **minter** may do it — so this can only subtract.
///
/// Exhaustive on purpose, with no wildcard: a new [`Action`] does not compile
/// until its author writes its arm here, and the arm to write is `false`.
#[deny(clippy::wildcard_enum_match_arm)]
pub(crate) fn covers(reach: &SandboxAgentReach, action: Action, resource: &Resource) -> bool {
    match action {
        // The custom-apps console's two doors. Both decide on the platform
        // singleton, whose nil org cannot narrow by app: the route fence
        // confines which console routes lie behind them.
        Action::PlatformOps | Action::PlatformApps => resource.kind == ResourceKind::Platform,
        // Opening a non-production environment: a sandbox of its own, or the
        // act of creating or listing one. Production, staging, someone else's
        // sandbox and a decision that names no environment are not covered.
        Action::AppNonProduction => {
            reach.owns_sandbox(resource)
                || (resource.environment == Some(EnvFacet::NewSandbox)
                    && reach.grants_app(resource))
        }
        // The app's admin surface — its logs, and `ctx.user.appRole` on a
        // function call — in a sandbox of its own only.
        Action::AppAdmin => reach.owns_sandbox(resource),
        Action::OrgRead
        | Action::ManageLocations
        | Action::ManageOrgRoles
        | Action::ManageDocuments
        | Action::ManageAssignments
        | Action::ServiceAccountManage
        | Action::TokenInventoryView
        | Action::TokenGrantRevoke
        | Action::TokenPolicyManage
        | Action::MemberInvite
        | Action::MemberSetRole
        | Action::MemberRemove
        | Action::OrgBilling
        | Action::OrgOwnerManage
        | Action::OrgReadStrict
        | Action::WorkspaceManage
        | Action::WorkspaceEdit
        | Action::AppAccess
        | Action::WorkspaceDataAccess
        | Action::AppAccessManage
        | Action::WorkspaceRename
        | Action::NamespaceDelete
        | Action::WorkspaceOxyAccess
        | Action::WorkspacePreview
        | Action::PartnerManageMembers
        | Action::PartnerManageApps
        | Action::PartnerDevelopApps
        | Action::PartnerViewAudit
        | Action::PartnerManageBilling
        | Action::PartnerManageSecrets
        | Action::PartnerCreateOrgs
        | Action::PartnerManageOrgSettings
        | Action::PartnerManageOltp
        | Action::PlatformOltp
        | Action::PlatformAirhouse
        | Action::PlatformExplorer
        | Action::PlatformAudit
        | Action::PlatformOrgs
        | Action::PlatformOrgCreate
        | Action::PlatformUsers
        | Action::PlatformPartners
        | Action::PlatformOperate
        | Action::PlatformGrants
        | Action::PlatformOwnerOnly => false,
    }
}

#[cfg(test)]
#[path = "sandbox_agent_tests.rs"]
mod tests;
