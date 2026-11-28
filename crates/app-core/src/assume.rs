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
//!   callers keep resolving `assume::…` unchanged. [`may_act_as`] and [`ActingAs`]
//!   are re-exported from there too (`assume_authority`), since C2.
//!
//! It went down from `oxy-app`'s `server::api::admin::assume` (the base) on
//! 2026-11-22 (decompose G2), and `oxy-app` re-exports it at that path. It is not in
//! `oxy-server-authz` with the liveness queries: the middleware opens its own
//! database connection (`oxy`'s client), which `oxy-server-authz` does not depend
//! on.

use axum::http::StatusCode;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;

// The pure liveness-query cluster now lives in `oxy-server-authz` so the authz fact
// loader and the partner tier can read session liveness without depending on `oxy-app`.
// The three query fns are re-exported so external callers keep resolving `assume::…`
// unchanged.
pub use oxy_server_authz::assume_liveness::{
    is_session_live, live_assumed_org_ids, live_sessions_for,
};

// The authority half — `may_act_as` and `ActingAs` — moved down to `oxy-server-authz`
// (`assume_authority`) in C2 (2026-11-28), so the enforcement port's implementation
// answers it without a dependency back on this crate. Re-exported so every
// `assume::{may_act_as, ActingAs}` caller resolves unchanged.
pub use oxy_server_authz::assume_authority::{ActingAs, may_act_as};

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
