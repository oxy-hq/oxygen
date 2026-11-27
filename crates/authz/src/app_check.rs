//! The promote checks' rings, stated here and named by [`crate::allows`]'s one
//! match: the platform checker on one environment of an app
//! (`Ring::StagingCheck`, `Ring::ProductionCheck`), and who may promote an
//! app's build past its checks (`Ring::PromoteOverride`). Beside them, what
//! the rest of the model reads about the checker's standing, and the one role
//! the grant table never holds.

use crate::{Cap, EnvFacet, PlatformRole, PrincipalFacts, Resource, ResourceKind, RoleCeiling};

/// `Ring::StagingCheck` and `Ring::ProductionCheck`: Oxy staff holding `cap`
/// over the app's org, on the app's `environment` and nowhere else. It records
/// the checks' verdicts as well as reading, so a token carries it only at an
/// admin ceiling (`ceiling`, the token's for this resource).
pub(crate) fn checker_on(
    facts: &PrincipalFacts,
    resource: &Resource,
    cap: Cap,
    environment: EnvFacet,
    ceiling: RoleCeiling,
) -> bool {
    resource.kind == ResourceKind::App
        && resource.environment == Some(environment)
        && ceiling >= RoleCeiling::Admin
        && facts.platform_grants(cap, resource.org_id)
}

/// `Ring::PromoteOverride`: on an app, an officer of its org (`officer_here`)
/// or staff standing in as one with `ManageApps` (`staff_override`).
pub(crate) fn promote_override(
    resource: &Resource,
    officer_here: bool,
    staff_override: bool,
) -> bool {
    resource.kind == ResourceKind::App && (officer_here || staff_override)
}

impl PrincipalFacts {
    /// Whether the only staff standing held is the platform checker's
    /// ([`Cap::CheckApps`] and nothing else): staff for [`crate::Action::AppCheck`]
    /// and [`crate::Action::AppCheckProduction`],
    /// and not staff at the console door (`Ring::PlatformAny`), which it has
    /// no section behind. A standing with no capability at all is not the
    /// checker's, and is left as the door always read it.
    pub fn checks_apps_only(&self) -> bool {
        !self.is_global_owner
            && self
                .platform
                .as_ref()
                .is_some_and(|p| !p.caps.is_empty() && p.caps.iter().all(|c| *c == Cap::CheckApps))
    }

    /// Staff at the console door (`Ring::PlatformAny`): staff, and not only
    /// the platform checker ([`Self::checks_apps_only`]).
    pub(crate) fn is_console_staff(&self) -> bool {
        self.is_staff() && !self.checks_apps_only()
    }
}

impl PlatformRole {
    /// Whether the platform-grant table may hold this role for a person. The
    /// checker is a service's standing, configured as code; a grant row naming it
    /// is refused at the write.
    pub fn is_grantable(self) -> bool {
        !matches!(self, PlatformRole::AppChecker)
    }
}
