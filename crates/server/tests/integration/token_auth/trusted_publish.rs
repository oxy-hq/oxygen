//! Phase 4, around the edges of trusted access:
//!
//! - **the legacy trusted-publishing exchange still works end to end** — the
//!   same route, audience, table and `oxypublish_` token as before, now through
//!   the shared verifier;
//! - a token holding an **`app_publish` grant** is confined, on the custom-apps
//!   surface, to its app, and reaches nothing else;
//! - the **sweep** removes spent `jti`s and long-expired `ci` tokens and
//!   nothing else;
//! - the **legacy-key regression** (design §3.5): a key that existed before any
//!   of this keeps its whole reach through all of it.

use axum::http::StatusCode;
use chrono::{Duration, Utc};
use entity::{api_tokens, app_publish_tokens, app_publishers, oidc_used_jti};
use oxy_auth::github_oidc::jti;
use oxy_auth::token::ci;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::legacy_reach::{assert_full_reach, world};
use super::oidc::{
    OWNER_ID, Run, exchange, exchange_as, exchange_ok, policies_uri, policy_body, register_policy,
    trust_test_keys, whole_org,
};
use super::service_accounts::{
    admin_fixture, create_account, in_session, with_token, workspace_status,
};
use super::stack::{flat_api, published_app};
use super::{Fixture, call, endpoint_key, legacy_key, minted_pat};

const LEGACY_EXCHANGE: &str = "/customer-apps/publish/oidc-exchange";

/// Register `acme/app`'s publish workflow as a trusted publisher of `app_id`,
/// the way the staff route writes the row.
async fn register_publisher(db: &DatabaseConnection, app_id: Uuid) {
    app_publishers::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(app_id),
        repo_owner: ActiveValue::Set("acme".into()),
        repo_owner_id: ActiveValue::Set(OWNER_ID),
        repo_name: ActiveValue::Set("app".into()),
        workflow_ref: ActiveValue::Set(".github/workflows/release.yml".into()),
        environment: ActiveValue::Set("production".into()),
        created_by: ActiveValue::Set(None),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
    }
    .insert(db)
    .await
    .expect("register the trusted publisher");
}

async fn legacy_exchange(jwt: &str, app: &str) -> (StatusCode, Value) {
    let bearer = format!("Bearer {jwt}");
    let headers = [("authorization", bearer.as_str())];
    call(
        flat_api(),
        "POST",
        LEGACY_EXCHANGE,
        &headers,
        Some(json!({ "app": app })),
    )
    .await
}

async fn status_as(secret: &str, method: &str, uri: &str) -> StatusCode {
    with_token(secret, method, uri, None).await.0
}

#[tokio::test]
async fn the_legacy_publish_exchange_still_mints_an_app_scoped_publish_token() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    register_publisher(&fx.db, app.id).await;
    let named = format!("acme-{}/{}", fx.org_id.simple(), app.slug);

    // Audience `oxy-publish`, body `{app}`, the JWT as the bearer — unchanged.
    let run = Run::new().with("jti", json!("legacy-once"));
    let (status, minted) = legacy_exchange(&run.jwt("oxy-publish"), &named).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    let secret = minted["token"].as_str().expect("a token").to_string();
    assert!(secret.starts_with("oxypublish_"), "{secret}");
    assert_eq!(minted["app_id"], app.id.to_string());
    assert!(minted["expires_at"].is_string());

    // It is an `app_publish_tokens` row, exactly as before — not an API token.
    let rows = app_publish_tokens::Entity::find()
        .all(&fx.db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].app_id, Some(app.id));
    assert_eq!(rows[0].created_by, None, "the machine principal");
    assert_eq!(
        rows[0].name,
        "github-oidc:acme/app/.github/workflows/release.yml@refs/heads/main env=production"
    );
    assert_eq!(api_tokens::Entity::find().count(&fx.db).await.unwrap(), 0);

    // The token authenticates on its own path and is confined as it always
    // was: the upload is reachable, the rest of the API is not.
    let upload = status_as(&secret, "POST", "/customer-apps/publish").await;
    assert!(
        !matches!(
            upload,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        ),
        "the upload must reach its handler, not be refused by auth or scope: {upload}"
    );
    for uri in ["/orgs", "/customer-apps", "/customer-apps/fleet-health"] {
        assert_eq!(
            status_as(&secret, "GET", uri).await,
            StatusCode::NOT_FOUND,
            "an app-scoped publish token reaches its app and the upload only: {uri}"
        );
    }

    // The same JWT again is a replay.
    let (status, _) = legacy_exchange(&run.jwt("oxy-publish"), &named).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Its audience is its own: a trusted-access token is refused here...
    let (status, _) = legacy_exchange(&Run::new().jwt("oxy"), &named).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // ...and a publish token is refused there, with nothing minted.
    let (status, refusal) = super::oidc::exchange_with(json!({
        "token": Run::new().jwt("oxy-publish"),
        "service_account": Uuid::new_v4(),
    }))
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(refusal["code"], "wrong_audience");

    // A workflow that is not the registered one is refused, as before.
    let other = Run::new().with(
        "job_workflow_ref",
        json!("acme/app/.github/workflows/ci.yml@refs/heads/main"),
    );
    let (status, _) = legacy_exchange(&other.jwt("oxy-publish"), &named).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        app_publish_tokens::Entity::find()
            .count(&fx.db)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn an_app_publish_grant_reaches_its_app_on_the_publish_surface_and_nothing_else() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let other = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let sa = create_account(&fx, "publisher", "member").await;
    let grants = json!([{ "kind": "app_publish", "app_id": app.id }]);
    register_policy(&fx, sa, policy_body(grants)).await;

    let (status, minted) = exchange_as(&fx, "publisher", &Run::new()).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    let secret = minted["token"].as_str().unwrap().to_string();
    assert_eq!(minted["grants"][0]["kind"], "app_publish");
    assert_eq!(minted["grants"][0]["app_id"], app.id.to_string());

    // The upload reaches its handler: the grant is matched against the app
    // the body names, in `publish()`.
    let upload = status_as(&secret, "POST", "/customer-apps/publish").await;
    assert!(
        !matches!(upload, StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND),
        "the upload must reach its handler: {upload}"
    );
    // Its own app's routes pass the confinement. What answers next is the
    // route's own platform gate, which a service account never passes — the
    // grant confines, and lifts no gate.
    let own = format!("/customer-apps/{}", app.id);
    assert_eq!(status_as(&secret, "GET", &own).await, StatusCode::FORBIDDEN);

    // Everything else on the surface is 404: another app, the registry, the
    // fleet-wide rollups, and every write that is not a publish.
    for (method, uri) in [
        ("GET", format!("/customer-apps/{}", other.id)),
        ("POST", format!("/customer-apps/{}/publish", other.id)),
        ("GET", "/customer-apps".to_string()),
        ("GET", "/customer-apps/fleet-health".to_string()),
        ("GET", "/customer-apps/storage".to_string()),
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

    // And off it, the token reaches nothing: no workspace, no org, no admin.
    assert_eq!(
        workspace_status(&secret, fx.workspace_id, "GET", "read").await,
        StatusCode::NOT_FOUND
    );
    for uri in [
        format!("/orgs/{}", fx.org_id),
        format!("/orgs/{}/service-accounts", fx.org_id),
        "/admin/apps".to_string(),
        "/chat/threads".to_string(),
    ] {
        let status = status_as(&secret, "GET", &uri).await;
        assert!(
            matches!(status, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN),
            "GET {uri} answered {status}"
        );
    }
}

async fn spent(db: &DatabaseConnection, value: &str, expires_in: Duration) {
    oidc_used_jti::ActiveModel {
        jti: ActiveValue::Set(value.to_string()),
        expires_at: ActiveValue::Set((Utc::now() + expires_in).fixed_offset()),
    }
    .insert(db)
    .await
    .expect("seed a spent jti");
}

async fn expire(db: &DatabaseConnection, id: Uuid, ago: Duration) {
    let row = api_tokens::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .expect("the token row");
    let mut active: api_tokens::ActiveModel = row.into();
    active.expires_at = ActiveValue::Set(Some((Utc::now() - ago).fixed_offset()));
    active.update(db).await.expect("backdate the expiry");
}

async fn exists(db: &DatabaseConnection, id: Uuid) -> bool {
    api_tokens::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .is_some()
}

#[tokio::test]
async fn the_sweep_removes_spent_jtis_and_long_expired_ci_tokens_and_nothing_else() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let sa = create_account(&fx, "deployer", "member").await;
    register_policy(&fx, sa, policy_body(json!([whole_org("member")]))).await;
    let (long_gone, _) = exchange_ok(&fx, &Run::new()).await;
    let (recent, _) = exchange_ok(&fx, &Run::new()).await;
    let (live, _) = exchange_ok(&fx, &Run::new()).await;
    expire(&fx.db, long_gone, Duration::days(31)).await;
    expire(&fx.db, recent, Duration::days(29)).await;
    // Tokens of every other kind, expired just as long ago. A key an older pod
    // minted lives in `api_keys` alone until backfill or first use mirrors it
    // into `api_tokens` under the same id; mirror it, so the sweep sees a row.
    let (key, _) = legacy_key(&fx, None).await;
    oxy_auth::token::store::ensure_legacy_row(
        &fx.db,
        key,
        oxy_auth::token::credential::source::LEGACY_LAZY,
    )
    .await
    .expect("mirror the legacy key");
    let (pat, _) = minted_pat(&fx, None).await;
    let (sat, _) =
        super::service_accounts::mint_account_token(&fx, sa, json!({ "name": "t" })).await;
    for id in [key, pat, sat] {
        expire(&fx.db, id, Duration::days(400)).await;
    }

    let before = oidc_used_jti::Entity::find().count(&fx.db).await.unwrap();
    spent(&fx.db, "stale", Duration::minutes(-5)).await;
    spent(&fx.db, "fresh", Duration::minutes(5)).await;

    let now = Utc::now();
    assert_eq!(jti::sweep_expired(&fx.db, now).await.unwrap(), 1);
    assert_eq!(ci::delete_expired(&fx.db, now).await.unwrap(), 1);
    // Idempotent: a second pass finds nothing.
    assert_eq!(jti::sweep_expired(&fx.db, now).await.unwrap(), 0);
    assert_eq!(ci::delete_expired(&fx.db, now).await.unwrap(), 0);

    // The fresh jti — and the three the exchanges above spent — are kept.
    let kept = oidc_used_jti::Entity::find()
        .filter(oidc_used_jti::Column::Jti.eq("fresh"))
        .count(&fx.db)
        .await
        .unwrap();
    assert_eq!(kept, 1);
    assert_eq!(
        oidc_used_jti::Entity::find().count(&fx.db).await.unwrap(),
        before + 1
    );
    // Only the ci token more than 30 days past expiry is gone.
    assert!(!exists(&fx.db, long_gone).await);
    for (what, id) in [
        ("a ci token 29 days past expiry", recent),
        ("a live ci token", live),
        ("a legacy key", key),
        ("a personal token", pat),
        ("a service-account token", sat),
    ] {
        assert!(exists(&fx.db, id).await, "{what} must never be swept");
    }
}

#[tokio::test]
async fn a_legacy_key_is_unaffected_by_trust_policies_exchanges_and_sweeps() {
    trust_test_keys();
    // A staff member who owns two orgs, holding keys minted before any of it.
    let w = world().await;
    let fx: &Fixture = &w.fx;
    let (legacy, key) = legacy_key(fx, None).await;
    let (minted, pat) = endpoint_key(fx, None).await;
    assert_full_reach(&w, "an oxy_<hex> key, before", &key).await;
    assert_full_reach(&w, "a legacy-endpoint token, before", &pat).await;

    // The org gains an account, policies and ci tokens; a policy is disabled
    // and deleted; tokens are swept; an exchange is refused.
    let sa = create_account(fx, "deployer", "admin").await;
    let policy = register_policy(fx, sa, policy_body(json!([whole_org("admin")]))).await;
    let (_, ci_secret) = exchange_ok(fx, &Run::new()).await;
    // An admin-ceiling ci token of the key's own org cannot touch the key.
    assert_eq!(
        status_as(&ci_secret, "DELETE", &format!("/user/tokens/{legacy}")).await,
        StatusCode::FORBIDDEN
    );
    let one = format!("{}/{policy}", policies_uri(fx.org_id, sa));
    in_session(&fx.cookie, "PATCH", &one, Some(json!({ "disabled": true }))).await;
    assert_eq!(exchange(fx, &Run::new()).await.0, StatusCode::FORBIDDEN);
    in_session(&fx.cookie, "DELETE", &one, None).await;
    let now = Utc::now() + Duration::days(365);
    jti::sweep_expired(&fx.db, now).await.unwrap();
    ci::delete_expired(&fx.db, now).await.unwrap();

    // A legacy key cannot be traded for anything at the exchange.
    let body = json!({ "token": key, "service_account": sa });
    let (status, _) = super::oidc::exchange_with(body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Nothing narrowed, expired, revoked or deleted them.
    assert_full_reach(&w, "an oxy_<hex> key, after", &key).await;
    assert_full_reach(&w, "a legacy-endpoint token, after", &pat).await;
    for id in [legacy, minted] {
        let row = api_tokens::Entity::find_by_id(id)
            .one(&fx.db)
            .await
            .unwrap()
            .expect("the key's row survives the sweep");
        assert!(row.all_access && row.platform && row.partner);
        assert!(row.revoked_at.is_none());
        assert_eq!(row.expires_at, None);
        assert_eq!(row.trust_policy_id, None);
    }
    // A legacy key reads the policy routes as its owner would, and may not
    // write them — as no key or token may.
    let uri = policies_uri(fx.org_id, sa);
    for secret in [&key, &pat] {
        assert_eq!(status_as(secret, "GET", &uri).await, StatusCode::OK);
        let body = policy_body(json!([whole_org("admin")]));
        let (status, refused) = with_token(secret, "POST", &uri, Some(body)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(refused["code"], "session_required");
    }
}
