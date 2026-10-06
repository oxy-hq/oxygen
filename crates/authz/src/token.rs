//! What an API token narrows its bearer to (API-tokens design §3.2, §4.4).
//!
//! A token never *adds* authority. It is the same principal seen through a
//! narrower credential: [`TokenReach`] says which orgs and workspaces the token
//! covers, how high it may act in each ([`RoleCeiling`]), and whether the
//! bearer's platform and partner standing ride along.
//!
//! Three places apply it, all of them subtracting:
//!
//! - [`PrincipalFacts::narrowed_by`] — the loader caps the facts at the source;
//! - [`crate::allows`] — per decision, because facts load once per request and
//!   the target varies: is the resource inside a grant, and at what ceiling;
//! - the app's role resolution (`resolve_effective_role`, `org_middleware`),
//!   which caps the role the shipped checks read.
//!
//! A browser session and a legacy key carry no [`TokenReach`] at all — `None`
//! narrows nothing, which is how every existing key keeps today's reach.

use uuid::Uuid;

use crate::{
    PartnerStanding, PlatformRole, PlatformStanding, PrincipalFacts, Resource, ResourceKind, Scope,
};

/// How high a grant lets a token act. Ordered: `Viewer < Member < Admin <
/// Owner`. `Owner` is no cap. The effective role is `min(live role, ceiling)`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RoleCeiling {
    Viewer,
    Member,
    Admin,
    Owner,
}

impl RoleCeiling {
    pub const ALL: [RoleCeiling; 4] = [
        RoleCeiling::Viewer,
        RoleCeiling::Member,
        RoleCeiling::Admin,
        RoleCeiling::Owner,
    ];

    /// Stable id. Stored in `api_token_grants.role_ceiling` and on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            RoleCeiling::Viewer => "viewer",
            RoleCeiling::Member => "member",
            RoleCeiling::Admin => "admin",
            RoleCeiling::Owner => "owner",
        }
    }

    /// `None` for an unknown id — the caller refuses the grant rather than
    /// guessing a ceiling for it.
    pub fn parse(s: &str) -> Option<RoleCeiling> {
        RoleCeiling::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

/// One workspace grant: an org, one workspace in it or all of them, and a
/// ceiling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenGrant {
    pub org_id: Uuid,
    /// `None` = every workspace in the org, including ones created later, and
    /// the org's own routes. `Some` reaches that workspace and nothing org-wide.
    pub workspace_id: Option<Uuid>,
    pub ceiling: RoleCeiling,
}

/// What a token reaches. Only ever narrower than its bearer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenReach {
    /// Everything the bearer reaches, now and later, at no ceiling. When false,
    /// only [`Self::grants`].
    pub all_access: bool,
    /// The bearer's Oxy-staff standing rides along. When false the token holds
    /// none, whatever the bearer holds.
    pub platform: bool,
    /// The bearer's partner standings ride along. When false the token holds
    /// none.
    pub partner: bool,
    pub grants: Vec<TokenGrant>,
    /// Orgs that ended this token's reach into them (the org token inventory's
    /// *revoke grant*). The token covers nothing there — whatever its grants
    /// say, and **even when it is all-access**, which has no grant row to
    /// revoke. Everything it reaches elsewhere stands.
    pub blocked_orgs: Vec<Uuid>,
}

impl TokenReach {
    /// A token that narrows nothing: all-access, with both standings.
    pub fn unrestricted() -> Self {
        Self {
            all_access: true,
            platform: true,
            partner: true,
            grants: Vec::new(),
            blocked_orgs: Vec::new(),
        }
    }

    /// Whether this token takes anything away from its bearer.
    pub fn narrows(&self) -> bool {
        !(self.all_access && self.platform && self.partner) || !self.blocked_orgs.is_empty()
    }

    /// Whether the token is confined to its grants: it is not all-access. The
    /// routes that name no org answer from raw membership and cannot honour a
    /// grant, so such a token gets only the ones that honour its reach
    /// themselves.
    ///
    /// An org's block does **not** make an all-access token bound — see
    /// [`Self::blocked_somewhere`].
    pub fn bound(&self) -> bool {
        !self.all_access
    }

    /// Whether an org ended an **all-access** token's reach into it. The block
    /// takes that org's data away and nothing else: the token keeps the routes
    /// that name no org, and each of them leaves the blocked orgs out
    /// ([`Self::touches_org`]).
    pub fn blocked_somewhere(&self) -> bool {
        self.all_access && !self.blocked_orgs.is_empty()
    }

    /// The highest ceiling in `org_id` among grants matching `keep`, or `Owner`
    /// for an all-access token. `None` = no grant matches, or the org ended
    /// this token's reach.
    fn ceiling_where(
        &self,
        org_id: Uuid,
        keep: impl Fn(&TokenGrant) -> bool,
    ) -> Option<RoleCeiling> {
        if self.blocked_orgs.contains(&org_id) {
            return None;
        }
        if self.all_access {
            return Some(RoleCeiling::Owner);
        }
        self.grants
            .iter()
            .filter(|g| g.org_id == org_id && keep(g))
            .map(|g| g.ceiling)
            .max()
    }

    /// The ceiling over the org **itself** (`/api/orgs/{org}/…`): an org-wide
    /// grant only. A grant on one workspace does not reach the org's routes.
    pub fn org_ceiling(&self, org_id: Uuid) -> Option<RoleCeiling> {
        self.ceiling_where(org_id, |g| g.workspace_id.is_none())
    }

    /// The ceiling over one workspace: an org-wide grant on its org, or a
    /// grant naming it.
    pub fn workspace_ceiling(&self, org_id: Uuid, workspace_id: Uuid) -> Option<RoleCeiling> {
        self.ceiling_where(org_id, |g| g.workspace_id.is_none_or(|w| w == workspace_id))
    }

    /// The highest ceiling of **any** grant in the org, org-wide or not. The
    /// reach over an org's children that name no workspace (an app, a thread).
    pub fn ceiling_in_org(&self, org_id: Uuid) -> Option<RoleCeiling> {
        self.ceiling_where(org_id, |_| true)
    }

    /// Does any grant touch `org_id` at all? What discovery lists.
    pub fn touches_org(&self, org_id: Uuid) -> bool {
        self.ceiling_in_org(org_id).is_some()
    }

    /// The ceiling this token holds over `resource`; `None` when the resource
    /// is outside every grant.
    ///
    /// The two singletons are gated by the standing flags, not by a grant: the
    /// platform by [`Self::platform`], and a partner itself (a decision with no
    /// client org — creating one, reading the partner-wide audit) by
    /// [`Self::partner`] on an all-access token, since it reaches past any list
    /// of orgs a token could name.
    pub fn ceiling_for(&self, resource: &Resource) -> Option<RoleCeiling> {
        match resource.kind {
            ResourceKind::Platform => self.platform.then_some(RoleCeiling::Owner),
            ResourceKind::Partner => {
                (self.partner && self.all_access).then_some(RoleCeiling::Owner)
            }
            ResourceKind::Org => self.org_ceiling(resource.org_id),
            ResourceKind::Workspace => self.workspace_ceiling(resource.org_id, resource.id),
            // An app that names the workspace it was published from is capped
            // there; one that does not, by the org's highest grant.
            ResourceKind::App => match resource.app_workspace {
                Some(workspace_id) => self.workspace_ceiling(resource.org_id, workspace_id),
                None => self.ceiling_in_org(resource.org_id),
            },
            ResourceKind::Thread | ResourceKind::Namespace => self.ceiling_in_org(resource.org_id),
        }
    }

    /// Is `resource` inside this token's reach?
    pub fn covers(&self, resource: &Resource) -> bool {
        self.ceiling_for(resource).is_some()
    }

    /// The distinct orgs the grants name, less the ones that blocked the token.
    fn grant_orgs(&self) -> Vec<Uuid> {
        let mut orgs: Vec<Uuid> = Vec::new();
        for g in &self.grants {
            if !orgs.contains(&g.org_id) && !self.blocked_orgs.contains(&g.org_id) {
                orgs.push(g.org_id);
            }
        }
        orgs
    }

    /// The bearer's platform standing **as this token carries it**:
    /// `(is_global_owner, standing)`.
    ///
    /// - `platform = false` carries none.
    /// - An all-access token carries it whole.
    /// - A token with grants carries it over those orgs only. Root is unbounded
    ///   by definition, so a bounded token cannot be root: a Global Owner's
    ///   narrowed token carries Global Admin over its orgs instead.
    pub fn narrow_platform(
        &self,
        is_global_owner: bool,
        standing: Option<PlatformStanding>,
    ) -> (bool, Option<PlatformStanding>) {
        if !self.platform {
            return (false, None);
        }
        if self.all_access {
            return (is_global_owner, standing);
        }
        let orgs = self.grant_orgs();
        if is_global_owner {
            let bounded = PlatformStanding::from_role(PlatformRole::GlobalAdmin, Scope::Orgs(orgs));
            return (false, Some(bounded));
        }
        let narrowed = standing.map(|mut s| {
            s.scope = match s.scope {
                Scope::All => Scope::Orgs(orgs),
                Scope::Orgs(have) => {
                    Scope::Orgs(have.into_iter().filter(|o| orgs.contains(o)).collect())
                }
            };
            s
        });
        (false, narrowed)
    }

    /// The bearer's partner standings as this token carries them: none without
    /// `partner`, and otherwise only the clients a grant touches.
    pub fn narrow_partners(&self, partners: Vec<PartnerStanding>) -> Vec<PartnerStanding> {
        if !self.partner {
            return Vec::new();
        }
        if self.all_access {
            return partners;
        }
        partners
            .into_iter()
            .map(|mut p| {
                p.client_orgs.retain(|o| self.touches_org(*o));
                p
            })
            .collect()
    }
}

impl PrincipalFacts {
    /// These facts as `token` carries them — the cap **at the source** (design
    /// §4.4). Every set shrinks or stays; none grows.
    ///
    /// Coarse on purpose: an org set cannot say "admin in workspace A, viewer
    /// in B", so this keeps an org wherever *some* grant reaches the level and
    /// leaves the exact, per-target answer to [`crate::allows`], which reads
    /// [`PrincipalFacts::token`]. `ws_admin_override` is keyed by workspace
    /// with no org beside it, so the loader — which can look the org up —
    /// narrows that one.
    pub fn narrowed_by(mut self, token: &TokenReach) -> Self {
        let reaches =
            |org: Uuid, level: RoleCeiling| token.ceiling_in_org(org).is_some_and(|c| c >= level);
        self.member_orgs
            .retain(|o| reaches(*o, RoleCeiling::Viewer));
        self.admin_orgs.retain(|o| reaches(*o, RoleCeiling::Admin));
        self.owned_orgs.retain(|o| reaches(*o, RoleCeiling::Owner));
        // A service account keeps its standing — it is what it *is* — but stands as
        // an admin only where a grant reaches that high.
        if let Some(account) = &mut self.service_account {
            account.admin = account.admin && reaches(account.org_id, RoleCeiling::Admin);
        }
        let (is_global_owner, platform) =
            token.narrow_platform(self.is_global_owner, self.platform.take());
        self.is_global_owner = is_global_owner;
        self.platform = platform;
        self.partners = token.narrow_partners(std::mem::take(&mut self.partners));
        self.token = Some(token.clone());
        self
    }

    /// Whether the credential carries platform standing. A session or a legacy
    /// key (`token = None`) always does. A **service account** never does: it
    /// holds no staff standing by construction (design §3.3), so a grant row
    /// someone later keys to it still reaches nothing.
    pub(crate) fn carries_platform(&self) -> bool {
        self.service_account.is_none() && self.token.as_ref().is_none_or(|t| t.platform)
    }

    /// Whether the credential carries partner standing. Never, for a service
    /// account — as [`Self::carries_platform`].
    pub(crate) fn carries_partner(&self) -> bool {
        self.service_account.is_none() && self.token.as_ref().is_none_or(|t| t.partner)
    }
}

#[cfg(test)]
#[path = "token_tests.rs"]
mod tests;
