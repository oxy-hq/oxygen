//! Phase 5: the org token policy (API-tokens design §5). An org caps how long
//! a token reaching it may live, may refuse all-access tokens, and decides
//! whether trust policies must name an environment. A violating token is
//! **inert in that org, never revoked** — and a legacy key is never judged.

use axum::http::StatusCode;
use chrono::{Duration, Utc};
use entity::api_tokens;
use entity::org_members::OrgRole;
use oxy_auth::token::credential::source;
use oxy_auth::token::personal::{self, NewToken};
use oxy_authz::RoleCeiling;
use sea_orm::EntityTrait;
use serde_json::{Value, json};
use uuid::Uuid;

use super::oidc::{
    Run, exchange, exchange_ok, policies_uri, policy_body, register_policy, trust_test_keys,
    whole_org,
};
use super::service_accounts::{
    admin_fixture, create_account, in_session, mint_account_token, session_of, with_token,
    workspace_status,
};
use super::stack::{Reach, join_org, mint, org_grant};
use super::{
    Fixture, audit_rows, endpoint_key, legacy_key, seed_org, seed_user, seed_workspace_in,
};

fn policy_uri(org: Uuid) -> String {
    format!("/orgs/{org}/token-policy")
}

fn policy(max_lifetime_days: Option<i64>, all_access: bool, environment: bool) -> Value {
    json!({
        "max_lifetime_days": max_lifetime_days,
        "allow_all_access_tokens": all_access,
        "require_environment_on_trust_policies": environment,
    })
}

async fn put_policy(fx: &Fixture, org: Uuid, body: Value) {
    let (status, saved) = in_session(&fx.cookie, "PUT", &policy_uri(org), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "put policy: {saved}");
}

/// A personal token of the fixture's user, expiring at `days` from now.
async fn mint_expiring(fx: &Fixture, reach: Reach, days: Option<i64>) -> (Uuid, String) {
    let minted = personal::create(
        &fx.db,
        NewToken {
            user_id: fx.user.id,
            name: "policy".into(),
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

/// A second org the fixture's user owns, with a workspace.
async fn second_org(fx: &Fixture) -> (Uuid, Uuid) {
    let org = seed_org(&fx.db).await;
    join_org(&fx.db, org, fx.user.id, OrgRole::Owner).await;
    (org, seed_workspace_in(&fx.db, org).await)
}

/// The caller's own token list entry for `id`.
async fn own_token(fx: &Fixture, id: Uuid) -> Value {
    let (status, list) = in_session(&fx.cookie, "GET", "/user/tokens", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["tokens"]
        .as_array()
        .expect("tokens")
        .iter()
        .find(|t| t["id"] == id.to_string())
        .cloned()
        .unwrap_or_else(|| panic!("token {id} is listed: {list}"))
}

/// The org inventory's row for `id`.
async fn inventory_row(fx: &Fixture, org: Uuid, id: Uuid) -> Value {
    let uri = format!("/orgs/{org}/tokens");
    let (status, list) = in_session(&fx.cookie, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["tokens"]
        .as_array()
        .expect("tokens")
        .iter()
        .find(|t| t["id"] == id.to_string())
        .cloned()
        .unwrap_or_else(|| panic!("token {id} is in the inventory: {list}"))
}

#[tokio::test]
async fn the_policy_reads_the_defaults_until_an_admin_sets_one() {
    let fx = admin_fixture().await;
    let uri = policy_uri(fx.org_id);
    let (status, read) = in_session(&fx.cookie, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read, policy(None, true, true), "no row reads the defaults");

    let wanted = policy(Some(90), false, false);
    let (status, saved) = in_session(&fx.cookie, "PUT", &uri, Some(wanted.clone())).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved, wanted);
    assert_eq!(in_session(&fx.cookie, "GET", &uri, None).await.1, wanted);

    let rows = audit_rows(&fx.db, "token_policy.updated").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].org_id, Some(fx.org_id));
    assert_eq!(rows[0].actor_user_id, Some(fx.user.id));
    assert_eq!(rows[0].before, Some(policy(None, true, true)));
    assert_eq!(rows[0].after, Some(wanted.clone()));

    // The same policy again changes nothing and records nothing.
    in_session(&fx.cookie, "PUT", &uri, Some(wanted)).await;
    assert_eq!(audit_rows(&fx.db, "token_policy.updated").await.len(), 1);
}

#[tokio::test]
async fn a_policy_the_contract_does_not_allow_is_400() {
    let fx = admin_fixture().await;
    let uri = policy_uri(fx.org_id);
    for body in [
        policy(Some(0), true, true),
        policy(Some(3651), true, true),
        policy(Some(-1), true, true),
        json!({ "max_lifetime_days": 30 }),
        json!({ "max_lifetime_days": "30", "allow_all_access_tokens": true,
                "require_environment_on_trust_policies": true }),
    ] {
        let (status, refused) = in_session(&fx.cookie, "PUT", &uri, Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} → {refused}");
    }
    for ok in [Some(1), Some(3650), None] {
        let (status, _) = in_session(&fx.cookie, "PUT", &uri, Some(policy(ok, true, true))).await;
        assert_eq!(status, StatusCode::OK, "{ok:?}");
    }
}

#[tokio::test]
async fn only_an_org_admin_reads_it_and_only_in_a_browser_session_sets_it() {
    let fx = admin_fixture().await;
    let uri = policy_uri(fx.org_id);

    let member = seed_user(&fx.db, "member").await;
    join_org(&fx.db, fx.org_id, member.id, OrgRole::Member).await;
    let member_session = session_of(&member).await;
    for method in ["GET", "PUT"] {
        let body = (method == "PUT").then(|| policy(Some(30), true, true));
        let (status, _) = in_session(&member_session, method, &uri, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "a member {method}s");
    }

    // The owner's own token reads it, and is refused the write.
    let (_, secret) = mint(&fx.db, fx.user.id, Reach::all_access()).await;
    let (status, read) = with_token(&secret, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    let (status, refused) =
        with_token(&secret, "PUT", &uri, Some(policy(Some(30), true, true))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(refused["code"], "session_required");
    assert!(audit_rows(&fx.db, "token_policy.updated").await.is_empty());
}

#[tokio::test]
async fn a_token_past_the_cap_is_inert_in_the_capping_org_and_works_elsewhere() {
    let fx = admin_fixture().await;
    let (org_b, ws_b) = second_org(&fx).await;
    let grants = vec![
        org_grant(fx.org_id, RoleCeiling::Owner),
        org_grant(org_b, RoleCeiling::Owner),
    ];
    let (id, secret) = mint_expiring(&fx, Reach::granted(grants), Some(200)).await;
    let ws_a = fx.workspace_id;
    assert_eq!(
        workspace_status(&secret, ws_a, "GET", "read").await,
        StatusCode::NO_CONTENT
    );

    put_policy(&fx, fx.org_id, policy(Some(90), true, true)).await;

    assert_eq!(
        workspace_status(&secret, ws_a, "GET", "read").await,
        StatusCode::NOT_FOUND,
        "inert in the org whose cap it outlives"
    );
    let (status, _) = with_token(&secret, "GET", &format!("/orgs/{}", fx.org_id), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        workspace_status(&secret, ws_b, "GET", "read").await,
        StatusCode::NO_CONTENT,
        "every other org is unaffected"
    );

    // Inert, not revoked — and it says where and why.
    let token = own_token(&fx, id).await;
    assert_eq!(token["status"], "active");
    assert_eq!(
        token["blocked_orgs"],
        json!([{ "org_id": fx.org_id, "org_name": "Acme", "reason": "max_lifetime" }])
    );
    let here = inventory_row(&fx, fx.org_id, id).await;
    assert_eq!(here["blocked_by_policy"], "max_lifetime");
    let there = inventory_row(&fx, org_b, id).await;
    assert!(there["blocked_by_policy"].is_null());
    assert_eq!(
        there["blocked_orgs"],
        json!([]),
        "another org's block is not shown there"
    );

    // Lifting the cap gives it back.
    put_policy(&fx, fx.org_id, policy(None, true, true)).await;
    assert_eq!(
        workspace_status(&secret, ws_a, "GET", "read").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(own_token(&fx, id).await["blocked_orgs"], json!([]));
}

#[tokio::test]
async fn a_token_with_no_expiry_is_inert_under_any_cap() {
    let fx = admin_fixture().await;
    let grant = vec![org_grant(fx.org_id, RoleCeiling::Owner)];
    let (personal_id, personal) = mint_expiring(&fx, Reach::granted(grant), None).await;
    let sa = create_account(&fx, "deploy-bot", "member").await;
    let (sa_token, account) =
        mint_account_token(&fx, sa, json!({ "name": "ci", "expires_at": null })).await;
    let ws = fx.workspace_id;
    for secret in [&personal, &account] {
        assert_eq!(
            workspace_status(secret, ws, "GET", "read").await,
            StatusCode::NO_CONTENT
        );
    }

    put_policy(&fx, fx.org_id, policy(Some(3650), true, true)).await;

    for secret in [&personal, &account] {
        assert_eq!(
            workspace_status(secret, ws, "GET", "read").await,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        own_token(&fx, personal_id).await["blocked_orgs"][0]["reason"],
        "max_lifetime"
    );
    assert_eq!(
        inventory_row(&fx, fx.org_id, sa_token).await["blocked_by_policy"],
        "max_lifetime"
    );
}

#[tokio::test]
async fn the_create_dialog_is_told_each_orgs_real_policy() {
    // `GET /api/user/token-options` used to answer "no cap, all-access
    // allowed" for every org, so the dialog showed no cap and no warning.
    let fx = admin_fixture().await;
    let (org_b, _) = second_org(&fx).await;
    let policy_of = |options: &Value, org: Uuid| {
        options["orgs"]
            .as_array()
            .expect("orgs")
            .iter()
            .find(|o| o["org_id"] == org.to_string())
            .unwrap_or_else(|| panic!("org {org} is offered: {options}"))["policy"]
            .clone()
    };
    let defaults = json!({ "max_lifetime_days": null, "allow_all_access_tokens": true });

    let (status, options) = in_session(&fx.cookie, "GET", "/user/token-options", None).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(policy_of(&options, fx.org_id), defaults, "no row yet");
    assert_eq!(policy_of(&options, org_b), defaults);

    put_policy(&fx, fx.org_id, policy(Some(90), false, true)).await;
    let (_, options) = in_session(&fx.cookie, "GET", "/user/token-options", None).await;
    assert_eq!(
        policy_of(&options, fx.org_id),
        json!({ "max_lifetime_days": 90, "allow_all_access_tokens": false })
    );
    assert_eq!(policy_of(&options, org_b), defaults, "the other org's own");

    // A row that restricts nothing reads as the defaults do.
    put_policy(&fx, fx.org_id, policy(None, true, false)).await;
    let (_, options) = in_session(&fx.cookie, "GET", "/user/token-options", None).await;
    assert_eq!(policy_of(&options, fx.org_id), defaults);
}

#[tokio::test]
async fn refusing_all_access_tokens_blocks_them_while_grant_bound_ones_work() {
    let fx = admin_fixture().await;
    let (_org_b, ws_b) = second_org(&fx).await;
    let (all_id, all_access) = mint_expiring(&fx, Reach::all_access(), Some(30)).await;
    let grant = vec![org_grant(fx.org_id, RoleCeiling::Owner)];
    let (_, bound) = mint_expiring(&fx, Reach::granted(grant), Some(30)).await;
    let ws_a = fx.workspace_id;

    put_policy(&fx, fx.org_id, policy(None, false, true)).await;

    assert_eq!(
        workspace_status(&all_access, ws_a, "GET", "read").await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        workspace_status(&all_access, ws_b, "GET", "read").await,
        StatusCode::NO_CONTENT,
        "it still reaches the owner's other org"
    );
    assert_eq!(
        workspace_status(&bound, ws_a, "GET", "read").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        own_token(&fx, all_id).await["blocked_orgs"],
        json!([{ "org_id": fx.org_id, "org_name": "Acme", "reason": "all_access_disallowed" }])
    );
    assert_eq!(
        inventory_row(&fx, fx.org_id, all_id).await["blocked_by_policy"],
        "all_access_disallowed"
    );
}

#[tokio::test]
async fn an_expiry_past_the_cap_is_refused_on_create_extend_and_regenerate() {
    let fx = admin_fixture().await;
    // Minted before the cap: regenerating it later must not mint an inert secret.
    let grant = vec![org_grant(fx.org_id, RoleCeiling::Owner)];
    let (long_id, _) = mint_expiring(&fx, Reach::granted(grant), Some(200)).await;
    put_policy(&fx, fx.org_id, policy(Some(30), true, true)).await;
    let grants = json!([{ "org_id": fx.org_id, "workspace_id": null }]);

    let create = |days: i64| json!({ "name": "capped", "all_access": false, "grants": grants, "expires_in_days": days });
    let (status, refused) = in_session(&fx.cookie, "POST", "/user/tokens", Some(create(60))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "exceeds_policy");
    assert_eq!(refused["max_lifetime_days"], 30);
    let no_expiry =
        json!({ "name": "capped", "all_access": false, "grants": grants, "expires_at": null });
    let (status, refused) = in_session(&fx.cookie, "POST", "/user/tokens", Some(no_expiry)).await;
    assert_eq!(
        (status, refused["code"].clone()),
        (StatusCode::BAD_REQUEST, json!("exceeds_policy"))
    );

    let (status, minted) = in_session(&fx.cookie, "POST", "/user/tokens", Some(create(30))).await;
    assert_eq!(status, StatusCode::CREATED, "{minted}");
    let id = minted["token"]["id"].as_str().unwrap().to_string();
    let extend = format!("/user/tokens/{id}/extend");
    let (status, refused) =
        in_session(&fx.cookie, "POST", &extend, Some(json!({ "days": 60 }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "exceeds_policy");

    let regenerate = format!("/user/tokens/{long_id}/regenerate");
    let (status, refused) = in_session(&fx.cookie, "POST", &regenerate, None).await;
    assert_eq!(
        (status, refused["code"].clone()),
        (StatusCode::BAD_REQUEST, json!("exceeds_policy"))
    );

    // An all-access token is not refused: it goes inert where it is capped.
    let all = json!({ "name": "everything", "expires_in_days": 60 });
    let (status, minted) = in_session(&fx.cookie, "POST", "/user/tokens", Some(all)).await;
    assert_eq!(status, StatusCode::CREATED, "{minted}");
    assert_eq!(minted["token"]["blocked_orgs"][0]["reason"], "max_lifetime");

    // A service-account token: refused past the cap, at mint and at extend.
    let sa = create_account(&fx, "deploy-bot", "member").await;
    let tokens = format!("/orgs/{}/service-accounts/{sa}/tokens", fx.org_id);
    let (status, refused) = in_session(
        &fx.cookie,
        "POST",
        &tokens,
        Some(json!({ "name": "ci", "expires_in_days": 60 })),
    )
    .await;
    assert_eq!(
        (status, refused["code"].clone()),
        (StatusCode::BAD_REQUEST, json!("exceeds_policy"))
    );
    let (sa_token, _) =
        mint_account_token(&fx, sa, json!({ "name": "ci", "expires_in_days": 30 })).await;
    let extend = format!("{tokens}/{sa_token}/extend");
    let (status, refused) =
        in_session(&fx.cookie, "POST", &extend, Some(json!({ "days": 60 }))).await;
    assert_eq!(
        (status, refused["code"].clone()),
        (StatusCode::BAD_REQUEST, json!("exceeds_policy"))
    );
}

#[tokio::test]
async fn whether_a_trust_policy_needs_an_environment_follows_the_org_policy() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let sa = create_account(&fx, "deployer", "member").await;
    let mut body = policy_body(json!([whole_org("member")]));
    body["environment"] = Value::Null;
    let uri = policies_uri(fx.org_id, sa);

    let (status, refused) = in_session(&fx.cookie, "POST", &uri, Some(body.clone())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        refused["code"], "environment_required",
        "required by default"
    );

    put_policy(&fx, fx.org_id, policy(None, true, false)).await;
    let (status, created) = in_session(&fx.cookie, "POST", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert!(created["environment"].is_null());
}

/// The policy a ci token was minted from.
async fn minted_from(fx: &Fixture, token: Uuid) -> Option<Uuid> {
    api_tokens::Entity::find_by_id(token)
        .one(&fx.db)
        .await
        .unwrap()
        .expect("the token row")
        .trust_policy_id
}

#[tokio::test]
async fn turning_the_environment_requirement_back_on_stops_policies_that_name_none() {
    trust_test_keys();
    let fx = admin_fixture().await;
    let sa = create_account(&fx, "deployer", "member").await;
    let no_environment = || Run::new().with("environment", Value::Null);

    // Relaxed, the org registers a policy with no environment. It admits the
    // run with an environment or without.
    put_policy(&fx, fx.org_id, policy(None, true, false)).await;
    let mut body = policy_body(json!([whole_org("member")]));
    body["environment"] = Value::Null;
    let bare = register_policy(&fx, sa, body).await;
    exchange_ok(&fx, &Run::new()).await;
    exchange_ok(&fx, &no_environment()).await;

    // Required again. Nothing rewrote the policy: the requirement is asked
    // where a policy is used, so it stops minting at once — and the refusal
    // says what to fix and who fixes it.
    put_policy(&fx, fx.org_id, policy(None, true, true)).await;
    for run in [Run::new(), no_environment()] {
        let (status, refusal) = exchange(&fx, &run).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{refusal}");
        assert_eq!(refusal["code"], "missing_environment", "{refusal}");
        assert!(refusal.get("token").is_none(), "{refusal}");
        let said = refusal["error"].as_str().unwrap_or_default();
        assert!(said.contains("organization requires"), "{said}");
    }
    assert_eq!(
        audit_rows(&fx.db, "oidc.token_exchanged").await.len(),
        2,
        "nothing more was minted"
    );

    // A policy that names its environment is not held back by the older one
    // beside it: the run mints from it, and only a run in that environment.
    let named = register_policy(&fx, sa, policy_body(json!([whole_org("member")]))).await;
    let (token, _) = exchange_ok(&fx, &Run::new()).await;
    assert_eq!(minted_from(&fx, token).await, Some(named));
    let (status, refusal) = exchange(&fx, &no_environment()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refusal}");
    assert_eq!(refusal["code"], "missing_environment", "{refusal}");

    // Relaxed once more, the first policy mints again, exactly as it was —
    // and, being the oldest, is the one a run mints from.
    put_policy(&fx, fx.org_id, policy(None, true, false)).await;
    let (token, _) = exchange_ok(&fx, &no_environment()).await;
    assert_eq!(minted_from(&fx, token).await, Some(bare));
}

#[tokio::test]
async fn a_legacy_key_is_never_blocked_or_capped() {
    let fx = admin_fixture().await;
    let (key_id, key) = legacy_key(&fx, None).await;
    let (pat_id, pat) = endpoint_key(&fx, None).await;
    // The strictest policy there is.
    put_policy(&fx, fx.org_id, policy(Some(1), false, true)).await;

    let ws = fx.workspace_id;
    for secret in [&key, &pat] {
        let (status, _) = super::call(
            super::stack::workspace_api(),
            "GET",
            &format!("/{ws}/read"),
            &[("x-api-key", secret)],
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "a legacy key still reaches the org"
        );
    }
    for id in [key_id, pat_id] {
        // A legacy key is not in the token list; the org inventory shows it.
        let row = inventory_row(&fx, fx.org_id, id).await;
        assert_eq!(row["kind"], "legacy_key");
        assert_eq!(row["blocked_orgs"], json!([]));
        assert!(row["blocked_by_policy"].is_null());
        // Extend, through the legacy route, is never capped for one — not
        // even to no expiry.
        let extend = format!("/{ws}/api-keys/{id}/extend");
        for body in [json!({ "days": 365 }), json!({ "expires_at": null })] {
            let (status, extended) = super::call(
                super::api_surface(),
                "POST",
                &extend,
                &[("cookie", &fx.cookie)],
                Some(body),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{extended}");
        }
    }
}
