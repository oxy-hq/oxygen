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
//! Since C2 the values live in [`contracts_authz::Caller`], the enforcement
//! port's plain projection, and this type wraps it (see [`Caller`]).
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
use std::ops::Deref;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{Extensions, StatusCode};
use entity::org_members::OrgRole;
use entity::workspace_members::WorkspaceRole;
use oxy_auth::token::{CredentialContext, StoredKind};
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::RoleCeiling;
use uuid::Uuid;

/// The principal behind a request, as its credential presents them.
///
/// The values and every pure read of them (ceilings, `carries_*`, the service
/// account and sandbox agent branches) are [`contracts_authz::Caller`]'s, reached
/// through `Deref`, so a domain that enforces through the port reads the same
/// answers from the same code. This wrapper keeps what needs `oxy-auth`, axum or a
/// clock: the projection from the authenticated request, the extractor, and the
/// two reads that answer an entity row or a `chrono` time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller(contracts_authz::Caller);

impl Deref for Caller {
    type Target = contracts_authz::Caller;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// The port's caller is this one's values: the memo `Arc` included, so a guard
/// and a port read of one request share one uncached grant read.
impl From<Caller> for contracts_authz::Caller {
    fn from(caller: Caller) -> Self {
        caller.0
    }
}

impl From<contracts_authz::Caller> for Caller {
    fn from(caller: contracts_authz::Caller) -> Self {
        Self(caller)
    }
}

impl Caller {
    /// The caller of a request: its authenticated user and the key or token it
    /// used, if any. A legacy credential narrows nothing, so it reads as a
    /// session does.
    ///
    /// `credential` is the request's `CredentialContext` marker when the call
    /// site has it in hand; otherwise the one the user carries is used — they
    /// are the same value, set together by the auth entry points.
    ///
    /// **This is the projection to the port's plain [`contracts_authz::Caller`]**:
    /// the token's expiry becomes a [`contracts_authz::UnixTime`], and a
    /// configured service's standing is read from its entry here, once.
    pub fn of(user: &AuthenticatedUser, credential: Option<&CredentialContext>) -> Self {
        let credential = credential.or(user.credential.as_ref());
        let token = credential.and_then(|c| {
            c.reach().map(|reach| contracts_authz::CallerToken {
                id: c.token_id,
                reach,
                expires_at: c.expires_at.map(|at| contracts_authz::UnixTime {
                    secs: at.timestamp(),
                    subsec_nanos: at.timestamp_subsec_nanos(),
                }),
            })
        });
        let account =
            credential
                .filter(|c| c.is_service_account())
                .map(|c| contracts_authz::CallerAccount {
                    standing: c.service_account,
                });
        let app_publish = credential
            .map(|c| c.app_publish.clone())
            .unwrap_or_default();
        let service = credential
            .filter(|c| c.kind == StoredKind::Service)
            .and_then(|c| oxy_auth::token::service::standing_of(c.token_id));
        Self(contracts_authz::Caller::from_parts(
            contracts_authz::CallerParts {
                user_id: user.id,
                email: user.email.clone().unwrap_or_default(),
                token,
                account,
                app_publish,
                service,
            },
        ))
    }

    /// The caller a request's user is: the user, with the key or token they
    /// authenticated with. For code that holds the request's
    /// `AuthenticatedUser` and nothing else.
    pub fn from_user(user: &AuthenticatedUser) -> Self {
        Self::of(user, None)
    }

    /// The caller of the request these extensions belong to, once auth has run.
    ///
    /// A caller already [left in the extensions](Self::share_with) is handed
    /// out as a clone, so every guard of the request shares what it has read.
    pub fn from_extensions(extensions: &Extensions) -> Option<Self> {
        if let Some(shared) = extensions.get::<Self>() {
            return Some(shared.clone());
        }
        let user = extensions.get::<AuthenticatedUser>()?;
        Some(Self::of(user, extensions.get::<CredentialContext>()))
    }

    /// The caller of the request `extensions` belong to, for a guard that
    /// already holds its `user`: the one [left there](Self::share_with) when
    /// there is one, otherwise [`Self::from_user`], as it always was. Only a
    /// sandbox agent token's caller is ever left there, so for every other
    /// credential this is `from_user` and nothing else.
    pub fn of_request(extensions: &Extensions, user: &AuthenticatedUser) -> Self {
        match extensions.get::<Self>() {
            Some(shared) => shared.clone(),
            None => Self::from_user(user),
        }
    }

    /// Build the request's caller once and leave it in its extensions, so
    /// every later [`Self::from_extensions`] shares one uncached read of the
    /// platform grant. Does nothing unless the credential is a sandbox agent
    /// token: no other caller reads uncached, so none has anything to share.
    pub fn share_with(extensions: &mut Extensions) {
        if extensions.get::<Self>().is_some() {
            return;
        }
        if let Some(caller) =
            Self::from_extensions(extensions).filter(|caller| caller.is_sandbox_agent())
        {
            extensions.insert(caller);
        }
    }

    /// A principal with **no credential in hand**: code acting for a user
    /// outside a request (a job, the CLI), or a subject looked up by id.
    ///
    /// Never build this inside a request to stand in for the requester — it
    /// drops the token's narrowing. Take [`Caller`] as an extractor, or
    /// [`Caller::of`], instead.
    pub fn without_credential(user_id: Uuid, email: &str) -> Self {
        Self(contracts_authz::Caller::without_credential(user_id, email))
    }

    /// The port's view of this caller: the same values, for a call through
    /// [`contracts_authz::AuthzEnforcer`].
    pub fn as_contract(&self) -> &contracts_authz::Caller {
        &self.0
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

    /// When the token this caller arrived with ends: `None` for a token that
    /// never expires, and for a login session or a configured service, which
    /// have none to report. The same instant `CredentialContext::expires_at`
    /// held: [`contracts_authz::UnixTime`] keeps the nanoseconds.
    pub fn token_expires_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.token_expires_at_unix()
            .and_then(|at| chrono::DateTime::from_timestamp(at.secs, at.subsec_nanos))
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
