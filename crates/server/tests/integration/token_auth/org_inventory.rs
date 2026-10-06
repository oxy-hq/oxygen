//! Phase 3: the org token inventory and revoke-grant (design §5), plus the
//! legacy-key regression every phase carries (§3.5).
//!
//! The inventory is asserted against real tokens of every kind; revoke-grant
//! is asserted by what the token can still reach afterwards — in this org
//! and in another — not by the rows it wrote.

use axum::http::StatusCode;
use entity::org_members::OrgRole;
use entity::{api_token_grants, api_tokens};
use oxy_authz::RoleCeiling;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::legacy_reach::{assert_full_reach, world};
use super::service_accounts::{
    admin_fixture, create_account, in_session, mint_account_token, session_of, with_token,
    workspace_status,
};
use super::stack::{Reach, get_as, join_org, mint, org_grant, workspace_grant};
use super::{
    Fixture, audit_rows, call, endpoint_key, legacy_key, seed_org, seed_user, seed_workspace_in,
};

fn inventory_uri(org: Uuid) -> String {
    format!("/orgs/{org}/tokens")
}

fn revoke_uri(org: Uuid, token: Uuid) -> String {
    format!("/orgs/{org}/tokens/{token}/revoke-grant")
}

/// The inventory rows, by token id.
async fn inventory(fx: &Fixture, query: &str) -> Vec<Value> {
    let uri = format!("{}{query}", inventory_uri(fx.org_id));
    let (status, body) = in_session(&fx.cookie, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    body["tokens"].as_array().expect("tokens").clone()
}

fn ids_of(rows: &[Value]) -> Vec<String> {
    let mut ids: Vec<String> = rows
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    ids
}

fn sorted(ids: &[Uuid]) -> Vec<String> {
    let mut ids: Vec<String> = ids.iter().map(Uuid::to_string).collect();
    ids.sort();
    ids
}

fn row_of(rows: &[Value], id: Uuid) -> &Value {
    rows.iter()
        .find(|r| r["id"] == id.to_string())
        .unwrap_or_else(|| panic!("token {id} is not in the inventory"))
}

async fn grants_of(db: &DatabaseConnection, token: Uuid) -> Vec<api_token_grants::Model> {
    api_token_grants::Entity::find()
        .filter(api_token_grants::Column::TokenId.eq(token))
        .all(db)
        .await
        .unwrap()
}

/// Use a legacy key once, so it has its `api_tokens` mirror — as every key
/// that existed at the migration, or has been used since, does.
async fn touch(fx: &Fixture, key: &str) {
    let uri = format!("/{}/read", fx.workspace_id);
    let (status, _) = call(
        super::stack::workspace_external(),
        "GET",
        &uri,
        &[("x-api-key", key)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "the legacy key works");
}

struct Tokens {
    fx: Fixture,
    ws2: Uuid,
    org_b: Uuid,
    legacy: Uuid,
    all_access: Uuid,
    narrowed: Uuid,
    account: Uuid,
    account_token: Uuid,
}

/// An org with one token of each kind reaching it, and two that do not.
async fn tokens() -> Tokens {
    let fx = admin_fixture().await;
    let ws2 = seed_workspace_in(&fx.db, fx.org_id).await;
    let org_b = seed_org(&fx.db).await;
    join_org(&fx.db, org_b, fx.user.id, OrgRole::Owner).await;

    let (legacy, key) = legacy_key(&fx, None).await;
    touch(&fx, &key).await;
    let (all_access, _) = mint(&fx.db, fx.user.id, Reach::all_access()).await;
    // One workspace here, and the whole of another org.
    let grants = vec![
        workspace_grant(fx.org_id, fx.workspace_id, RoleCeiling::Member),
        org_grant(org_b, RoleCeiling::Admin),
    ];
    let (narrowed, _) = mint(&fx.db, fx.user.id, Reach::granted(grants)).await;
    let account = create_account(&fx, "deploy-bot", "member").await;
    let (account_token, _) = mint_account_token(&fx, account, json!({ "name": "release" })).await;

    // Not reaching this org: a token granted only org B, and a stranger's.
    mint(
        &fx.db,
        fx.user.id,
        Reach::granted(vec![org_grant(org_b, RoleCeiling::Owner)]),
    )
    .await;
    let stranger = seed_user(&fx.db, "stranger").await;
    mint(&fx.db, stranger.id, Reach::all_access()).await;

    Tokens {
        fx,
        ws2,
        org_b,
        legacy,
        all_access,
        narrowed,
        account,
        account_token,
    }
}

#[tokio::test]
async fn the_inventory_lists_every_kind_with_its_grants_here() {
    let t = tokens().await;
    let rows = inventory(&t.fx, "").await;
    assert_eq!(
        ids_of(&rows),
        sorted(&[t.legacy, t.all_access, t.narrowed, t.account_token]),
        "every token reaching the org, and no other"
    );
    for row in &rows {
        assert!(row["blocked_by_policy"].is_null());
        assert_eq!(row["status"], "active");
    }

    let legacy = row_of(&rows, t.legacy);
    assert_eq!(legacy["kind"], "legacy_key");
    assert_eq!(legacy["grants_here"], json!([]));
    assert_eq!(legacy["all_access"], true);
    assert_eq!(legacy["owner"]["type"], "user");
    assert_eq!(legacy["owner"]["id"], t.fx.user.id.to_string());

    let all_access = row_of(&rows, t.all_access);
    assert_eq!(all_access["kind"], "personal");
    assert_eq!(all_access["grants_here"], json!([]));

    let narrowed = row_of(&rows, t.narrowed);
    assert_eq!(narrowed["kind"], "personal");
    let here = narrowed["grants_here"].as_array().unwrap();
    assert_eq!(here.len(), 1, "only this org's grant: {narrowed}");
    assert_eq!(here[0]["org_id"], t.fx.org_id.to_string());
    assert_eq!(here[0]["workspace_id"], t.fx.workspace_id.to_string());
    assert_eq!(here[0]["role_ceiling"], "member");
    assert!(here[0]["revoked_at"].is_null());
    // What it reaches in another org is not this org's to read.
    assert_eq!(narrowed["grants"], narrowed["grants_here"]);
    assert!(!narrowed.to_string().contains(&t.org_b.to_string()));

    let account = row_of(&rows, t.account_token);
    assert_eq!(account["kind"], "service_account");
    assert_eq!(account["owner"]["type"], "service_account");
    assert_eq!(account["owner"]["id"], t.account.to_string());
    assert_eq!(account["owner"]["label"], "deploy-bot");
    assert_eq!(account["grants_here"].as_array().unwrap().len(), 1);
    assert!(account["grants_here"][0]["workspace_id"].is_null());
}

#[tokio::test]
async fn the_inventory_filters_by_kind_owner_and_workspace() {
    let t = tokens().await;
    let filtered = |query: String| {
        let fx = &t.fx;
        async move { ids_of(&inventory(fx, &query).await) }
    };

    assert_eq!(
        filtered("?kind=legacy_key".into()).await,
        sorted(&[t.legacy])
    );
    assert_eq!(
        filtered("?kind=personal".into()).await,
        sorted(&[t.all_access, t.narrowed])
    );
    assert_eq!(
        filtered("?kind=service_account".into()).await,
        sorted(&[t.account_token])
    );
    assert_eq!(filtered("?kind=ci".into()).await, Vec::<String>::new());

    // `owner` is `Token.owner.id`: a person's id, or the service account's.
    assert_eq!(
        filtered(format!("?owner={}", t.fx.user.id)).await,
        sorted(&[t.legacy, t.all_access, t.narrowed])
    );
    assert_eq!(
        filtered(format!("?owner={}", t.account)).await,
        sorted(&[t.account_token])
    );
    assert_eq!(filtered("?owner=nobody".into()).await, Vec::<String>::new());

    // The narrowed token is granted the first workspace only.
    assert_eq!(
        filtered(format!("?workspace_id={}", t.fx.workspace_id)).await,
        sorted(&[t.legacy, t.all_access, t.narrowed, t.account_token])
    );
    assert_eq!(
        filtered(format!("?workspace_id={}", t.ws2)).await,
        sorted(&[t.legacy, t.all_access, t.account_token])
    );
    // A workspace that is not this org's matches nothing.
    let foreign = seed_workspace_in(&t.fx.db, t.org_b).await;
    assert_eq!(
        filtered(format!("?workspace_id={foreign}")).await,
        Vec::<String>::new()
    );
    // Filters combine.
    assert_eq!(
        filtered(format!("?kind=personal&workspace_id={}", t.ws2)).await,
        sorted(&[t.all_access])
    );
}

#[tokio::test]
async fn a_tokens_activity_in_the_org_is_this_orgs_events_only() {
    let t = tokens().await;
    // A token minted through the API, so its creation is audited — once in
    // each of the two orgs it reaches.
    let body = json!({ "name": "two orgs", "all_access": false, "grants": [
        { "org_id": t.fx.org_id, "workspace_id": t.fx.workspace_id, "role_ceiling": "member" },
        { "org_id": t.org_b, "role_ceiling": "admin" },
    ] });
    let (status, minted) = in_session(&t.fx.cookie, "POST", "/user/tokens", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{minted}");
    let id = minted["token"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        audit_rows(&t.fx.db, "token.created")
            .await
            .iter()
            .filter(|r| { r.target_id.as_deref() == Some(id.as_str()) })
            .count(),
        2
    );

    let uri = format!("{}/{id}/activity", inventory_uri(t.fx.org_id));
    let (status, activity) = in_session(&t.fx.cookie, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{activity}");
    let events = activity["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "this org's copy of the event, and only it");
    assert_eq!(events[0]["action"], "token.created");
    assert_eq!(events[0]["org_id"], t.fx.org_id.to_string());
    assert_eq!(
        activity["usage"],
        json!([]),
        "a person's usage is not the org's"
    );

    // The org's own token has its whole activity.
    let uri = format!(
        "{}/{}/activity",
        inventory_uri(t.fx.org_id),
        t.account_token
    );
    let (status, activity) = in_session(&t.fx.cookie, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(activity["events"][0]["action"], "token.created");

    // A token that does not reach the org reads as not found.
    let stranger = seed_user(&t.fx.db, "other").await;
    let (elsewhere, _) = mint(&t.fx.db, stranger.id, Reach::all_access()).await;
    let uri = format!("{}/{elsewhere}/activity", inventory_uri(t.fx.org_id));
    assert_eq!(
        in_session(&t.fx.cookie, "GET", &uri, None).await.0,
        StatusCode::NOT_FOUND
    );
}

// ── revoke-grant ─────────────────────────────────────────────────────────────

struct TwoOrgs {
    fx: Fixture,
    org_b: Uuid,
    ws_b: Uuid,
    /// The token's owner: an Owner of both orgs, with their own session.
    owner_cookie: String,
    owner: Uuid,
}

/// An admin (the fixture's user) of org A, and a second person who owns a
/// token and is an Owner in org A and org B.
async fn two_orgs() -> TwoOrgs {
    let fx = admin_fixture().await;
    let org_b = seed_org(&fx.db).await;
    let ws_b = seed_workspace_in(&fx.db, org_b).await;
    let owner = seed_user(&fx.db, "token-owner").await;
    join_org(&fx.db, fx.org_id, owner.id, OrgRole::Owner).await;
    join_org(&fx.db, org_b, owner.id, OrgRole::Owner).await;
    TwoOrgs {
        owner_cookie: session_of(&owner).await,
        owner: owner.id,
        org_b,
        ws_b,
        fx,
    }
}

/// `(org A's own route, a workspace of A, org B's own route, a workspace of B)`.
async fn reach(w: &TwoOrgs, secret: &str) -> [StatusCode; 4] {
    [
        get_as(secret, &format!("/orgs/{}", w.fx.org_id)).await.0,
        workspace_status(secret, w.fx.workspace_id, "GET", "read").await,
        get_as(secret, &format!("/orgs/{}", w.org_b)).await.0,
        workspace_status(secret, w.ws_b, "GET", "read").await,
    ]
}

const EVERYWHERE: [StatusCode; 4] = [
    StatusCode::OK,
    StatusCode::NO_CONTENT,
    StatusCode::OK,
    StatusCode::NO_CONTENT,
];
/// Org A answers as an org the token was never granted; org B is untouched.
const ONLY_ORG_B: [StatusCode; 4] = [
    StatusCode::NOT_FOUND,
    StatusCode::NOT_FOUND,
    StatusCode::OK,
    StatusCode::NO_CONTENT,
];

#[tokio::test]
async fn revoke_grant_ends_a_tokens_reach_in_this_org_only() {
    let w = two_orgs().await;
    let grants = vec![
        org_grant(w.fx.org_id, RoleCeiling::Admin),
        org_grant(w.org_b, RoleCeiling::Admin),
    ];
    let (id, secret) = mint(&w.fx.db, w.owner, Reach::granted(grants)).await;
    assert_eq!(reach(&w, &secret).await, EVERYWHERE);

    let uri = revoke_uri(w.fx.org_id, id);
    let (status, _) = in_session(&w.fx.cookie, "POST", &uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(reach(&w, &secret).await, ONLY_ORG_B);

    // The owner sees the grant marked revoked, and the other one untouched.
    let (status, token) =
        in_session(&w.owner_cookie, "GET", &format!("/user/tokens/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{token}");
    assert_eq!(token["status"], "active", "the token itself is not revoked");
    for grant in token["grants"].as_array().unwrap() {
        let here = grant["org_id"] == w.fx.org_id.to_string();
        assert_eq!(grant["revoked_at"].is_string(), here, "{grant}");
    }

    // One event, in this org's chain, by the admin who did it.
    let rows = audit_rows(&w.fx.db, "token.grant_revoked_by_org").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].org_id, Some(w.fx.org_id));
    assert_eq!(rows[0].actor_user_id, Some(w.fx.user.id));
    assert_eq!(rows[0].target_id.as_deref(), Some(id.to_string().as_str()));

    // Idempotent: nothing more is written.
    let (status, _) = in_session(&w.fx.cookie, "POST", &uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        audit_rows(&w.fx.db, "token.grant_revoked_by_org")
            .await
            .len(),
        1
    );

    // The owner cannot grant themselves back in — by a new grant, or by
    // making the token all-access.
    let regrant = json!({ "grants": [
        { "org_id": w.fx.org_id, "workspace_id": w.fx.workspace_id },
        { "org_id": w.org_b },
    ] });
    let patch = format!("/user/tokens/{id}");
    let (status, _) = in_session(&w.owner_cookie, "PATCH", &patch, Some(regrant)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reach(&w, &secret).await, ONLY_ORG_B);
    let widen = json!({ "all_access": true });
    let (status, _) = in_session(&w.owner_cookie, "PATCH", &patch, Some(widen)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reach(&w, &secret).await, ONLY_ORG_B);

    // The inventory keeps the ended row, as history.
    let rows = inventory(&w.fx, "").await;
    let row = row_of(&rows, id);
    assert!(
        row["grants_here"]
            .as_array()
            .unwrap()
            .iter()
            .all(|g| g["revoked_at"].is_string())
    );
}

#[tokio::test]
async fn revoke_grant_blocks_an_all_access_token_in_this_org_only() {
    let w = two_orgs().await;
    let (id, secret) = mint(&w.fx.db, w.owner, Reach::all_access()).await;
    assert_eq!(reach(&w, &secret).await, EVERYWHERE);

    let (status, _) = in_session(&w.fx.cookie, "POST", &revoke_uri(w.fx.org_id, id), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(reach(&w, &secret).await, ONLY_ORG_B);

    // Stored as a revoked org-wide grant beside a token that is still
    // all-access — which is what the owner's list shows them.
    let grants = grants_of(&w.fx.db, id).await;
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].org_id, w.fx.org_id);
    assert_eq!(grants[0].workspace_id, None);
    assert!(grants[0].revoked_at.is_some());
    assert_eq!(grants[0].revoked_by, Some(w.fx.user.id));
    let row = api_tokens::Entity::find_by_id(id)
        .one(&w.fx.db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.all_access && row.revoked_at.is_none());
    let (_, token) = in_session(&w.owner_cookie, "GET", &format!("/user/tokens/{id}"), None).await;
    assert_eq!(token["all_access"], true);
    assert_eq!(token["grants"].as_array().unwrap().len(), 1);
    assert!(token["grants"][0]["revoked_at"].is_string());

    // Discovery follows: the token lists only the org it still reaches.
    let (status, orgs) = get_as(&secret, "/orgs").await;
    assert_eq!(status, StatusCode::OK);
    let listed: Vec<&str> = orgs
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap())
        .collect();
    assert_eq!(listed, [w.org_b.to_string().as_str()]);

    // `GET /api/user/token-options` is unchanged by any of it.
    let (status, options) = in_session(&w.owner_cookie, "GET", "/user/token-options", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(options["orgs"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn revoke_grant_refuses_what_is_not_the_orgs_to_end() {
    let w = two_orgs().await;
    let fx = &w.fx;

    // A legacy key, in both its shapes: only its owner can end it.
    let (legacy, key) = legacy_key(fx, None).await;
    touch(fx, &key).await;
    let (minted, pat) = endpoint_key(fx, None).await;
    for (id, secret) in [(legacy, &key), (minted, &pat)] {
        let (status, refused) =
            in_session(&fx.cookie, "POST", &revoke_uri(fx.org_id, id), None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{refused}");
        assert_eq!(refused["code"], "legacy_immutable");
        assert!(
            grants_of(&fx.db, id).await.is_empty(),
            "nothing was written"
        );
        assert_eq!(
            workspace_status(secret, fx.workspace_id, "POST", "manage").await,
            StatusCode::NO_CONTENT,
            "and the key works exactly as before"
        );
    }

    // The org's own token is revoked from its account.
    let account = create_account(fx, "deploy-bot", "member").await;
    let (account_token, secret) = mint_account_token(fx, account, json!({ "name": "t" })).await;
    let uri = revoke_uri(fx.org_id, account_token);
    let (status, refused) = in_session(&fx.cookie, "POST", &uri, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["code"], "use_service_account_routes");
    assert_eq!(
        workspace_status(&secret, fx.workspace_id, "GET", "read").await,
        StatusCode::NO_CONTENT
    );

    // A token that does not reach this org, and an id that is no token.
    let stranger = seed_user(&fx.db, "stranger").await;
    let (elsewhere, _) = mint(&fx.db, stranger.id, Reach::all_access()).await;
    for id in [
        elsewhere.to_string(),
        Uuid::new_v4().to_string(),
        "nope".to_string(),
    ] {
        let uri = format!("/orgs/{}/tokens/{id}/revoke-grant", fx.org_id);
        assert_eq!(
            in_session(&fx.cookie, "POST", &uri, None).await.0,
            StatusCode::NOT_FOUND,
            "{id}"
        );
    }
    assert!(
        audit_rows(&fx.db, "token.grant_revoked_by_org")
            .await
            .is_empty()
    );
}

// ── The legacy-key regression (design §3.5) ──────────────────────────────────

#[tokio::test]
async fn a_legacy_key_is_unaffected_by_service_accounts_and_org_revokes() {
    // The Phase 2 world — a staff member who owns two orgs — with everything
    // Phase 3 adds happening around its keys.
    let w = world().await;
    let fx = &w.fx;
    let (legacy, key) = legacy_key(fx, None).await;
    let (minted, pat) = endpoint_key(fx, None).await;
    assert_full_reach(&w, "an oxy_<hex> key, before", &key).await;
    assert_full_reach(&w, "a legacy-endpoint token, before", &pat).await;

    // The org gains service accounts and tokens; one is disabled, one deleted.
    let account = create_account(fx, "deploy-bot", "admin").await;
    mint_account_token(fx, account, json!({ "name": "t" })).await;
    let doomed = create_account(fx, "doomed-bot", "member").await;
    let accounts = format!("/orgs/{}/service-accounts", fx.org_id);
    let disable = json!({ "disabled": true });
    in_session(
        &fx.cookie,
        "PATCH",
        &format!("{accounts}/{account}"),
        Some(disable),
    )
    .await;
    in_session(&fx.cookie, "DELETE", &format!("{accounts}/{doomed}"), None).await;

    // The org ends the reach of the owner's OTHER tokens...
    let (personal, _) = mint(&fx.db, fx.user.id, Reach::all_access()).await;
    let (status, _) = in_session(&fx.cookie, "POST", &revoke_uri(fx.org_id, personal), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // ...and tries to end the keys', in both orgs. It cannot.
    for org in [fx.org_id, w.org_b] {
        for id in [legacy, minted] {
            let (status, refused) =
                in_session(&fx.cookie, "POST", &revoke_uri(org, id), None).await;
            assert_eq!(status, StatusCode::CONFLICT);
            assert_eq!(refused["code"], "legacy_immutable");
        }
    }

    // Nothing narrowed, blocked, expired or revoked them.
    assert_full_reach(&w, "an oxy_<hex> key, after", &key).await;
    assert_full_reach(&w, "a legacy-endpoint token, after", &pat).await;
    for id in [legacy, minted] {
        assert!(grants_of(&fx.db, id).await.is_empty());
        let row = api_tokens::Entity::find_by_id(id)
            .one(&fx.db)
            .await
            .unwrap()
            .unwrap();
        assert!(row.all_access && row.platform && row.partner);
        assert!(row.revoked_at.is_none());
        assert_eq!(row.expires_at, None);
    }

    // A legacy key reads the new admin routes as its owner would, and is
    // listed — as a legacy key — in the inventory of each org it reaches.
    for secret in [&key, &pat] {
        assert_eq!(
            with_token(secret, "GET", &accounts, None).await.0,
            StatusCode::OK
        );
        let (status, listed) =
            with_token(secret, "GET", &format!("/orgs/{}/tokens", fx.org_id), None).await;
        assert_eq!(status, StatusCode::OK);
        let rows = listed["tokens"].as_array().unwrap();
        for id in [legacy, minted] {
            assert_eq!(row_of(rows, id)["kind"], "legacy_key");
        }
    }
}
