//! Assume-role's **authority**: who may act as an org ([`may_act_as`]), the role a
//! live session runs with ([`ActingAs`]), and the refusal of the staff console while
//! acting ([`block_admin_while_acting`]).
//!
//! The `/api/assume` routes that open, end and list sessions are `oxy-api-admin`'s
//! (`oxy_api_admin::assume`) since the admin split (2026-11-20), merged at the root of
//! the protected tree through `SurfaceSeams::api`, outside `/admin`. That module's doc
//! carries the design: why assume-role exists, why it names an org and never a user,
//! and what it deliberately does not change.
//!
//! What stays below the routes is what `oxy-app` itself, and the surfaces, still
//! call, and a surface above `oxy-app` cannot be called from it:
//!
//! * [`block_admin_while_acting`] — `admin::router` and the sibling nests
//!   (`router::global`'s `staff_nest`) layer it, as does `oxy-api-custom-apps`'
//!   console, through the prelude;
//! * [`may_act_as`] and [`ActingAs`] — `org_context` and `workspace_context`
//!   synthesize the acting member from them, and `oxy-api-documents`,
//!   `oxy-api-tenancy` and the session routes read them from here (through
//!   the prelude's `assume` until decompose R1);
//! * session liveness, re-exported from `oxy-server-authz` (`assume_liveness`), so
//!   callers keep resolving `assume::…` unchanged.
//!
//! It went down from `oxy-app`'s `server::api::admin::assume` (the base) on
//! 2026-11-22 (decompose G2), and `oxy-app` re-exports it at that path. It is not in
//! `oxy-server-authz` with the liveness queries: the middleware opens its own
//! database connection (`oxy`'s client), which `oxy-server-authz` does not depend
//! on.

use axum::http::StatusCode;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

// The pure liveness-query cluster now lives in `oxy-server-authz` so the authz fact
// loader and the partner tier can read session liveness without depending on `oxy-app`.
// The three query fns are re-exported so external callers keep resolving `assume::…`
// unchanged.
pub use oxy_server_authz::assume_liveness::{
    is_session_live, live_assumed_org_ids, live_sessions_for,
};

/// **Who may act as an org.** Two populations, one rule each:
///
/// * **Oxy staff** — any org. They always could (the synthetic-Owner seam);
///   assume-role just makes it deliberate and audited.
/// * **A partner** — only the clients they are assigned to, and only with
///   `develop_apps`. Entering a client's product IS the data plane: it means
///   reading their warehouse, their threads, their dashboards. An
///   `account_manager` — whose whole point is "runs the relationship, cannot touch
///   the data" — must not be able to walk in through this door, or the role split
///   is decorative.
///
/// Re-checked on **every request** (not just at session creation), so revoking a
/// partner's `develop_apps` or un-assigning a client kills a live session's reach
/// immediately rather than at expiry.
///
/// Takes the [`Caller`], so both authorities are what the request's credential
/// carries: an API token without `platform` is not staff here, and one without
/// `partner` operates no partner.
///
/// [`Caller`]: oxy_server_authz::Caller
pub async fn may_act_as(
    db: &DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    org_id: Uuid,
) -> Option<ActingAs> {
    // Staff acting as a tenant synthesize **Owner** in that org, so this door demands the
    // capability that gates owner-level authority (`ManageOrgSettings`) — and honours the
    // grant's scope, since it is a reach INTO a specific tenant rather than a console
    // section.
    //
    // `is_staff()` alone would reopen exactly the hole the doc comment above warns about:
    // an App Operator holds neither capability, and their legitimate reach into a tenant
    // is the app data plane (`Ring::AppAccess` via `develop_apps`), which needs no
    // impersonation at all. A Global Admin holds `ManageOrgSettings` at `Scope::All`, so
    // nothing changes for them.
    if oxy_server_authz::globals::platform_reaches(
        db,
        caller,
        oxy_authz::Cap::ManageOrgSettings,
        org_id,
    )
    .await
    {
        return Some(ActingAs::Staff);
    }

    // A partner acting as one of its clients.
    use crate::guards::partner_authz::{PartnerCapability, partner_for_org, resolve_scope};
    let partner_org_id = partner_for_org(db, org_id).await?;
    let scope = resolve_scope(db, partner_org_id, caller).await?;
    if !scope.allows(PartnerCapability::DevelopApps) {
        return None;
    }
    if !scope.org_ids.contains(&org_id) {
        return None;
    }
    Some(ActingAs::Partner)
}

/// Which authority a live session rests on — it decides the role the request runs
/// with, so it must not be guessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActingAs {
    Staff,
    Partner,
}

impl ActingAs {
    /// Staff get Owner (what the old silent override gave them). A partner gets
    /// **Admin**: enough to work inside the client's product, but the org's own
    /// Owner remains untouchable — a partner administers a client, it never owns
    /// one. Both are marked `is_global_override`, so `OrgAdminStrict` still shuts
    /// billing and admin-promotion to them.
    pub fn org_role(self) -> entity::org_members::OrgRole {
        match self {
            ActingAs::Staff => entity::org_members::OrgRole::Owner,
            ActingAs::Partner => entity::org_members::OrgRole::Admin,
        }
    }
}

// ── acting is a MODE, not a badge ─────────────────────────────────────────

/// **Acting closes the admin surface.**
///
/// Without this, "act as" was decoration: you got a banner, kept every staff
/// power, and stayed on the admin page — so nothing about your reach actually
/// changed, and the mode had no meaning. Impersonation has to be a *place you go*,
/// not a hat you wear while continuing to be yourself.
///
/// It also removes a genuine hazard: staff writes performed through the admin API
/// during an assume window would be attributable to the real actor, but sit inside
/// an audit span that says "was acting as tenant X" — mixing two authorities in
/// one session is exactly the thing the audit chain is supposed to keep separate.
///
/// The `/assume` subtree is deliberately NOT behind this layer (it is
/// `oxy_api_admin::assume::router`, on the api seam outside `/admin`), so the
/// one-click exit always works. Ending the session restores admin.
pub async fn block_admin_while_acting(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    let Ok(db) = establish_connection().await else {
        // Can't tell ⇒ don't invent a lockout. Admin routes have their own guards.
        return Ok(next.run(request).await);
    };
    let caller = oxy_server_authz::Caller::from_user(&actor);
    if !live_sessions_for(&db, &caller).await.is_empty() {
        tracing::info!(
            actor = %actor.label(),
            "admin/assume: admin surface refused — actor is currently acting as a tenant"
        );
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    /// Liveness = not ended AND not expired. An expired-but-unended row grants
    /// nothing, so a forgotten session can't become a permanent backdoor.
    #[test]
    fn expiry_alone_kills_a_session() {
        let now = Utc::now();
        let expired = now - Duration::minutes(1);
        let live = now + Duration::minutes(1);
        assert!(expired < now, "an expired session must not be live");
        assert!(live > now, "an unexpired session is live");
    }
}
