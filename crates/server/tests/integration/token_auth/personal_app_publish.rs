//! `app_publish` on a **personal** token (API-tokens design §3.2), as built.
//!
//! - **No route makes one.** `POST /api/user/tokens` refuses a body that
//!   names the grant, and `PATCH` cannot add it. Both are 400.
//! - **A row written by hand is admitted, and only narrows.** On the
//!   custom-apps surface the token reaches the app it names and nothing else
//!   of that surface, whatever its workspace grants and standing reach. Alone
//!   — no workspace grant — it reaches no org, so nothing behind a gate.
//!
//! What such a token may *publish* is decided in `publish()`, and driven
//! there: `custom_apps_publish_machine/personal_grant.rs` in `oxy-app`.

use axum::http::StatusCode;
use entity::api_token_grants;
use oxy_authz::RoleCeiling;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use serde_json::json;
use uuid::Uuid;

use super::pat_row;
use super::sandbox_agent::{grants_of, staff_with_app};
use super::service_accounts::{admin_fixture, in_session, with_token, workspace_status};
use super::stack::{Reach, get_in_session, mint, org_grant, published_app};

/// An `app_publish` grant on `token_id`, written straight to the table.
async fn grant_app_publish(db: &DatabaseConnection, org_id: Uuid, token_id: Uuid, app_id: Uuid) {
    api_token_grants::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        token_id: ActiveValue::Set(token_id),
        kind: ActiveValue::Set(api_token_grants::KIND_APP_PUBLISH.to_string()),
        org_id: ActiveValue::Set(org_id),
        workspace_id: ActiveValue::Set(None),
        role_ceiling: ActiveValue::Set(None),
        app_id: ActiveValue::Set(Some(app_id)),
        created_at: ActiveValue::Set(chrono::Utc::now().fixed_offset()),
        revoked_at: ActiveValue::Set(None),
        revoked_by: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("write the app_publish grant by hand");
    oxy_auth::token::cache::clear();
}

async fn status_as(secret: &str, method: &str, uri: &str) -> StatusCode {
    with_token(secret, method, uri, None).await.0
}

#[tokio::test]
async fn no_route_puts_an_app_publish_grant_on_a_personal_token() {
    let fx = admin_fixture().await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let publish = json!({ "kind": "app_publish", "app_id": app.id });
    let whole_org = json!({ "org_id": fx.org_id, "role_ceiling": "admin" });

    // Create: alone, and beside a workspace grant the owner may give.
    for grants in [json!([publish]), json!([whole_org, publish])] {
        let body = json!({ "name": "ci", "all_access": false, "grants": grants });
        let (status, refused) = in_session(&fx.cookie, "POST", "/user/tokens", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        let said = refused.to_string();
        assert!(
            said.contains("cannot be created with an app_publish grant"),
            "the refusal names the grant: {said}"
        );
        assert!(refused.get("secret").is_none(), "{refused}");
    }
    let (_, listed) = in_session(&fx.cookie, "GET", "/user/tokens", None).await;
    assert!(
        !listed.to_string().contains("app_publish"),
        "nothing was minted: {listed}"
    );

    // Edit: a narrowed token cannot gain one, and keeps what it had.
    let reach = Reach::granted(vec![org_grant(fx.org_id, RoleCeiling::Admin)]);
    let (id, _) = mint(&fx.db, fx.user.id, reach).await;
    let body = json!({ "grants": [whole_org, publish] });
    let uri = format!("/user/tokens/{id}");
    let (status, refused) = in_session(&fx.cookie, "PATCH", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert!(refused.to_string().contains("app_publish"), "{refused}");
    let held = grants_of(&fx.db, id).await;
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0].kind, api_token_grants::KIND_WORKSPACE);
    assert!(!pat_row(&fx.db, id).await.all_access);
}

#[tokio::test]
async fn a_hand_written_grant_alone_reaches_no_org_and_nothing_behind_a_gate() {
    let (fx, app) = staff_with_app().await;
    let other = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    // Staff standing asked for, and no workspace grant for it to ride on.
    let alone = Reach::granted(Vec::new()).with_platform();
    let (id, secret) = mint(&fx.db, fx.user.id, alone).await;
    grant_app_publish(&fx.db, fx.org_id, id, app.id).await;

    // Its own app passes the confinement and is then refused by the staff
    // gates: standing bounded to a token's grant orgs is bounded to none.
    let own = format!("/customer-apps/{}", app.id);
    let refused = status_as(&secret, "GET", &own).await;
    assert!(
        matches!(refused, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "the grant confines; it lifts no gate: {refused}"
    );
    for (method, uri) in [
        ("GET", format!("/customer-apps/{}", other.id)),
        ("GET", "/customer-apps".to_string()),
        ("DELETE", own.clone()),
        ("POST", format!("{own}/secrets")),
        ("POST", format!("{own}/rollback")),
    ] {
        assert_eq!(
            status_as(&secret, method, &uri).await,
            StatusCode::NOT_FOUND,
            "{method} {uri}"
        );
    }
    // And off the surface it reaches nothing: no workspace, no org.
    let workspace = workspace_status(&secret, fx.workspace_id, "GET", "read").await;
    let org = status_as(&secret, "GET", &format!("/orgs/{}", fx.org_id)).await;
    for (what, status) in [("the workspace", workspace), ("the org", org)] {
        assert!(
            matches!(status, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN),
            "{what} answered {status}"
        );
    }
}

#[tokio::test]
async fn beside_a_workspace_grant_it_narrows_a_staff_token_to_the_app_it_names() {
    let (fx, app) = staff_with_app().await;
    let other = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let bound = || Reach::granted(vec![org_grant(fx.org_id, RoleCeiling::Owner)]).with_platform();
    let own = format!("/customer-apps/{}", app.id);
    let sibling = format!("/customer-apps/{}", other.id);
    // The control: how an app of the org answers the staffer's own session.
    let (reached, _) = get_in_session(&fx, &own).await;
    assert_ne!(reached, StatusCode::NOT_FOUND);

    // Without the grant: both apps of the org, and the registry.
    let (_, unconfined) = mint(&fx.db, fx.user.id, bound()).await;
    assert_eq!(status_as(&unconfined, "GET", &own).await, reached);
    assert_eq!(status_as(&unconfined, "GET", &sibling).await, reached);
    assert_eq!(
        status_as(&unconfined, "GET", "/customer-apps").await,
        StatusCode::OK
    );

    // With it: the one app, and nothing else of the surface.
    let (id, confined) = mint(&fx.db, fx.user.id, bound()).await;
    grant_app_publish(&fx.db, fx.org_id, id, app.id).await;
    assert_eq!(status_as(&confined, "GET", &own).await, reached);
    for (method, uri) in [
        ("GET", sibling.clone()),
        ("GET", "/customer-apps".to_string()),
        ("DELETE", own.clone()),
        ("POST", format!("{own}/secrets")),
    ] {
        assert_eq!(
            status_as(&confined, method, &uri).await,
            StatusCode::NOT_FOUND,
            "{method} {uri}"
        );
    }
}
