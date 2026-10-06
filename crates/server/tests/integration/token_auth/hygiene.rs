//! Phase 5 hygiene (API-tokens design §8 Phase 5): the expiry header, the
//! leak endpoint, the 7-day expiry notice and the unused-token sweep. The
//! hard constraint runs through every test: **a legacy key is never revoked,
//! expired or swept** — it may only be told it is expiring.
//!
//! Mail is asserted on the in-process preview outbox
//! (`emails::token_mail::outbox`), never SES: each test sets
//! `OXY_APP_EMAIL_LOCAL_TEST`, and nextest gives it a process of its own.

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use entity::api_tokens;
use entity::org_members::OrgRole;
use oxy_app::emails::token_mail::outbox;
use oxy_app::server::api::user_tokens::hygiene::{expire_unused_tokens, send_expiry_notices};
use oxy_auth::token::credential::source;
use oxy_auth::token::format::generate_personal;
use oxy_auth::token::personal::{self, NewToken};
use oxy_auth::token::store::ensure_legacy_row;
use oxy_authz::RoleCeiling;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, Statement};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::service_accounts::{
    admin_fixture, create_account, in_session, mint_account_token, workspace_status,
};
use super::stack::{Reach, flat_api, join_org, org_grant, workspace_api, workspace_external};
use super::{Fixture, audit_rows, call, endpoint_key, legacy_key, seed_user};

const HEADER: &str = "x-oxy-token-expiration";

fn preview_mail() {
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var("OXY_APP_EMAIL_LOCAL_TEST", "1");
        std::env::set_var("OXY_API_URL", "https://oxy.example");
    }
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .expect("raw sql");
}

/// Make the token look `days` old.
async fn age(db: &DatabaseConnection, token_id: Uuid, days: i64) {
    let at = (Utc::now() - Duration::days(days)).fixed_offset();
    exec(
        db,
        "UPDATE api_tokens SET created_at = $2 WHERE id = $1",
        vec![token_id.into(), at.into()],
    )
    .await;
}

async fn row(db: &DatabaseConnection, id: Uuid) -> api_tokens::Model {
    api_tokens::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .expect("token row")
}

/// The hygiene columns the entity does not declare.
async fn hygiene_of(db: &DatabaseConnection, id: Uuid) -> (bool, Option<String>) {
    let found = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT expiry_notified_at IS NOT NULL AS notified, expired_reason FROM api_tokens WHERE id = $1",
            [id.into()],
        ))
        .await
        .unwrap()
        .expect("token row");
    (
        found.try_get("", "notified").unwrap(),
        found.try_get("", "expired_reason").unwrap(),
    )
}

async fn mint_expiring(fx: &Fixture, reach: Reach, days: Option<i64>) -> (Uuid, String) {
    let minted = personal::create(
        &fx.db,
        NewToken {
            user_id: fx.user.id,
            name: "hygiene".into(),
            all_access: reach.all_access,
            platform: reach.platform,
            partner: reach.partner,
            grants: reach.grants,
            expires_at: days.map(|d| Utc::now() + Duration::days(d)),
            source: source::UI,
        },
    )
    .await
    .expect("mint a personal token");
    (minted.row.id, minted.secret)
}

/// `GET /{workspace}/read` on `router`; the status and the response headers.
async fn read_headers(
    router: axum::Router,
    ws: Uuid,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap) {
    let mut req = Request::builder().method("GET").uri(format!("/{ws}/read"));
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    let resp = router
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    (resp.status(), resp.headers().clone())
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

async fn report(body: Value, client: Option<&str>) -> (StatusCode, Value) {
    let headers: Vec<(&str, &str)> = client.map(|c| ("x-forwarded-for", c)).into_iter().collect();
    call(
        flat_api(),
        "POST",
        "/auth/tokens/revoke-leaked",
        &headers,
        Some(body),
    )
    .await
}

async fn legacy_reads(fx: &Fixture, key: &str) -> StatusCode {
    call(
        workspace_api(),
        "GET",
        &format!("/{}/read", fx.workspace_id),
        &[("x-api-key", key)],
        None,
    )
    .await
    .0
}

// ── X-Oxy-Token-Expiration ───────────────────────────────────────────────────

#[tokio::test]
async fn every_token_response_says_when_the_token_expires() {
    let fx = admin_fixture().await;
    let ws = fx.workspace_id;
    let (id, secret) = mint_expiring(&fx, Reach::all_access(), Some(30)).await;
    let expires = rfc3339(row(&fx.db, id).await.expires_at.unwrap().into());
    let bearer = format!("Bearer {secret}");

    let (status, headers) = read_headers(workspace_api(), ws, &[("authorization", &bearer)]).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(headers[HEADER], expires.as_str());
    let (_, headers) = read_headers(workspace_external(), ws, &[("x-api-key", &secret)]).await;
    assert_eq!(
        headers[HEADER],
        expires.as_str(),
        "the same on /external/api"
    );

    // A legacy key too: the header informs and changes nothing.
    let (key_id, key) = legacy_key(&fx, Some(Utc::now() + Duration::days(7))).await;
    let stored = super::key_row(&fx.db, key_id).await.expires_at.unwrap();
    let (status, headers) = read_headers(workspace_api(), ws, &[("x-api-key", &key)]).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(headers[HEADER], rfc3339(stored.into()).as_str());

    // Nothing to say for a token that never expires, or a session.
    let (_, forever) = mint_expiring(&fx, Reach::all_access(), None).await;
    let bearer = format!("Bearer {forever}");
    let (_, headers) = read_headers(workspace_api(), ws, &[("authorization", &bearer)]).await;
    assert!(headers.get(HEADER).is_none());
    let (_, headers) = read_headers(workspace_api(), ws, &[("cookie", &fx.cookie)]).await;
    assert!(headers.get(HEADER).is_none());
}

// ── revoke-leaked ────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_leaked_token_is_revoked_once_audited_and_its_owner_told() {
    preview_mail();
    let fx = admin_fixture().await;
    let (id, secret) = mint_expiring(&fx, Reach::all_access(), Some(30)).await;
    let prefix = row(&fx.db, id).await.display_prefix;
    let body = json!([{ "token": secret, "source": "github", "url": "https://github.com/acme/app/blob/main/.env" }]);

    let (status, answer) = report(body.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer,
        json!([{ "display_prefix": prefix, "status": "revoked" }])
    );
    assert_eq!(
        workspace_status(&secret, fx.workspace_id, "GET", "read").await,
        StatusCode::UNAUTHORIZED
    );
    let revoked = row(&fx.db, id).await;
    assert!(revoked.revoked_at.is_some());
    assert_eq!(revoked.revoke_reason.as_deref(), Some("leaked"));
    assert_eq!(revoked.revoked_by, None, "no person revoked it");

    let audit = audit_rows(&fx.db, "token.revoked").await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].actor_type, "system");
    assert_eq!(audit[0].org_id, Some(fx.org_id));
    assert_eq!(audit[0].metadata["reason"], "leaked");
    assert_eq!(audit[0].metadata["leak_source"], "github");
    assert_eq!(
        audit[0].metadata["leak_url"],
        "https://github.com/acme/app/blob/main/.env"
    );
    assert!(
        !audit[0].metadata.to_string().contains(&secret),
        "never the token"
    );

    let mail = outbox();
    assert_eq!(mail.len(), 1);
    assert_eq!(Some(mail[0].to.clone()), fx.user.email);
    assert_eq!(
        mail[0].subject,
        "An API token was revoked because it was exposed"
    );
    assert!(
        mail[0]
            .text_body
            .contains("https://github.com/acme/app/blob/main/.env")
    );
    assert!(!mail[0].text_body.contains(&secret));

    // Reported again: nothing changes, nobody is mailed twice.
    let (_, answer) = report(body, None).await;
    assert_eq!(answer[0]["status"], "already_revoked");
    assert_eq!(audit_rows(&fx.db, "token.revoked").await.len(), 1);
    assert_eq!(outbox().len(), 1);
}

#[tokio::test]
async fn a_lookalike_an_unknown_token_and_a_legacy_key_are_left_alone() {
    preview_mail();
    let fx = admin_fixture().await;
    let (key_id, key) = legacy_key(&fx, None).await;
    let (pat_id, pat) = endpoint_key(&fx, None).await;
    let (_, live) = mint_expiring(&fx, Reach::all_access(), Some(30)).await;
    let forged = format!(
        "{}{}",
        &live[..live.len() - 1],
        if live.ends_with('a') { 'b' } else { 'a' }
    );
    let stranger = generate_personal().plaintext;

    let body = json!([
        { "token": forged },
        { "token": stranger },
        { "token": key, "source": "github" },
        { "token": pat },
    ]);
    let (status, answer) = report(body, None).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let statuses: Vec<&str> = answer
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["status"].as_str().unwrap())
        .collect();
    assert_eq!(
        statuses,
        ["unknown", "unknown", "ignored_legacy", "ignored_legacy"]
    );

    // Nothing was revoked, audited or mailed; every key still works.
    assert!(audit_rows(&fx.db, "token.revoked").await.is_empty());
    assert!(outbox().is_empty());
    assert_eq!(legacy_reads(&fx, &key).await, StatusCode::NO_CONTENT);
    assert_eq!(legacy_reads(&fx, &pat).await, StatusCode::NO_CONTENT);
    assert_eq!(
        workspace_status(&live, fx.workspace_id, "GET", "read").await,
        StatusCode::NO_CONTENT
    );
    for id in [key_id, pat_id] {
        let mirror = super::token_row(&fx.db, id).await.expect("mirror");
        assert!(mirror.revoked_at.is_none());
    }

    // At most a hundred per report.
    let many: Vec<Value> = (0..101).map(|_| json!({ "token": "x" })).collect();
    assert_eq!(report(json!(many), None).await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_service_account_tokens_leak_is_mailed_to_the_orgs_admins() {
    preview_mail();
    let fx = admin_fixture().await;
    let admin = seed_user(&fx.db, "admin").await;
    join_org(&fx.db, fx.org_id, admin.id, OrgRole::Admin).await;
    let member = seed_user(&fx.db, "member").await;
    join_org(&fx.db, fx.org_id, member.id, OrgRole::Member).await;
    let sa = create_account(&fx, "deploy-bot", "member").await;
    let (_, secret) =
        mint_account_token(&fx, sa, json!({ "name": "ci", "expires_in_days": 30 })).await;

    let (_, answer) = report(json!([{ "token": secret }]), None).await;
    assert_eq!(answer[0]["status"], "revoked");
    let mut to: Vec<String> = outbox().into_iter().map(|m| m.to).collect();
    to.sort();
    let mut officers = vec![fx.user.email.clone().unwrap(), admin.email.clone().unwrap()];
    officers.sort();
    assert_eq!(
        to, officers,
        "the owner and the admin, never a plain member"
    );
    assert!(outbox()[0].text_body.contains("service account deploy-bot"));
    assert!(
        outbox()[0]
            .text_body
            .contains("settings=organization.api_access")
    );
}

#[tokio::test]
async fn the_public_token_routes_are_rate_limited_per_client() {
    let _fx = admin_fixture().await;
    let api = flat_api();
    let post = |uri: &'static str, client: &'static str, body: Value| {
        let api = api.clone();
        async move { call(api, "POST", uri, &[("x-forwarded-for", client)], Some(body)).await }
    };
    // The bucket refills at one a second, so a slow loop could slip a request
    // through at the edge: the burst of sixty passes, and a refusal follows
    // well before the loop could refill it.
    for route in ["/auth/tokens/revoke-leaked", "/auth/oidc/exchange"] {
        let body = if route.ends_with("revoke-leaked") {
            json!([])
        } else {
            json!({})
        };
        for i in 0..60 {
            let (status, _) = post(route, "198.51.100.7", body.clone()).await;
            assert_ne!(status, StatusCode::TOO_MANY_REQUESTS, "{route} request {i}");
        }
        let mut refused = None;
        for _ in 0..60 {
            let (status, answer) = post(route, "198.51.100.7", body.clone()).await;
            if status == StatusCode::TOO_MANY_REQUESTS {
                refused = Some(answer);
                break;
            }
        }
        let refused = refused.unwrap_or_else(|| panic!("{route} never refused"));
        assert_eq!(refused["code"], "rate_limited");
        let (status, _) = post(route, "198.51.100.8", body).await;
        assert_ne!(
            status,
            StatusCode::TOO_MANY_REQUESTS,
            "another client has its own budget"
        );
    }
}

// ── The expiry notice ────────────────────────────────────────────────────────

#[tokio::test]
async fn the_expiry_notice_goes_out_once_a_week_ahead_and_again_after_extend() {
    preview_mail();
    let fx = admin_fixture().await;
    let grant = vec![org_grant(fx.org_id, RoleCeiling::Owner)];
    let (due_id, _) = mint_expiring(&fx, Reach::granted(grant), Some(5)).await;
    age(&fx.db, due_id, 30).await;
    let (later_id, _) = mint_expiring(&fx, Reach::all_access(), Some(20)).await;
    age(&fx.db, later_id, 30).await;
    let (forever_id, _) = mint_expiring(&fx, Reach::all_access(), None).await;
    age(&fx.db, forever_id, 30).await;
    // A legacy key is told too.
    let (key_id, _) = legacy_key(&fx, Some(Utc::now() + Duration::days(3))).await;
    ensure_legacy_row(&fx.db, key_id, source::LEGACY_LAZY)
        .await
        .unwrap();
    age(&fx.db, key_id, 30).await;

    let sent = send_expiry_notices(&fx.db, Utc::now()).await.unwrap();
    assert_eq!(sent.sent, 2, "{sent:?}");
    let mail = outbox();
    assert!(mail.iter().all(|m| Some(m.to.clone()) == fx.user.email));
    // Each is told in its own words and sent to its own page: a token to the
    // token list, a legacy API key to the Legacy API keys section.
    let token_mail = mail
        .iter()
        .find(|m| {
            m.subject
                .starts_with("Your API token \"hygiene\" expires on")
        })
        .expect("the token's notice");
    assert!(
        token_mail
            .text_body
            .contains("https://oxy.example/?settings=account.tokens")
    );
    let key_mail = mail
        .iter()
        .find(|m| {
            m.subject
                .starts_with("Your legacy API key \"legacy\" expires on")
        })
        .expect("the legacy key's notice");
    assert!(
        key_mail
            .text_body
            .contains("https://oxy.example/?settings=workspace.legacy_api_keys")
    );
    assert!(!key_mail.text_body.contains("settings=account.tokens"));
    assert!(hygiene_of(&fx.db, due_id).await.0);
    assert!(!hygiene_of(&fx.db, later_id).await.0);

    // Once per expiry.
    assert_eq!(
        send_expiry_notices(&fx.db, Utc::now()).await.unwrap().sent,
        0
    );
    assert_eq!(outbox().len(), 2);

    // Extend resets it — a personal token through the token route, a legacy
    // key through its own.
    let extend = format!("/user/tokens/{due_id}/extend");
    let (status, extended) =
        in_session(&fx.cookie, "POST", &extend, Some(json!({ "days": 1 }))).await;
    assert_eq!(status, StatusCode::OK, "{extended}");
    let extend = format!("/{}/api-keys/{key_id}/extend", fx.workspace_id);
    let (status, extended) = call(
        super::api_surface(),
        "POST",
        &extend,
        &[("cookie", &fx.cookie)],
        Some(json!({ "days": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{extended}");
    for id in [due_id, key_id] {
        assert!(
            !hygiene_of(&fx.db, id).await.0,
            "a new expiry, a new notice"
        );
    }
    assert_eq!(
        send_expiry_notices(&fx.db, Utc::now()).await.unwrap().sent,
        2
    );
    assert_eq!(outbox().len(), 4);
}

#[tokio::test]
async fn a_service_account_tokens_notice_goes_to_its_orgs_admins() {
    preview_mail();
    let fx = admin_fixture().await;
    let admin = seed_user(&fx.db, "admin").await;
    join_org(&fx.db, fx.org_id, admin.id, OrgRole::Admin).await;
    let member = seed_user(&fx.db, "member").await;
    join_org(&fx.db, fx.org_id, member.id, OrgRole::Member).await;
    let sa = create_account(&fx, "deploy-bot", "member").await;
    let (id, _) =
        mint_account_token(&fx, sa, json!({ "name": "release", "expires_in_days": 5 })).await;
    age(&fx.db, id, 30).await;

    assert_eq!(
        send_expiry_notices(&fx.db, Utc::now()).await.unwrap().sent,
        1
    );
    let mail = outbox();
    let mut to: Vec<String> = mail.iter().map(|m| m.to.clone()).collect();
    to.sort();
    let mut officers = vec![fx.user.email.clone().unwrap(), admin.email.clone().unwrap()];
    officers.sort();
    assert_eq!(to, officers);
    assert!(
        mail[0]
            .subject
            .starts_with("A service-account token in Acme expires on")
    );
    assert!(
        mail[0]
            .text_body
            .contains("https://oxy.example/?settings=organization.api_access")
    );
}

// ── The unused-token sweep ───────────────────────────────────────────────────

#[tokio::test]
async fn a_year_idle_token_expires_and_a_legacy_key_never_does() {
    let fx = admin_fixture().await;
    let (idle_id, idle) = mint_expiring(&fx, Reach::all_access(), None).await;
    age(&fx.db, idle_id, 400).await;
    let (used_id, used) = mint_expiring(&fx, Reach::all_access(), None).await;
    age(&fx.db, used_id, 400).await;
    let ten_days_ago = (Utc::now() - Duration::days(10)).fixed_offset();
    exec(
        &fx.db,
        "UPDATE api_tokens SET last_used_at = $2 WHERE id = $1",
        vec![used_id.into(), ten_days_ago.into()],
    )
    .await;
    let sa = create_account(&fx, "deploy-bot", "member").await;
    let (sa_id, _) = mint_account_token(&fx, sa, json!({ "name": "ci", "expires_at": null })).await;
    age(&fx.db, sa_id, 400).await;
    let (key_id, key) = legacy_key(&fx, None).await;
    ensure_legacy_row(&fx.db, key_id, source::LEGACY_LAZY)
        .await
        .unwrap();
    age(&fx.db, key_id, 400).await;
    let (pat_id, pat) = endpoint_key(&fx, None).await;
    age(&fx.db, pat_id, 400).await;

    assert_eq!(expire_unused_tokens(&fx.db, Utc::now()).await.unwrap(), 2);

    let swept = row(&fx.db, idle_id).await;
    assert!(
        swept.revoked_at.is_none(),
        "expired, never revoked or deleted"
    );
    assert!(
        swept
            .expires_at
            .is_some_and(|at| DateTime::<Utc>::from(at) <= Utc::now())
    );
    assert_eq!(
        hygiene_of(&fx.db, idle_id).await.1.as_deref(),
        Some("unused")
    );
    assert_eq!(hygiene_of(&fx.db, sa_id).await.1.as_deref(), Some("unused"));
    assert_eq!(
        workspace_status(&idle, fx.workspace_id, "GET", "read").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        workspace_status(&used, fx.workspace_id, "GET", "read").await,
        StatusCode::NO_CONTENT
    );
    let audit = audit_rows(&fx.db, "token.expired_unused").await;
    assert_eq!(audit.len(), 2);
    assert!(audit.iter().all(|a| a.actor_type == "system"));

    // Legacy keys are exempt, however idle.
    for id in [key_id, pat_id] {
        let mirror = super::token_row(&fx.db, id).await.expect("mirror");
        assert!(mirror.expires_at.is_none() && mirror.revoked_at.is_none());
    }
    assert_eq!(legacy_reads(&fx, &key).await, StatusCode::NO_CONTENT);
    assert_eq!(legacy_reads(&fx, &pat).await, StatusCode::NO_CONTENT);

    // Idempotent, and an owner's Extend revives a swept token for good.
    assert_eq!(expire_unused_tokens(&fx.db, Utc::now()).await.unwrap(), 0);
    let extend = format!("/user/tokens/{idle_id}/extend");
    let (status, _) = in_session(&fx.cookie, "POST", &extend, Some(json!({ "days": 30 }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hygiene_of(&fx.db, idle_id).await.1, None);
    assert_eq!(expire_unused_tokens(&fx.db, Utc::now()).await.unwrap(), 0);
    assert_eq!(
        workspace_status(&idle, fx.workspace_id, "GET", "read").await,
        StatusCode::NO_CONTENT
    );
}
