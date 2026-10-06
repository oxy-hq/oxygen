//! `POST /api/admin/app-publish-tokens`: who may mint a staff publish token
//! (`oxypublish_`, never expiring, naming no app), and what the audit log says
//! of a mint and a revoke.
//!
//! A browser session and a legacy API key mint as they always have. No
//! new-format token does: one that could would turn its own hours or days into
//! a credential that outlives it. Each kind is driven through the served flat
//! tree, so the test sees whichever gate answers it first.

use axum::http::StatusCode;
use entity::org_members::OrgRole;
use oxy_app::server::api::admin::app_publish_tokens::{MINTED, REVOKED};
use oxy_authz::RoleCeiling;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use super::oidc::{Run, exchange_as, policy_body, register_policy, trust_test_keys, whole_org};
use super::sandbox_agent::{another_session, minted as sandbox_agent_token, staff_with_app};
use super::service_accounts::{create_account, mint_account_token};
use super::stack::{Reach, flat_api, join_org, mint, org_grant};
use super::{Fixture, audit_rows, call, endpoint_key, legacy_key};

const TOKENS: &str = "/admin/app-publish-tokens";

async fn mint_with(headers: &[(&str, &str)]) -> (StatusCode, Value) {
    let body = json!({ "name": "ci-publish" });
    call(flat_api(), "POST", TOKENS, headers, Some(body)).await
}

async fn mint_as(secret: &str) -> (StatusCode, Value) {
    let bearer = format!("Bearer {secret}");
    mint_with(&[("authorization", &bearer)]).await
}

async fn publish_tokens(db: &DatabaseConnection) -> usize {
    entity::app_publish_tokens::Entity::find()
        .all(db)
        .await
        .expect("read publish tokens")
        .len()
}

fn id_of(body: &Value) -> Uuid {
    Uuid::parse_str(body["id"].as_str().expect("an id")).expect("a uuid")
}

#[tokio::test]
async fn a_session_mints_and_the_mint_is_audited_without_the_secret() {
    let (fx, _app) = staff_with_app().await;

    let (status, body) = mint_with(&[("cookie", &fx.cookie)]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let secret = body["token"].as_str().expect("the plaintext, once");
    assert!(secret.starts_with("oxypublish_"), "{body}");
    assert_eq!(body["name"], "ci-publish");
    assert_eq!(publish_tokens(&fx.db).await, 1);

    let rows = audit_rows(&fx.db, MINTED).await;
    assert_eq!(rows.len(), 1, "one row for one mint");
    let row = &rows[0];
    assert_eq!(row.actor_type, "user");
    assert_eq!(row.actor_user_id, Some(fx.user.id));
    assert_eq!(row.org_id, None, "a platform-level event");
    assert_eq!(row.target_type.as_deref(), Some("app_publish_token"));
    assert_eq!(row.target_id, Some(id_of(&body).to_string()));
    assert_eq!(row.target_label.as_deref(), Some("ci-publish"));
    assert_eq!(row.metadata["token_prefix"], body["token_prefix"]);
    assert_eq!(row.metadata["minted_by"], json!(fx.user.id));
    assert!(row.metadata.get("token_id").is_none(), "no key acted");
    assert!(
        !format!("{row:?}").contains(secret),
        "the audit row must never carry the token"
    );
}

#[tokio::test]
async fn a_legacy_key_still_mints_and_the_row_names_that_key() {
    let (fx, _app) = staff_with_app().await;
    // One as every release before this one wrote it, one the legacy endpoint
    // mints today: CI scripts hold both.
    let seeded = legacy_key(&fx, None).await;
    let endpoint = endpoint_key(&fx, None).await;

    for (what, (key_id, key)) in [("seeded", seeded), ("endpoint-minted", endpoint)] {
        let (status, body) = mint_with(&[("x-api-key", &key)]).await;
        assert_eq!(status, StatusCode::OK, "a {what} legacy key: {body}");
        assert!(body["token"].as_str().unwrap().starts_with("oxypublish_"));

        let rows = audit_rows(&fx.db, MINTED).await;
        let row = rows.last().expect("the mint's row");
        assert_eq!(row.target_id, Some(id_of(&body).to_string()), "{what}");
        assert_eq!(row.actor_type, "api_key", "{what}");
        assert_eq!(row.metadata["token_id"], json!(key_id), "{what}");
        assert_eq!(row.metadata["token_kind"], "legacy_key", "{what}");
        assert_eq!(row.metadata["token_prefix"], body["token_prefix"], "{what}");
    }
    assert_eq!(publish_tokens(&fx.db).await, 2);
    assert_eq!(audit_rows(&fx.db, MINTED).await.len(), 2);
}

#[tokio::test]
async fn the_staff_audit_console_shows_the_key_the_address_and_the_client_of_a_mint() {
    let (fx, _app) = staff_with_app().await;
    let (key_id, key) = legacy_key(&fx, None).await;
    let headers = [
        ("x-api-key", key.as_str()),
        ("user-agent", "oxyc/9.9.9 agent/test"),
        // The last hop is the one the load balancer wrote.
        ("x-forwarded-for", "10.0.0.1, 198.51.100.4"),
    ];
    let (status, minted) = mint_with(&headers).await;
    assert_eq!(status, StatusCode::OK, "{minted}");

    let uri = format!("/admin/audit?action={MINTED}");
    let (status, rows) = call(flat_api(), "GET", &uri, &[("cookie", &fx.cookie)], None).await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    let rows = rows.as_array().expect("a bare array, as before");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["token_id"], json!(key_id));
    assert_eq!(row["token_name"], "legacy");
    assert_eq!(row["token_kind"], "legacy_key");
    assert!(row["token_prefix"].as_str().is_some_and(|p| !p.is_empty()));
    assert_eq!(row["user_agent"], "oxyc/9.9.9 agent/test");
    assert_eq!(row["ip"], "198.51.100.4");
    // The fields the console already read are unchanged.
    assert_eq!(row["action"], MINTED);
    assert_eq!(row["actor_type"], "api_key");
    assert_eq!(row["target_id"], minted["id"]);
    assert!(!row.to_string().contains(&key), "never the key itself");
}

#[tokio::test]
async fn a_personal_token_is_refused_the_mint_as_token_management_refuses_it() {
    let (fx, _app) = staff_with_app().await;
    // Both carry the staff standing the route's own gate asks for, so the
    // answer is the mint's.
    let all_access = Reach::all_access().with_platform();
    let mut bound = Reach::granted(vec![org_grant(fx.org_id, RoleCeiling::Owner)]);
    bound.platform = true;

    for (what, reach) in [("all-access", all_access), ("grant-bound", bound)] {
        let (_, secret) = mint(&fx.db, fx.user.id, reach).await;
        let (status, body) = mint_as(&secret).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{what}: {body}");
        assert_eq!(body["code"], "session_required", "{what}: {body}");
        assert!(body.get("token").is_none(), "{what}: no token on a refusal");
    }
    assert_eq!(publish_tokens(&fx.db).await, 0);
    assert!(audit_rows(&fx.db, MINTED).await.is_empty());
}

#[tokio::test]
async fn no_other_new_format_token_mints_either() {
    trust_test_keys();
    let (fx, app) = staff_with_app().await;
    let (_, sandbox_agent) = sandbox_agent_token(&fx, &[app.id]).await;

    // The org's own owner — not staff — runs its service account.
    let (owner, cookie) = another_session(&fx.db, "org-owner").await;
    join_org(&fx.db, fx.org_id, owner.id, OrgRole::Owner).await;
    let org = Fixture {
        db: fx.db.clone(),
        user: owner,
        org_id: fx.org_id,
        workspace_id: fx.workspace_id,
        cookie,
    };
    let account = create_account(&org, "publisher", "member").await;
    let (_, service_account) = mint_account_token(&org, account, json!({ "name": "t" })).await;
    register_policy(&org, account, policy_body(json!([whole_org("member")]))).await;
    let (status, exchanged) = exchange_as(&org, "publisher", &Run::new()).await;
    assert_eq!(status, StatusCode::OK, "{exchanged}");
    let ci = exchanged["token"].as_str().expect("a ci token").to_string();

    // Each is stopped before the mint: the sandbox agent token by its route
    // allow-list, the account's two by the staff gate no account passes.
    for (what, secret) in [
        ("sandbox_agent", sandbox_agent),
        ("service_account", service_account),
        ("ci", ci),
    ] {
        let (status, body) = mint_as(&secret).await;
        assert!(
            matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
            "{what} answered {status}: {body}"
        );
        assert!(body.get("token").is_none(), "{what}: {body}");
    }
    assert_eq!(publish_tokens(&fx.db).await, 0);
    assert!(audit_rows(&fx.db, MINTED).await.is_empty());
}

#[tokio::test]
async fn a_revoke_is_audited_once_and_names_the_token_that_revoked() {
    let (fx, _app) = staff_with_app().await;
    let (_, by_session) = mint_with(&[("cookie", &fx.cookie)]).await;
    let (_, by_token) = mint_with(&[("cookie", &fx.cookie)]).await;
    let revoke = |body: &Value| format!("{TOKENS}/{}/revoke", id_of(body));

    // In a session; the second, idempotent revoke changes nothing to record.
    for _ in 0..2 {
        let headers = [("cookie", fx.cookie.as_str())];
        let (status, body) = call(flat_api(), "POST", &revoke(&by_session), &headers, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["revoked"], true);
    }
    // Revoking only takes power away, so a token may still do it — on the record.
    let (pat_id, pat) = mint(&fx.db, fx.user.id, Reach::all_access().with_platform()).await;
    let bearer = format!("Bearer {pat}");
    let headers = [("authorization", bearer.as_str())];
    let (status, body) = call(flat_api(), "POST", &revoke(&by_token), &headers, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let rows = audit_rows(&fx.db, REVOKED).await;
    assert_eq!(rows.len(), 2, "one row per token revoked");
    assert_eq!(rows[0].actor_type, "user");
    assert_eq!(rows[0].target_id, Some(id_of(&by_session).to_string()));
    assert_eq!(rows[0].metadata["minted_by"], json!(fx.user.id));
    assert_eq!(rows[1].actor_type, "api_key");
    assert_eq!(rows[1].target_id, Some(id_of(&by_token).to_string()));
    assert_eq!(rows[1].metadata["token_id"], json!(pat_id));
    assert_eq!(rows[1].metadata["token_kind"], "personal");
}
