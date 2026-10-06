//! The one place a handler asks "may I write **this** grant?".
//!
//! `platform_cap_guard` cannot answer it, for the same structural reason it cannot
//! answer scope: a guard sees a verb and a nil-org `Resource::platform()`, never the row
//! being written. `Cap::ManagePlatformGrants` gets a caller through the door of
//! `/admin/app-admins`; which rows they may touch once inside is this module.
//!
//! **Why the grant table stopped being owner-only.** It was, and the rationale in
//! `admin/mod.rs` was sound as far as it went: "a capability that could edit the grant
//! table would let its holder widen their own grant, and the ceiling would mean
//! nothing." That is an argument against an *unbounded* capability, not against
//! delegation. `oxy_authz::may_delegate` supplies the bound — a write is admissible only
//! against a grant strictly weaker than the writer's own — and under it the one row a
//! holder can never touch is their own. Owner stayed a bottleneck for "add another app
//! publisher", which is routine work.
//!
//! Everything about *what* is admissible lives in `oxy-authz`. This module is the
//! transport half: load the actor's standing ([`actor_facts`]), ask the model
//! (`may_delegate`), map the answer to a response ([`refuse`]).

use axum::http::StatusCode;
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::{DelegationDenial, PrincipalFacts};
use sea_orm::DatabaseConnection;

use crate::server::authz::globals;

/// The actor's platform standing, shaped for a delegation decision.
///
/// Loaded **once per request**, then reused for every row the request decides about —
/// `may_delegate` is pure, so one load serves any number of decisions and the list
/// endpoint does not re-read the grant table per staff member.
///
/// **500 on an unreadable grant, never "not staff".** `load_platform_facts` collapses a
/// `DbErr` into `None`, which reads identically to holding no grant — on a write path
/// that turns a transient blip into a silent denial, and worse, a `None` standing that
/// later gained a permissive default would fail *open*. This is the same rule
/// `admin::scope` applies: unknown is not unbounded, and on a write it is not "nothing"
/// either. It is a refusal that says so.
pub async fn actor_facts(
    db: &DatabaseConnection,
    actor: &AuthenticatedUser,
) -> Result<PrincipalFacts, StatusCode> {
    let platform =
        globals::platform_grant_checked(db, &crate::server::authz::Caller::from_user(actor))
            .await
            .map_err(|e| {
                tracing::error!(
                    target: "authz",
                    error = %e,
                    "platform grant unreadable on a grant-table decision — refusing"
                );
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    Ok(PrincipalFacts {
        user_id: actor.id,
        platform,
        is_global_owner: globals::is_global_owner(&crate::server::authz::Caller::from_user(actor)),
        ..Default::default()
    })
}

/// A refusal on this console: a status, and — when the delegation bound is what stopped
/// the request — which bound it was.
///
/// Exists because `DelegationDenial::as_str` promises the reason is "carried out to the
/// API so the console can say which bound was hit", and a bare `StatusCode` cannot keep
/// that promise: the frontend read `err.response.data.message` and rendered
/// "Request failed with status code 403", which is exactly the answer that doc contrasts
/// itself against.
///
/// `From<StatusCode>` is what keeps this cheap to adopt — every existing
/// `.map_err(|e| StatusCode::INTERNAL_SERVER_ERROR)?` in a handler converts through `?`
/// untouched, so only the return type changes.
pub struct Refusal {
    status: StatusCode,
    denial: Option<DelegationDenial>,
}

impl From<StatusCode> for Refusal {
    fn from(status: StatusCode) -> Self {
        Self {
            status,
            denial: None,
        }
    }
}

/// The operator-facing sentence for a denial. Kept beside the mapping rather than in the
/// frontend so a new `DelegationDenial` variant cannot ship with the console silently
/// falling back to "forbidden" — the `match` is exhaustive, so adding a variant fails the
/// build here.
fn denial_message(denial: DelegationDenial) -> &'static str {
    match denial {
        DelegationDenial::NotStaff => "You do not hold Oxy staff standing.",
        DelegationDenial::NoCapability => {
            "Your grant does not include managing other people's staff access."
        }
        DelegationDenial::RoleNotBelow => {
            "You can only issue a grant weaker than your own. Only a Global Owner can \
             add or change a Global Admin."
        }
        DelegationDenial::ScopeNotContained => {
            "You can only grant access to organizations your own grant reaches."
        }
    }
}

impl axum::response::IntoResponse for Refusal {
    fn into_response(self) -> axum::response::Response {
        match self.denial {
            // `message` is the field the frontend's error handler already reads; `denial`
            // is the stable id, for a client that wants to branch rather than display.
            Some(denial) => (
                self.status,
                axum::Json(serde_json::json!({
                    "message": denial_message(denial),
                    "denial": denial.as_str(),
                })),
            )
                .into_response(),
            None => self.status.into_response(),
        }
    }
}

/// Map a model verdict onto a refusal, logging the bound that was hit.
///
/// **403, not 404.** Every other out-of-scope refusal on this console answers 404 so an
/// operator cannot enumerate tenants they have no reach into. That reasoning does not
/// transfer: the grant list is staff-internal and returns every row to anyone who may
/// open it, so there is no existence to conceal — and "you cannot issue a grant wider
/// than your own" is a rule the operator can act on, where a 404 would send them to ask
/// someone why the console is lying about a row they can see.
///
/// **The contract for a handler on this surface is two steps, not one:**
/// [`actor_facts`] once per request, then `refuse(may_delegate(&facts, role, &scope))`
/// once per row the request touches — twice for an upsert that replaces an existing
/// grant, since the row being destroyed needs authorizing as well as the one being
/// written.
///
/// There was a `deny_undelegatable(db, actor, role, scope)` one-liner here that bundled
/// both steps. Nothing called it: it takes a `&DatabaseConnection`, and the check that
/// matters most runs against the *locked* row inside a transaction (`&DatabaseTransaction`),
/// so no correct handler could use it. A helper no correct caller can call, named by the
/// docs as the contract, is worse than none — the next author follows the doc, re-reads
/// the caller's standing per row, and still misses the locked read.
pub fn refuse(verdict: Result<(), DelegationDenial>, actor_email: &str) -> Result<(), Refusal> {
    match verdict {
        Ok(()) => Ok(()),
        Err(denial) => {
            tracing::warn!(
                target: "authz",
                actor = %actor_email,
                denial = denial.as_str(),
                "refused a platform-grant write outside the delegation bound"
            );
            Err(Refusal {
                status: StatusCode::FORBIDDEN,
                denial: Some(denial),
            })
        }
    }
}
