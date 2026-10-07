//! `/api/admin/standing-tokens` — every personal token that carries its
//! owner's standing (`platform` or `partner`), for the staff who answer for
//! who holds staff access (API-tokens design, "Standing tokens, for staff").
//!
//! - `GET  /admin/standing-tokens` — list them, newest first, ended ones
//!   included.
//! - `POST /admin/standing-tokens/{id}/revoke` — revoke one.
//!
//! The route shape is the sandbox agent tokens' (`sandbox_agent_tokens`), and
//! so is the split: an owner manages their own token on `/api/user/tokens`,
//! and this is the view across owners. It sits behind
//! `manage_platform_grants`, the capability of the grant table
//! (`app_admins`): a token carrying a standing is that grant in credential
//! form, so whoever may say who holds staff access may see and end the
//! credentials that wield it. An App Operator does not hold it.
//!
//! **An unbounded grant only.** The capability gate decides on the platform
//! singleton, so a grant bounded to a few orgs passes it — and on the other
//! staff lists the handler then narrows the rows to those orgs. Here it
//! refuses the caller instead (403 `unbounded_grant_required`, on both routes,
//! before any token is read). A standing token is a credential for the whole
//! deployment: revoking one ends its reach in every org, and its row names
//! orgs and people outside a bounded grant, so no subset of these rows is a
//! bounded grant's to see or to end.
//!
//! Unlike the grant table, there is no `may_delegate` fence on the rows: a
//! revoke only takes a credential away, and its owner mints another from a
//! browser session. A Global Admin may revoke a peer's token or the Global
//! Owner's — a lost laptop must be containable by whoever is on call.
//!
//! This module is the section's mount, so the console's router inventory names
//! it like every other. The handlers live beside the rest of the token code
//! (`user_tokens::standing_staff`), which owns revocation and its audit row;
//! that is also where the session-only gate is taken and a bounded grant is
//! refused.

use axum::Router;
use axum::routing::{get, post};

use crate::server::api::user_tokens::standing_staff::{
    list_standing_tokens, revoke_standing_token,
};
use crate::server::router::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/standing-tokens", get(list_standing_tokens))
        .route("/standing-tokens/{id}/revoke", post(revoke_standing_token))
}
