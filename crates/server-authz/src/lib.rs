//! App-side authorization wiring. The decision layer itself lives in the
//! transport-agnostic [`oxy_authz`] crate; this module re-exports it so call sites use
//! `crate::server::authz::{Action, Resource, allows, enforce, ...}`, and adds the one
//! piece that can't live in the crate — the [`loader`] that fills
//! [`oxy_authz::PrincipalFacts`] from the database.
//!
//! Why the loader stays here: it reaches into app-specific primitives (org
//! membership, partner scope, the global-admin table). Moving it into `oxy-authz`
//! would make the crate depend back on `oxy-app` — a cycle. So the crate owns the
//! MODEL (portable), and the app owns FACT-LOADING (app-shaped).
//!
//! **Wiring a call site:** use [`enforce_guard`] (in a guard) or [`enforce_for`] (a
//! handler holding a DB handle), passing the shipped check as `existing_allow`. The
//! decision is `existing_allow && allows(..)`, so the model can only ever subtract, and
//! `existing_allow` is also the oracle the differential tests difference against. Call
//! [`require`] directly only where no legacy check exists — mapping [`oxy_authz::Denied`]
//! to 403 (or 404 for existence hiding).

pub use oxy_authz::*;

pub mod assume_liveness;
pub mod caller;
pub mod globals;
pub mod loader;
pub mod org_context;
pub mod oxy_owner_guard;
pub mod partner_authz;
pub mod role_guards;
pub mod workspace_role;

#[cfg(test)]
mod differential;

use axum::http::request::Parts;
use uuid::Uuid;

pub use crate::caller::{Caller, cap_org_role, cap_workspace_role};
use crate::partner_authz::{PartnerCapability, PartnerScope};
use oxy_platform::db::establish_connection;

/// The principal's facts for THIS request, loaded once and memoized in the request's
/// extensions so repeated guard checks in a single request pay the load at most once.
///
/// `None` means the facts are **unknown**: no authenticated user, no connection, or a
/// lookup that errored. It never means "this principal has no standing" — the caller
/// decides what unknown costs, and for [`enforce_guard`] that is deferring to the
/// shipped check.
///
/// Only a successful load is memoized, so a blip on one guard doesn't pin an unknown
/// verdict onto every later guard in the same request.
pub async fn request_facts(parts: &mut Parts) -> Option<PrincipalFacts> {
    if let Some(facts) = parts.extensions.get::<PrincipalFacts>() {
        return Some(facts.clone());
    }
    // The caller is the user AND the credential the request arrived with, so an
    // API token's narrowing reaches every guard through these facts.
    //
    // A user with no email holds no platform standing, and that is the only
    // thing the loader reads an address for. `is_oxy_owner` refuses a blank
    // needle and `grant_of_email` short-circuits a blank key, so "" is the
    // established "no standing" value rather than a hole.
    let caller = Caller::from_extensions(&parts.extensions)?;
    let db = establish_connection().await.ok()?;
    let facts = loader::load_principal_facts(&db, &caller).await?;
    parts.extensions.insert(facts.clone());
    Some(facts)
}

/// **Enforce** one guard's ring: load the request's facts (memoized) and return
/// `existing_allow && allows(..)`. Wiring this into a guard makes the model binding for
/// every route that guard protects — which is why the six role guards are the highest-
/// leverage call sites in the system.
///
/// Fail-safe by construction: if the facts are unknown — no authenticated user, no
/// connection, **or a lookup that errored** — we return `existing_allow` untouched
/// rather than denying. A database blip must not lock every tenant out of their org.
/// The model only ever subtracts, so that fallback cannot open a hole either.
///
/// That last case is the one that used to slip through. The loader averaged a failed
/// query into an empty set, which reads as the *fact* "belongs to no org" — so the model
/// denied and the conjunction 403'd a real org officer, never reaching this branch.
/// Unknown is now `None`, and lands here.
pub async fn enforce_guard(
    parts: &mut Parts,
    label: &str,
    action: Action,
    resource: Resource,
    existing_allow: bool,
) -> bool {
    match request_facts(parts).await {
        Some(facts) => enforce(label, &facts, action, &resource, existing_allow),
        None => existing_allow,
    }
}

/// Enforce from a NON-guard call site that already holds a DB handle and the
/// [`Caller`]. Loads the caller's facts — capped by the API token the request arrived
/// with, if any — and returns `existing_allow && allows(..)`.
///
/// Unknown facts defer to `existing_allow` for a session, exactly as in
/// [`enforce_guard`]. For a **narrowing token** unknown facts deny: `existing_allow`
/// at these call sites is computed from the bearer's uncapped standing, so deferring
/// to it would hand the token its bearer's full reach for as long as the blip lasts.
///
/// Call sites on a hot path should instead load scoped facts
/// ([`loader::load_principal_facts_scoped`]) and call [`enforce`] directly, so they
/// don't pay for facts their ring never reads — see the custom-app data-plane gate.
/// If you do that, handle the `None` the same way: defer, don't deny.
pub async fn enforce_for(
    db: &sea_orm::DatabaseConnection,
    caller: &Caller,
    label: &str,
    action: Action,
    resource: Resource,
    existing_allow: bool,
) -> bool {
    match loader::load_principal_facts(db, caller).await {
        Some(facts) => enforce(label, &facts, action, &resource, existing_allow),
        None => existing_allow && !caller.reach().is_some_and(TokenReach::narrows),
    }
}

/// Translate the partner tier's capability enum into the authz vocabulary.
pub fn cap_of(cap: PartnerCapability) -> Cap {
    match cap {
        PartnerCapability::ManageMembers => Cap::ManageMembers,
        PartnerCapability::ManageApps => Cap::ManageApps,
        PartnerCapability::DevelopApps => Cap::DevelopApps,
        PartnerCapability::ViewAudit => Cap::ViewAudit,
        PartnerCapability::ManageBilling => Cap::ManageBilling,
        PartnerCapability::ManageSecrets => Cap::ManageSecrets,
        PartnerCapability::CreateOrgs => Cap::CreateOrgs,
        PartnerCapability::ManageOrgSettings => Cap::ManageOrgSettings,
    }
}

/// The action carrying `cap`'s ring.
pub fn partner_action(cap: PartnerCapability) -> Action {
    match cap {
        PartnerCapability::ManageMembers => Action::PartnerManageMembers,
        PartnerCapability::ManageApps => Action::PartnerManageApps,
        PartnerCapability::DevelopApps => Action::PartnerDevelopApps,
        PartnerCapability::ViewAudit => Action::PartnerViewAudit,
        PartnerCapability::ManageBilling => Action::PartnerManageBilling,
        PartnerCapability::ManageSecrets => Action::PartnerManageSecrets,
        PartnerCapability::CreateOrgs => Action::PartnerCreateOrgs,
        PartnerCapability::ManageOrgSettings => Action::PartnerManageOrgSettings,
    }
}

/// Facts for a decision that is ALREADY scoped to one partner — the console, where
/// `partner_middleware` resolved the [`PartnerScope`] (covering both a real operator and
/// a live assume session).
///
/// Pure and DB-free: the scope IS the standing, so there is nothing to load. `user_id`
/// is not read by any partner ring — the standing decides — so this is safe to build
/// without one.
fn partner_scope_facts(scope: &PartnerScope) -> PrincipalFacts {
    PrincipalFacts {
        partners: vec![PartnerStanding {
            partner_id: scope.partner_id,
            client_orgs: scope.org_ids.clone(),
            caps: PartnerCapability::ALL
                .into_iter()
                .filter(|c| scope.capabilities.allows(*c))
                .map(cap_of)
                .collect(),
        }],
        // The token the scope was resolved under decides with it: coverage, the
        // partner gate and the ceiling all apply in `allows`.
        token: scope.token.clone(),
        ..Default::default()
    }
}

/// Does the partner this scope is acting as hold `cap` — over `org_id` if given, or at
/// all if not?
///
/// This is the partner tier's decision, made by the same model as everything else. It
/// replaces the entirely separate decision engine the partner tier used to ship
/// (`partner_policy`), and is identical to it by construction rather than by luck:
/// the scope and the facts now come from the same standings
/// (`partner_authz::operated_partners` / `assumed_partners`), so the two cannot drift.
pub fn partner_allows(scope: &PartnerScope, org_id: Option<Uuid>, cap: PartnerCapability) -> bool {
    let facts = partner_scope_facts(scope);
    let resource = match org_id {
        // Scoped to a client: the capability must come from THIS partner, and the org
        // must be one of its clients.
        Some(org_id) => Resource::partner_client(org_id, scope.partner_id),
        // Capability-only (create-orgs, view-audit): the partner itself is the target.
        None => Resource::partner(scope.partner_id),
    };
    allows(&facts, partner_action(cap), &resource)
}
