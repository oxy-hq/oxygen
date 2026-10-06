//! `GET /api/{workspace_id}/api-tokens` — "tokens with access to this
//! workspace" — under the org's token policy (design §5).
//!
//! A token the policy blocks is inert in the org. The list used to show it as
//! active with its owner's reach; it now computes reach as a request does,
//! policy blocks included, so a blocked token is not listed. The real handler
//! runs behind the served auth stack and the workspace tree's authorization
//! step.

use axum::http::StatusCode;
use axum::routing::get;
use axum::{Router, middleware};
use chrono::{Duration, Utc};
use oxy_app::server::api::middlewares::workspace_context::workspace_access_middleware;
use oxy_app::server::api::user_tokens::inventory::list_workspace_tokens;
use oxy_auth::token::credential::source;
use oxy_auth::token::personal::{self, NewToken};
use oxy_authz::RoleCeiling;
use serde_json::{Value, json};
use uuid::Uuid;

use super::service_accounts::{admin_fixture, in_session};
use super::stack::{Reach, org_grant, workspace_grant};
use super::{Fixture, call, legacy_key, seed_org, seed_workspace_in};

fn inventory_api() -> Router {
    let workspace = Router::new()
        .route("/api-tokens", get(list_workspace_tokens))
        .layer(middleware::from_fn(workspace_access_middleware));
    oxy_app::server::router::api_auth_layers(Router::new().nest("/{workspace_id}", workspace))
}

/// The ids the workspace's inventory lists, as its admin.
async fn listed(fx: &Fixture, workspace: Uuid) -> Vec<Uuid> {
    let uri = format!("/{workspace}/api-tokens");
    let (status, body) = call(
        inventory_api(),
        "GET",
        &uri,
        &[("cookie", &fx.cookie)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    let mut ids: Vec<Uuid> = super::stack::ids(&body["tokens"]);
    ids.sort();
    ids
}

fn sorted(mut ids: Vec<Uuid>) -> Vec<Uuid> {
    ids.sort();
    ids
}

/// A personal token of the fixture's user, expiring `days` from now.
async fn mint(fx: &Fixture, reach: Reach, days: Option<i64>) -> Uuid {
    personal::create(
        &fx.db,
        NewToken {
            user_id: fx.user.id,
            name: "inventory".into(),
            all_access: reach.all_access,
            platform: reach.platform,
            partner: reach.partner,
            grants: reach.grants,
            expires_at: days.map(|d| Utc::now() + Duration::days(d)),
            source: source::UI,
        },
    )
    .await
    .expect("mint a personal token")
    .row
    .id
}

async fn put_policy(fx: &Fixture, org: Uuid, max_lifetime_days: Option<i64>, all_access: bool) {
    let body: Value = json!({
        "max_lifetime_days": max_lifetime_days,
        "allow_all_access_tokens": all_access,
        "require_environment_on_trust_policies": true,
    });
    let uri = format!("/orgs/{org}/token-policy");
    let (status, saved) = in_session(&fx.cookie, "PUT", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "put policy: {saved}");
}

#[tokio::test]
async fn a_token_the_orgs_policy_blocks_is_not_listed_as_reaching_the_workspace() {
    let fx = admin_fixture().await;
    let ws = fx.workspace_id;
    let all_access = mint(&fx, Reach::all_access(), Some(30)).await;
    let granted = |ceiling| Reach::granted(vec![workspace_grant(fx.org_id, ws, ceiling)]);
    let short = mint(&fx, granted(RoleCeiling::Member), Some(30)).await;
    let long = mint(&fx, granted(RoleCeiling::Admin), Some(400)).await;
    let forever = mint(
        &fx,
        Reach::granted(vec![org_grant(fx.org_id, RoleCeiling::Viewer)]),
        None,
    )
    .await;
    // A legacy key is never listed here, policy or not: it is not a token.
    legacy_key(&fx, None).await;

    let everything = sorted(vec![all_access, short, long, forever]);
    assert_eq!(listed(&fx, ws).await, everything, "no policy yet");

    // The org refuses all-access tokens: that one is inert here, so it goes.
    put_policy(&fx, fx.org_id, None, false).await;
    assert_eq!(
        listed(&fx, ws).await,
        sorted(vec![short, long, forever]),
        "an all-access token the policy blocks"
    );

    // A 90-day cap: the 400-day token and the one that never expires go too.
    put_policy(&fx, fx.org_id, Some(90), false).await;
    assert_eq!(listed(&fx, ws).await, [short], "only the compliant token");

    // Lifting the policy brings them back — blocked, never revoked.
    put_policy(&fx, fx.org_id, None, true).await;
    assert_eq!(listed(&fx, ws).await, everything);
}

#[tokio::test]
async fn another_orgs_policy_does_not_take_a_token_off_this_workspaces_list() {
    // The owner is in two orgs; org B's policy blocks the all-access token in
    // org B only, so org A's workspace still lists it.
    let fx = admin_fixture().await;
    let org_b = seed_org(&fx.db).await;
    super::stack::join_org(
        &fx.db,
        org_b,
        fx.user.id,
        entity::org_members::OrgRole::Owner,
    )
    .await;
    let ws_b = seed_workspace_in(&fx.db, org_b).await;
    let all_access = mint(&fx, Reach::all_access(), Some(30)).await;

    put_policy(&fx, org_b, None, false).await;
    assert_eq!(listed(&fx, fx.workspace_id).await, [all_access]);
    assert!(listed(&fx, ws_b).await.is_empty());
}
