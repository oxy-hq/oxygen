//! `/api/admin/sandbox-agent-tokens` — every staff member's sandbox agent
//! tokens, for the people who operate Oxy (sandbox agent credential design §2,
//! "Revoked by").
//!
//! - `GET  /admin/sandbox-agent-tokens` — list them, newest first.
//! - `POST /admin/sandbox-agent-tokens/{id}/revoke` — revoke one.
//!
//! The route shape is the publish tokens' (`app_publish_tokens`), and so is the
//! split: a minter manages their own token on `/api/user/tokens`, and this
//! shared cross-admin view is what `operate_platform` buys — an App Operator,
//! who may mint one, does not hold it.
//!
//! This module is the section's mount, so the console's router inventory names
//! it like every other. The handlers live beside the rest of the token code
//! (`user_tokens::sandbox_staff`), which owns revocation and its audit row;
//! that is also where the rows are narrowed to the caller's scope.

use axum::Router;
use axum::routing::{get, post};

use crate::server::api::user_tokens::sandbox_staff::{
    list_sandbox_agent_tokens, revoke_sandbox_agent_token,
};
use crate::server::router::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/sandbox-agent-tokens", get(list_sandbox_agent_tokens))
        .route(
            "/sandbox-agent-tokens/{id}/revoke",
            post(revoke_sandbox_agent_token),
        )
}
