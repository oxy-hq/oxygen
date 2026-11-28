//! **Who is asking**: the user, and the credential the request arrived with,
//! as plain values.
//!
//! Platform standing is keyed by email and partner standing by membership, and
//! both used to be read from a bare `(user_id, email)` pair. That pair cannot
//! say "this request came in on a token that carries no staff standing", so a
//! narrowed token would have held its bearer's full standing behind every
//! email-keyed door. [`Caller`] is what those doors take instead: the facts
//! loader, the platform-standing reads, `enforce_for`, the partner scope and
//! the assume-role liveness reads (API-tokens design §4.4).
//!
//! A browser session and a **legacy key** carry no token here; they decide
//! exactly as they always have (§3.5).
//!
//! A **service account** is the one caller that is not a person. Its standing
//! is not in `org_members` or any email-keyed table but on its own row, which
//! authentication reads on every request; the caller carries it
//! ([`Caller::account_standing`]), and the facts loader and the two role
//! resolutions take their service-account branch from it (§3.3).
//!
//! **Built by authentication, never by hand.** The request's caller comes from
//! `oxy_auth::caller_parts`, which projects `oxy_auth`'s `AuthenticatedUser`
//! and `CredentialContext` into [`CallerParts`] (`oxy_authz`'s, re-exported
//! here), and [`Caller::from_parts`]; `oxy_server_authz::Caller::of` and its
//! extractor do exactly that. `from_parts` is public only because that
//! projection lives in another crate: a caller built from invented parts holds
//! whatever standing the parts claim, as a [`Caller::without_credential`] built
//! for someone else always could. Never build one to stand in for a requester.
//! `crates/guards/tests/integration/caller_has_pinned_constructors.rs` pins
//! the files that call it.

use entity::org_members::OrgRole;
use oxy_authz::{
    AccountStanding, AppPublishGrant, CallerAccount, CallerParts, CallerToken, PlatformStanding,
    PrincipalFacts, RoleCeiling, SandboxAgentReach, TokenReach, UnixTime,
};
use std::sync::Arc;

use uuid::Uuid;

use crate::enforcer::AuthzEnforcer;
use crate::enforcer_handle::EnforcerHandle;
use crate::grant_memo::GrantMemo;

/// The principal behind a request, as its credential presents them, and the
/// enforcer to ask about them ([`Self::authz`]).
///
/// **Equality is by who the caller is.** Two callers built from the same parts
/// compare equal whatever enforcer handle or grant memo each holds: neither is
/// part of the principal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    pub user_id: Uuid,
    /// `""` for a user with no address (a frontline worker): the established
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
    /// The platform grant this request has read uncached, kept for its other
    /// guards. Written only for a sandbox agent token (the platform-standing
    /// read); never part of who the caller is.
    grant_memo: GrantMemo,
    /// Set when the request is a configured service (`oxysvc_`): the standing
    /// its `OXY_SERVICE_TOKENS` entry gives it, which the platform-standing
    /// read takes instead of the grant table.
    service: Option<PlatformStanding>,
    /// The enforcer every authorization read about this caller goes through.
    /// Set by whoever built the caller; never part of who the caller is.
    authz: EnforcerHandle,
}

impl Caller {
    /// The caller the projection from an authenticated request built, and the
    /// enforcer to ask about it. See the module doc: never build one from
    /// invented parts.
    pub fn from_parts(parts: CallerParts, authz: Arc<dyn AuthzEnforcer>) -> Self {
        Self {
            user_id: parts.user_id,
            email: parts.email,
            token: parts.token,
            account: parts.account,
            app_publish: parts.app_publish,
            grant_memo: GrantMemo::default(),
            service: parts.service,
            authz: EnforcerHandle(authz),
        }
    }

    /// The enforcer this caller was built with: what a domain holding a caller
    /// enforces through, so it needs no `AppState` and no process global.
    pub fn authz(&self) -> &Arc<dyn AuthzEnforcer> {
        &self.authz.0
    }

    /// A principal with **no credential in hand**: code acting for a user
    /// outside a request (a job, the CLI), or a subject looked up by id.
    ///
    /// Never build this inside a request to stand in for the requester: it
    /// drops the token's narrowing.
    pub fn without_credential(user_id: Uuid, email: &str, authz: Arc<dyn AuthzEnforcer>) -> Self {
        Self::from_parts(
            CallerParts {
                user_id,
                email: email.to_string(),
                ..CallerParts::default()
            },
            authz,
        )
    }

    /// The slot the platform-standing read keeps this request's uncached
    /// grant in. Only the enforcement implementation writes it.
    pub fn grant_memo(&self) -> &GrantMemo {
        &self.grant_memo
    }

    /// The standing of a configured service (`oxysvc_`), from its entry; `None`
    /// for every other caller. Read by the platform-standing read alone.
    pub fn service_standing(&self) -> Option<&PlatformStanding> {
        self.service.as_ref()
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
    /// Surfaces that act *as the caller* (minting the caller their own
    /// warehouse credential, say) refuse one by default (design §3.3): there
    /// is no person behind it to hold what they hand out.
    pub fn is_service_account(&self) -> bool {
        self.account.is_some()
    }

    /// Whether the request is a service account acting through a **CI run
    /// token** (`oxy_ci_`, minted by a trust policy for one CI run) rather than
    /// through its own long-lived token or as a person. Read by the compile
    /// check's guard (`role_guards::CompileChecker`) for its shipped term; the
    /// decision itself is `oxy_authz::Action::CompileCheck`.
    pub fn is_ci_run(&self) -> bool {
        self.is_service_account() && self.reach().is_some_and(|reach| reach.ci_run)
    }

    /// The service account's standing: its one org, and whether it is an
    /// admin there. `None` for a person, and for an account with none.
    pub fn account_standing(&self) -> Option<AccountStanding> {
        self.account.and_then(|a| a.standing)
    }

    /// The sandbox agent token the request authenticated with: its id, and
    /// the apps it names. `None` for every other credential.
    ///
    /// Such a caller is its minter confined to the sandbox loop on those apps
    /// (sandbox agent credential design §3.2). It holds **no tenant role**:
    /// the facts loader loads it none, and the two role resolutions answer it
    /// as they answer a workspace or an org that does not exist.
    pub fn sandbox_agent(&self) -> Option<&SandboxAgentReach> {
        self.reach().and_then(|reach| reach.sandbox_agent.as_ref())
    }

    /// Whether the request authenticated with a sandbox agent token.
    pub fn is_sandbox_agent(&self) -> bool {
        self.sandbox_agent().is_some()
    }

    /// The user's address, for display and logs. Not what standing is read
    /// by: that is [`Self::standing_email`].
    pub fn email(&self) -> &str {
        &self.email
    }

    /// What the token narrows its bearer to; `None` when nothing does.
    pub fn reach(&self) -> Option<&TokenReach> {
        self.token.as_ref().map(|t| &t.reach)
    }

    /// The address to read platform standing by: blank (nobody) when the
    /// token does not carry it.
    pub fn standing_email(&self) -> &str {
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
    /// all-access token: none of them has a grant to be bound by.
    ///
    /// The flat routes have no org or workspace in their path to check a grant
    /// against; this is what their one guard (`token_grant_scope`) keys on.
    pub fn bound_to_grants(&self) -> bool {
        self.reach().is_some_and(TokenReach::bound)
    }

    /// Is this an **all-access** token that an org has blocked, by its
    /// revoke-grant or by a token policy the token breaks? It loses that
    /// org's data and nothing else, so the same guard lets it onto the flat
    /// routes that leave a blocked org out. `false` for a session and a
    /// legacy key: nothing blocks either.
    pub fn blocked_somewhere(&self) -> bool {
        self.reach().is_some_and(TokenReach::blocked_somewhere)
    }

    /// Whether the request arrived on a login session: no token, no service
    /// account and no configured service.
    pub fn is_login_session(&self) -> bool {
        self.token.is_none() && self.account.is_none() && self.service.is_none()
    }

    /// The stored token (`api_tokens.id`) the request arrived with: what
    /// code acting for this person again outside the request re-admits by id
    /// (`oxy_auth::token::store::resolve_stored`), so it acts under the same
    /// ceiling. `None` for a login session, and for a configured service
    /// (`oxysvc_`), which has no stored row to re-admit.
    pub fn stored_token_id(&self) -> Option<Uuid> {
        if self.service.is_some() {
            return None;
        }
        self.token.as_ref().map(|t| t.id)
    }

    /// When the token this caller arrived with ends: `None` for a token that
    /// never expires, and for a login session or a configured service, which
    /// have none to report.
    pub fn token_expires_at_unix(&self) -> Option<UnixTime> {
        if self.service.is_some() {
            return None;
        }
        self.token.as_ref().and_then(|t| t.expires_at)
    }

    /// [`Self::token_expires_at_unix`] as the timestamp the credential held:
    /// [`UnixTime`] keeps the nanoseconds, so it is the same instant
    /// `CredentialContext::expires_at` held.
    pub fn token_expires_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.token_expires_at_unix()
            .and_then(|at| chrono::DateTime::from_timestamp(at.secs, at.subsec_nanos))
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

    /// Whether any grant touches the org: what discovery lists.
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
