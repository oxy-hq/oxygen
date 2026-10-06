//! **Who is asking** — the user, and the credential the request arrived with.
//!
//! Platform standing is keyed by email and partner standing by membership, and
//! both used to be read from a bare `(user_id, email)` pair. That pair cannot
//! say "this request came in on a token that carries no staff standing", so a
//! narrowed token would have held its bearer's full standing behind every
//! email-keyed door. [`Caller`] is what those doors take instead: the facts
//! loader, [`crate::globals`], [`crate::enforce_for`], the partner scope and
//! the assume-role liveness reads (API-tokens design §4.4).
//!
//! A browser session and a **legacy key** carry no token here — they decide
//! exactly as they always have (§3.5).
//!
//! A **service account** is the one caller that is not a person. Its standing
//! is not in `org_members` or any email-keyed table but on its own row, which
//! authentication reads on every request; the caller carries it
//! ([`Caller::account_role_in`]), and the facts loader and the two role
//! resolutions take their service-account branch from it (§3.3).

use std::future::Future;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{Extensions, StatusCode};
use entity::org_members::OrgRole;
use entity::workspace_members::WorkspaceRole;
use oxy_auth::token::{AccountStanding, AppPublishGrant, CredentialContext};
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::{PrincipalFacts, RoleCeiling, TokenReach};
use uuid::Uuid;

/// The new-format token a request authenticated with.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CallerToken {
    id: Uuid,
    reach: TokenReach,
}

/// The principal behind a request, as its credential presents them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    pub user_id: Uuid,
    /// `""` for a user with no address (a frontline worker) — the established
    /// "holds no platform standing" value.
    email: String,
    /// `None` for a browser session and for a legacy key.
    token: Option<CallerToken>,
    /// Set when the request acts as a service account: with the account's
    /// own token, or with a `ci` token a trust policy minted for it.
    account: Option<CallerAccount>,
    /// The apps the credential's `app_publish` grants name. Empty for a
    /// session, a legacy key and every token that holds none.
    app_publish: Vec<AppPublishGrant>,
}

/// The service account a request acts as. `standing` is `None` only if the
/// credential named an account without one — which reads as an account with
/// no standing anywhere, never as a person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CallerAccount {
    standing: Option<AccountStanding>,
}

impl Caller {
    /// The caller of a request: its authenticated user and the key or token it
    /// used, if any. A legacy credential narrows nothing, so it reads as a
    /// session does.
    ///
    /// `credential` is the request's `CredentialContext` marker when the call
    /// site has it in hand; otherwise the one the user carries is used — they
    /// are the same value, set together by the auth entry points.
    pub fn of(user: &AuthenticatedUser, credential: Option<&CredentialContext>) -> Self {
        let credential = credential.or(user.credential.as_ref());
        let token = credential.and_then(|c| {
            c.reach().map(|reach| CallerToken {
                id: c.token_id,
                reach,
            })
        });
        let account = credential
            .filter(|c| c.is_service_account())
            .map(|c| CallerAccount {
                standing: c.service_account,
            });
        // A legacy credential narrows nothing, so a grant row beside it — which
        // admission refuses anyway — is never read as a confinement.
        let app_publish = credential
            .filter(|c| !c.is_legacy())
            .map(|c| c.app_publish.clone())
            .unwrap_or_default();
        Self {
            user_id: user.id,
            email: user.email.clone().unwrap_or_default(),
            token,
            account,
            app_publish,
        }
    }

    /// The caller a request's user is: the user, with the key or token they
    /// authenticated with. For code that holds the request's
    /// `AuthenticatedUser` and nothing else.
    pub fn from_user(user: &AuthenticatedUser) -> Self {
        Self::of(user, None)
    }

    /// The caller of the request these extensions belong to, once auth has run.
    pub fn from_extensions(extensions: &Extensions) -> Option<Self> {
        let user = extensions.get::<AuthenticatedUser>()?;
        Some(Self::of(user, extensions.get::<CredentialContext>()))
    }

    /// A principal with **no credential in hand**: code acting for a user
    /// outside a request (a job, the CLI), or a subject looked up by id.
    ///
    /// Never build this inside a request to stand in for the requester — it
    /// drops the token's narrowing. Take [`Caller`] as an extractor, or
    /// [`Caller::of`], instead.
    pub fn without_credential(user_id: Uuid, email: &str) -> Self {
        Self {
            user_id,
            email: email.to_string(),
            token: None,
            account: None,
            app_publish: Vec::new(),
        }
    }

    /// Whether the credential holds any `app_publish` grant. One that does
    /// publishes only the apps its grants name.
    pub fn holds_app_publish(&self) -> bool {
        !self.app_publish.is_empty()
    }

    /// Whether an `app_publish` grant names this app of this org.
    pub fn publishes_app(&self, org_id: Uuid, app_id: Uuid) -> bool {
        self.app_publish
            .iter()
            .any(|g| g.org_id == org_id && g.app_id == app_id)
    }

    /// Whether the request acts as an org's service account, not as a person.
    ///
    /// Surfaces that act *as the caller* — minting the caller their own
    /// warehouse credential, say — refuse one by default (design §3.3): there
    /// is no person behind it to hold what they hand out.
    pub fn is_service_account(&self) -> bool {
        self.account.is_some()
    }

    /// The service account's standing: its one org, and whether it is an
    /// admin there. `None` for a person — and for an account with none.
    pub fn account_standing(&self) -> Option<AccountStanding> {
        self.account.and_then(|a| a.standing)
    }

    /// The role a service account holds in `org_id`, from its own row: Admin
    /// or Member in its org, `None` anywhere else. **Never Owner.** `None` for
    /// a person, whose role is their `org_members` row.
    pub fn account_role_in(&self, org_id: Uuid) -> Option<OrgRole> {
        let standing = self.account_standing().filter(|s| s.org_id == org_id)?;
        Some(if standing.admin {
            OrgRole::Admin
        } else {
            OrgRole::Member
        })
    }

    /// The user's address, for display and logs. Not what standing is read
    /// by — that is [`Self::standing_email`].
    pub fn email(&self) -> &str {
        &self.email
    }

    /// What the token narrows its bearer to; `None` when nothing does.
    pub fn reach(&self) -> Option<&TokenReach> {
        self.token.as_ref().map(|t| &t.reach)
    }

    /// The address to read platform standing by: blank — nobody — when the
    /// token does not carry it.
    pub(crate) fn standing_email(&self) -> &str {
        if self.carries_platform() {
            &self.email
        } else {
            ""
        }
    }

    /// Whether the bearer's Oxy-staff standing rides this credential.
    ///
    /// Never, for a service account: it is not staff, by construction.
    pub fn carries_platform(&self) -> bool {
        !self.is_service_account() && self.reach().is_none_or(|r| r.platform)
    }

    /// Whether the bearer's partner standings ride this credential.
    /// Never, for a service account.
    pub fn carries_partner(&self) -> bool {
        !self.is_service_account() && self.reach().is_none_or(|r| r.partner)
    }

    /// Is this a token that reaches only what its grants name
    /// (`all_access = false`)? `false` for a session, a legacy key and an
    /// all-access token — none of them has a grant to be bound by.
    ///
    /// The flat routes have no org or workspace in their path to check a grant
    /// against; this is what their one guard (`token_grant_scope`) keys on.
    pub fn bound_to_grants(&self) -> bool {
        self.reach().is_some_and(TokenReach::bound)
    }

    /// Is this an **all-access** token that an org has blocked — by its
    /// revoke-grant, or by a token policy the token breaks? It loses that
    /// org's data and nothing else, so the same guard lets it onto the flat
    /// routes that leave a blocked org out. `false` for a session and a
    /// legacy key: nothing blocks either.
    pub fn blocked_somewhere(&self) -> bool {
        self.reach().is_some_and(TokenReach::blocked_somewhere)
    }

    /// The org membership a service account stands in for in `org_id` — a
    /// synthesized row (nil id) carrying [`Self::account_role_in`], for the two
    /// role resolutions that hand an `org_members` row to everything after
    /// them. `None` for a person, and for an account outside its org.
    ///
    /// It is never written anywhere: a service account has no `org_members`
    /// row, which is what keeps it out of seats, member lists and audiences.
    pub fn account_membership(&self, org_id: Uuid) -> Option<entity::org_members::Model> {
        let role = self.account_role_in(org_id)?;
        let now = chrono::Utc::now().into();
        Some(entity::org_members::Model {
            id: Uuid::nil(),
            org_id,
            user_id: self.user_id,
            role,
            created_at: now,
            updated_at: now,
        })
    }

    /// The assume-role sessions this credential may use: `Some(id)` = only the
    /// ones this new-format token opened; `None` = the ones opened in a browser
    /// (a session, and a legacy key, which inherits them).
    pub fn assume_binding(&self) -> Option<Uuid> {
        self.token.as_ref().map(|t| t.id)
    }

    /// The ceiling over the org's own routes. `None` = outside every grant;
    /// `Owner` (no cap) when no token narrows.
    pub fn org_ceiling(&self, org_id: Uuid) -> Option<RoleCeiling> {
        match self.reach() {
            None => Some(RoleCeiling::Owner),
            Some(reach) => reach.org_ceiling(org_id),
        }
    }

    /// The ceiling over one workspace; as [`Self::org_ceiling`].
    pub fn workspace_ceiling(&self, org_id: Uuid, workspace_id: Uuid) -> Option<RoleCeiling> {
        match self.reach() {
            None => Some(RoleCeiling::Owner),
            Some(reach) => reach.workspace_ceiling(org_id, workspace_id),
        }
    }

    /// The highest ceiling of any grant in the org, org-wide or not: the reach
    /// over an org's children that name no workspace. `None` = the token does
    /// not touch the org; `Owner` when no token narrows.
    pub fn org_reach_ceiling(&self, org_id: Uuid) -> Option<RoleCeiling> {
        match self.reach() {
            None => Some(RoleCeiling::Owner),
            Some(reach) => reach.ceiling_in_org(org_id),
        }
    }

    /// Whether any grant touches the org — what discovery lists.
    pub fn touches_org(&self, org_id: Uuid) -> bool {
        self.reach().is_none_or(|r| r.touches_org(org_id))
    }

    /// The facts as this credential carries them: capped at the source.
    pub fn narrow(&self, facts: PrincipalFacts) -> PrincipalFacts {
        match self.reach() {
            None => facts,
            Some(reach) => facts.narrowed_by(reach),
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let result = Self::from_extensions(&parts.extensions).ok_or(StatusCode::UNAUTHORIZED);
        async move { result }
    }
}

/// `min(role, ceiling)` for a workspace role.
pub fn cap_workspace_role(role: WorkspaceRole, ceiling: RoleCeiling) -> WorkspaceRole {
    let cap = match ceiling {
        RoleCeiling::Viewer => WorkspaceRole::Viewer,
        RoleCeiling::Member => WorkspaceRole::Member,
        RoleCeiling::Admin => WorkspaceRole::Admin,
        RoleCeiling::Owner => WorkspaceRole::Owner,
    };
    std::cmp::min(role, cap)
}

/// An org role under a ceiling: Owner and Admin are capped at the ceiling, and
/// anything below admin is Member — an org has no lower role (design §3.2).
pub fn cap_org_role(role: OrgRole, ceiling: RoleCeiling) -> OrgRole {
    match (role, ceiling) {
        (role, RoleCeiling::Owner) => role,
        (OrgRole::Owner | OrgRole::Admin, RoleCeiling::Admin) => OrgRole::Admin,
        _ => OrgRole::Member,
    }
}

#[cfg(test)]
#[path = "caller_tests.rs"]
mod tests;
