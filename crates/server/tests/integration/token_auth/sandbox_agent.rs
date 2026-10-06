//! The sandbox agent token (`oxy_sbx_`): minting one, and what its minter may
//! do with it on `/api/user/tokens` (sandbox agent credential design §2).
//!
//! Also the seeds its sibling modules share — `sandbox_agent_reach` (what a
//! stored token is admitted as), `sandbox_agent_cli` (minting from `oxyc`) and
//! `sandbox_agent_staff` (the cross-admin view).

use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use entity::org_members::OrgRole;
use entity::{api_token_grants, api_tokens, app_admin_scope_orgs, app_admins, apps, users};
use oxy_app::server::authz::Caller;
use oxy_auth::token::{CredentialContext, StoredKind, TokenFormat, store};
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::PlatformRole;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::stack::{Reach, flat_api, get_in_session, join_org, make_staff, mint, published_app};
use super::{Fixture, audit_rows, call, fixture, pat_row, seed_org, seed_user, seed_workspace_in};

/// The fixture's user as Oxy staff (a Global Admin over every org), and an
/// app of the fixture's org published from its workspace.
pub(super) async fn staff_with_app() -> (Fixture, apps::Model) {
    let fx = fixture().await;
    make_staff(&fx.db, fx.user.email.as_deref().unwrap()).await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    (fx, app)
}

/// A platform grant of `role` for `email`, bounded to `orgs`.
pub(super) async fn grant_staff(
    db: &DatabaseConnection,
    email: &str,
    role: PlatformRole,
    orgs: &[Uuid],
) {
    let id = Uuid::new_v4();
    app_admins::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(email.to_string()),
        granted_by: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
        role: ActiveValue::Set(role.as_str().to_string()),
        scope_all: ActiveValue::Set(false),
        updated_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("seed a bounded staff grant");
    for org_id in orgs {
        app_admin_scope_orgs::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            app_admin_id: ActiveValue::Set(id),
            org_id: ActiveValue::Set(*org_id),
            created_at: ActiveValue::NotSet,
            created_by: ActiveValue::Set(None),
        }
        .insert(db)
        .await
        .expect("seed a staff scope org");
    }
}

/// Another user with a browser session of their own.
pub(super) async fn another_session(
    db: &DatabaseConnection,
    prefix: &str,
) -> (users::Model, String) {
    let user = seed_user(db, prefix).await;
    let jwt = oxy_app::server::api::auth::create_auth_token(user.clone())
        .await
        .expect("mint a session");
    (user, format!("oxy_session={jwt}"))
}

pub(super) async fn post_as(cookie: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    call(flat_api(), "POST", uri, &[("cookie", cookie)], Some(body)).await
}

pub(super) fn mint_body(apps: &[Uuid]) -> Value {
    json!({ "name": "agent", "kind": "sandbox_agent", "apps": apps })
}

/// Mint a sandbox agent token for `apps` under the fixture's session:
/// `(token id, secret)`.
pub(super) async fn minted(fx: &Fixture, apps: &[Uuid]) -> (Uuid, String) {
    let (status, body) = post_as(&fx.cookie, "/user/tokens", mint_body(apps)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = Uuid::parse_str(body["token"]["id"].as_str().expect("an id")).unwrap();
    (id, body["secret"].as_str().expect("the secret").to_string())
}

/// What a request presenting `secret` would be admitted as — the store call
/// the dispatch makes for an `oxy_sbx_` token.
pub(super) async fn admitted(fx: &Fixture, secret: &str) -> Result<CredentialContext, String> {
    store::resolve(&fx.db, secret, TokenFormat::SandboxAgent)
        .await
        .map(|resolved| resolved.credential)
        .map_err(|e| e.to_string())
}

/// The caller a request on the token would be.
pub(super) async fn token_caller(fx: &Fixture, secret: &str) -> Caller {
    let credential = admitted(fx, secret).await.expect("the token is admitted");
    let user = AuthenticatedUser::from(fx.user.clone()).with_credential(Some(credential));
    Caller::from_user(&user)
}

pub(super) async fn grants_of(
    db: &DatabaseConnection,
    token_id: Uuid,
) -> Vec<api_token_grants::Model> {
    api_token_grants::Entity::find()
        .filter(api_token_grants::Column::TokenId.eq(token_id))
        .all(db)
        .await
        .unwrap()
}

pub(super) fn assert_refused(
    (status, body): (StatusCode, Value),
    expected: StatusCode,
    code: &str,
    why: &str,
) {
    assert_eq!(status, expected, "{why}: {body}");
    assert_eq!(body["code"], code, "{why}: {body}");
    assert!(
        body.get("secret").is_none(),
        "{why}: no secret on a refusal"
    );
}

// ── Minting ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_staff_session_mints_a_token_of_the_kinds_fixed_shape() {
    let (fx, app) = staff_with_app().await;
    let (status, body) = post_as(&fx.cookie, "/user/tokens", mint_body(&[app.id])).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let secret = body["secret"].as_str().expect("the secret");
    assert!(secret.starts_with("oxy_sbx_"), "{secret}");
    assert!(oxy_auth::token::verify_checksum(secret));

    let token = &body["token"];
    assert_eq!(token["kind"], "sandbox_agent");
    assert_eq!(token["name"], "agent");
    assert_eq!(token["source"], "ui");
    assert_eq!(token["all_access"], false);
    assert_eq!(token["platform"], true);
    assert_eq!(token["partner"], false);
    assert_eq!(token["status"], "active");
    assert_eq!(token["owner"]["id"], json!(fx.user.id));

    // Eight hours when the body asks for no lifetime.
    let expires = DateTime::parse_from_rfc3339(token["expires_at"].as_str().expect("an expiry"))
        .unwrap()
        .with_timezone(&Utc);
    let lifetime = expires - Utc::now();
    assert!(
        lifetime > Duration::minutes(7 * 60 + 59) && lifetime <= Duration::hours(8),
        "8 hours, got {lifetime}"
    );

    // One `app_sandbox` grant: the org and the app, no workspace, no ceiling.
    let grants = token["grants"].as_array().expect("grants");
    assert_eq!(grants.len(), 1, "{token}");
    assert_eq!(grants[0]["kind"], "app_sandbox");
    assert_eq!(grants[0]["org_id"], json!(fx.org_id));
    assert_eq!(grants[0]["org_name"], "Acme");
    assert_eq!(grants[0]["app_id"], json!(app.id));
    assert_eq!(grants[0]["app_name"], "Token Test App");
    // And both slugs, which is how the app is named to a person: `org/app`.
    assert_eq!(grants[0]["app_slug"], json!(app.slug));
    let org_slug = grants[0]["org_slug"].as_str().expect("the org's slug");
    assert!(org_slug.starts_with("acme-"), "{org_slug}");
    assert_eq!(grants[0]["workspace_id"], Value::Null);
    assert_eq!(grants[0]["workspace_name"], Value::Null);
    assert_eq!(grants[0]["role_ceiling"], Value::Null);

    // Stored as the kind's fixed shape, hash only.
    let id = Uuid::parse_str(token["id"].as_str().unwrap()).unwrap();
    let row = pat_row(&fx.db, id).await;
    assert_eq!(row.kind, "sandbox_agent");
    assert_eq!(row.principal_user_id, fx.user.id);
    assert!(!row.all_access && row.platform && !row.partner);
    assert_eq!(row.token_hash, oxy_auth::token::hash_token(secret));
    assert_eq!(row.legacy_api_key_id, None);
    let stored = grants_of(&fx.db, id).await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].kind, api_token_grants::KIND_APP_SANDBOX);
    assert_eq!(
        (stored[0].org_id, stored[0].app_id),
        (fx.org_id, Some(app.id))
    );
    assert_eq!(
        (stored[0].workspace_id, &stored[0].role_ceiling),
        (None, &None)
    );
}

#[tokio::test]
async fn the_mint_is_audited_on_the_chain_of_every_granted_apps_org() {
    let (fx, app) = staff_with_app().await;
    let other_org = seed_org(&fx.db).await;
    let other_ws = seed_workspace_in(&fx.db, other_org).await;
    let other_app = published_app(&fx.db, other_org, other_ws).await;

    let (id, _) = minted(&fx, &[app.id, other_app.id]).await;

    let rows = audit_rows(&fx.db, "token.created").await;
    let mut orgs: Vec<Option<Uuid>> = rows.iter().map(|r| r.org_id).collect();
    orgs.sort();
    let mut expected = vec![Some(fx.org_id), Some(other_org)];
    expected.sort();
    assert_eq!(orgs, expected, "one row per granted app's org");
    // One event: the rows share an id.
    assert_eq!(rows[0].metadata["event_id"], rows[1].metadata["event_id"]);

    // Each org's row names its own app and grant, and nothing of the other
    // org: not its id, not its app's id.
    let sides = [(fx.org_id, app.id), (other_org, other_app.id)];
    for ((org, own_app), (foreign_org, foreign_app)) in [(sides[0], sides[1]), (sides[1], sides[0])]
    {
        let row = rows
            .iter()
            .find(|row| row.org_id == Some(org))
            .unwrap_or_else(|| panic!("a row on org {org}"));
        assert_eq!(row.actor_user_id, Some(fx.user.id), "audited as the minter");
        assert_eq!(row.target_id.as_deref(), Some(id.to_string().as_str()));
        assert_eq!(row.metadata["token_id"], json!(id));
        assert_eq!(row.metadata["token_kind"], "sandbox_agent");
        assert!(row.metadata["expires_at"].is_string());
        assert_eq!(row.metadata["platform"], json!(true));

        assert_eq!(row.metadata["apps"], json!([own_app]));
        let grants = row.metadata["grants"].as_array().expect("grants");
        assert_eq!(grants.len(), 1, "{}", row.metadata);
        assert_eq!(grants[0]["kind"], "app_sandbox");
        assert_eq!(grants[0]["org_id"], json!(org));
        assert_eq!(grants[0]["app_id"], json!(own_app));

        // The row exactly as it is stored and chained.
        let stored = serde_json::to_string(row).expect("serialise the row");
        for leaked in [foreign_org.to_string(), foreign_app.to_string()] {
            assert!(
                !stored.contains(&leaked),
                "the row on org {org} holds {leaked} of org {foreign_org}"
            );
        }
    }
}

#[tokio::test]
async fn minting_is_session_only_and_a_token_cannot_mint() {
    let (fx, app) = staff_with_app().await;
    let body = mint_body(&[app.id]);

    // No credential at all.
    let (status, _) = call(flat_api(), "POST", "/user/tokens", &[], Some(body.clone())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A staff member's own all-access token, carrying their staff standing.
    let (_, pat) = mint(&fx.db, fx.user.id, Reach::all_access().with_platform()).await;
    let bearer = format!("Bearer {pat}");
    let refused = call(
        flat_api(),
        "POST",
        "/user/tokens",
        &[("authorization", &bearer)],
        Some(body.clone()),
    )
    .await;
    assert_refused(refused, StatusCode::FORBIDDEN, "session_required", "a PAT");

    // A sandbox agent token minting its own successor: it authenticates on
    // `/api`, and the route allow-list answers everything outside the sandbox
    // loop — its minter's token routes included — as not found.
    let (_, sbx) = minted(&fx, &[app.id]).await;
    let bearer = format!("Bearer {sbx}");
    let (status, body) = call(
        flat_api(),
        "POST",
        "/user/tokens",
        &[("authorization", &bearer)],
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let kinds: Vec<api_tokens::Model> = api_tokens::Entity::find()
        .filter(api_tokens::Column::Kind.eq(StoredKind::SandboxAgent.as_str()))
        .all(&fx.db)
        .await
        .unwrap();
    assert_eq!(kinds.len(), 1, "only the session's mint exists");
}

#[tokio::test]
async fn a_caller_without_reach_gets_404_for_the_app_and_learns_nothing() {
    let (fx, app) = staff_with_app().await;
    let other_org = seed_org(&fx.db).await;
    let other_ws = seed_workspace_in(&fx.db, other_org).await;
    let elsewhere = published_app(&fx.db, other_org, other_ws).await;
    let unknown = Uuid::new_v4();

    // Not staff at all — even as the org's owner.
    let (member, member_cookie) = another_session(&fx.db, "member").await;
    join_org(&fx.db, fx.org_id, member.id, OrgRole::Owner).await;
    // An App Operator whose grant reaches the fixture's org only.
    let (operator, operator_cookie) = another_session(&fx.db, "operator").await;
    grant_staff(
        &fx.db,
        operator.email.as_deref().unwrap(),
        PlatformRole::AppOperator,
        &[fx.org_id],
    )
    .await;

    for (cookie, app_id, why) in [
        (&member_cookie, app.id, "an org owner who is not staff"),
        (
            &operator_cookie,
            elsewhere.id,
            "an app outside the grant's scope",
        ),
        (&operator_cookie, unknown, "an app that does not exist"),
        (
            &fx.cookie,
            unknown,
            "an app that does not exist, as a Global Admin",
        ),
    ] {
        let refused = post_as(cookie, "/user/tokens", mint_body(&[app_id])).await;
        assert_eq!(refused.1["app_id"], json!(app_id), "{why}: {}", refused.1);
        assert_refused(refused, StatusCode::NOT_FOUND, "app_not_found", why);
    }

    // One unreachable app refuses the whole mint, naming that app.
    let refused = post_as(
        &operator_cookie,
        "/user/tokens",
        mint_body(&[app.id, elsewhere.id]),
    )
    .await;
    assert_eq!(refused.1["app_id"], json!(elsewhere.id));
    assert_refused(
        refused,
        StatusCode::NOT_FOUND,
        "app_not_found",
        "one of two",
    );

    // An id that is not an id names no app: the same answer, echoed as sent.
    let body = json!({ "name": "agent", "kind": "sandbox_agent", "apps": ["acme/store"] });
    let refused = post_as(&fx.cookie, "/user/tokens", body).await;
    assert_eq!(refused.1["app_id"], "acme/store");
    assert_refused(refused, StatusCode::NOT_FOUND, "app_not_found", "a slug");

    // The operator mints for the app the grant does reach.
    let (status, body) = post_as(&operator_cookie, "/user/tokens", mint_body(&[app.id])).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["token"]["owner"]["id"], json!(operator.id));
}

#[tokio::test]
async fn who_may_mint_is_read_past_the_grant_cache() {
    let (fx, app) = staff_with_app().await;
    // The first mint reads the grant, which leaves it in the 60 s cache.
    minted(&fx, &[app.id]).await;

    app_admins::Entity::delete_many()
        .filter(app_admins::Column::Email.eq(fx.user.email.clone().unwrap()))
        .exec(&fx.db)
        .await
        .expect("take the grant away");

    // No cache invalidation, no wait: the very next mint is refused.
    let refused = post_as(&fx.cookie, "/user/tokens", mint_body(&[app.id])).await;
    assert_refused(
        refused,
        StatusCode::NOT_FOUND,
        "app_not_found",
        "a minter whose grant was just deleted",
    );
}

#[tokio::test]
async fn a_mint_outside_the_limits_answers_400() {
    let (fx, app) = staff_with_app().await;
    let mut six = Vec::new();
    for _ in 0..6 {
        six.push(published_app(&fx.db, fx.org_id, fx.workspace_id).await.id);
    }
    let with = |extra: Value| {
        let mut body = mint_body(&[app.id]);
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        body
    };
    let cases = [
        (mint_body(&[]), "no apps"),
        (mint_body(&six), "six apps"),
        (mint_body(&[app.id, app.id]), "an app named twice"),
        (with(json!({ "expires_in_hours": 0 })), "zero hours"),
        (with(json!({ "expires_in_hours": 169 })), "more than a week"),
        (
            with(json!({ "expires_in_hours": 1.5 })),
            "a fractional lifetime",
        ),
        (with(json!({ "all_access": false })), "all_access"),
        (with(json!({ "platform": true })), "platform"),
        (with(json!({ "partner": false })), "partner"),
        (with(json!({ "grants": [] })), "grants"),
        (with(json!({ "expires_in_days": 1 })), "expires_in_days"),
        (with(json!({ "expires_at": null })), "expires_at"),
        (with(json!({ "name": "" })), "an empty name"),
    ];
    for (body, why) in cases {
        let refused = post_as(&fx.cookie, "/user/tokens", body).await;
        assert!(refused.1["message"].is_string(), "{why}: {}", refused.1);
        assert_refused(
            refused,
            StatusCode::BAD_REQUEST,
            "invalid_sandbox_token",
            why,
        );
    }

    // The limits themselves are inside the range.
    for hours in [1, 168] {
        let (status, body) = post_as(
            &fx.cookie,
            "/user/tokens",
            with(json!({ "expires_in_hours": hours })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{hours} h: {body}");
    }
    let five: Vec<Uuid> = six[..5].to_vec();
    let (status, body) = post_as(&fx.cookie, "/user/tokens", mint_body(&five)).await;
    assert_eq!(status, StatusCode::CREATED, "five apps: {body}");
    assert_eq!(body["token"]["grants"].as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn a_body_with_no_kind_or_personal_still_mints_a_personal_token() {
    let (fx, _app) = staff_with_app().await;
    for body in [
        json!({ "name": "laptop" }),
        json!({ "name": "laptop", "kind": "personal" }),
    ] {
        let (status, minted) = post_as(&fx.cookie, "/user/tokens", body).await;
        assert_eq!(status, StatusCode::CREATED, "{minted}");
        assert_eq!(minted["token"]["kind"], "personal");
        assert_eq!(minted["token"]["all_access"], true);
        assert!(minted["secret"].as_str().unwrap().starts_with("oxy_pat_"));
    }
    // A kind this route does not mint is refused, never minted as personal.
    let (status, body) = post_as(
        &fx.cookie,
        "/user/tokens",
        json!({ "name": "x", "kind": "service_account" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.get("secret").is_none());
}

// ── The minter's routes ──────────────────────────────────────────────────────

#[tokio::test]
async fn the_minter_lists_reads_and_revokes_it_and_nothing_edits_it() {
    let (fx, app) = staff_with_app().await;
    let (id, _) = minted(&fx, &[app.id]).await;
    let (pat_id, _) = mint(&fx.db, fx.user.id, Reach::all_access()).await;

    // Listed beside the personal tokens.
    let (status, list) = get_in_session(&fx, "/user/tokens").await;
    assert_eq!(status, StatusCode::OK);
    let listed: Vec<(String, String)> = list["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["id"].as_str().unwrap().to_string(),
                t["kind"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(
        listed.contains(&(id.to_string(), "sandbox_agent".into())),
        "{list}"
    );
    assert!(
        listed.contains(&(pat_id.to_string(), "personal".into())),
        "{list}"
    );

    let uri = format!("/user/tokens/{id}");
    let (status, one) = get_in_session(&fx, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(one["kind"], "sandbox_agent");
    let (status, _) = get_in_session(&fx, &format!("{uri}/activity")).await;
    assert_eq!(status, StatusCode::OK);

    // Fixed: no rename, no widening, no extend, no new secret.
    let before = pat_row(&fx.db, id).await;
    let edits = [
        ("PATCH", uri.clone(), json!({ "name": "renamed" })),
        ("PATCH", uri.clone(), json!({ "all_access": true })),
        ("POST", format!("{uri}/extend"), json!({ "days": 30 })),
        (
            "POST",
            format!("{uri}/extend"),
            json!({ "expires_at": null }),
        ),
        ("POST", format!("{uri}/regenerate"), json!({})),
    ];
    for (method, uri, body) in edits {
        let refused = call(
            flat_api(),
            method,
            &uri,
            &[("cookie", &fx.cookie)],
            Some(body),
        )
        .await;
        assert_refused(
            refused,
            StatusCode::CONFLICT,
            "sandbox_token_fixed",
            &format!("{method} {uri}"),
        );
    }
    let after = pat_row(&fx.db, id).await;
    assert_eq!(
        (
            &after.name,
            after.expires_at,
            &after.token_hash,
            after.all_access
        ),
        (
            &before.name,
            before.expires_at,
            &before.token_hash,
            before.all_access
        ),
        "nothing about the token moved"
    );

    // Someone else's reads as no token at all.
    let (_, stranger_cookie) = another_session(&fx.db, "stranger").await;
    let (status, _) = call(
        flat_api(),
        "GET",
        &uri,
        &[("cookie", &stranger_cookie)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Revoked by its minter, audited in the app's org with the reason.
    let (status, _) = call(flat_api(), "DELETE", &uri, &[("cookie", &fx.cookie)], None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let revoked = pat_row(&fx.db, id).await;
    assert!(revoked.revoked_at.is_some());
    assert_eq!(revoked.revoke_reason.as_deref(), Some("owner"));
    let audit = audit_rows(&fx.db, "token.revoked").await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].org_id, Some(fx.org_id));
    assert_eq!(audit[0].metadata["reason"], "owner");
    assert_eq!(audit[0].metadata["token_kind"], "sandbox_agent");

    // Still fixed once revoked: the kind is what refuses, not its state.
    let refused = call(
        flat_api(),
        "POST",
        &format!("{uri}/regenerate"),
        &[("cookie", &fx.cookie)],
        Some(json!({})),
    )
    .await;
    assert_refused(
        refused,
        StatusCode::CONFLICT,
        "sandbox_token_fixed",
        "revoked",
    );
}

#[tokio::test]
async fn token_options_tell_staff_the_limits_and_the_apps_they_may_mint_for() {
    let (fx, app) = staff_with_app().await;
    let (status, options) = get_in_session(&fx, "/user/token-options").await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(
        options["sandbox_agent"],
        json!({ "default_hours": 8, "max_hours": 168, "max_apps": 5 })
    );
    let apps = options["sandbox_apps"].as_array().expect("sandbox_apps");
    assert_eq!(apps.len(), 1, "{options}");
    assert_eq!(apps[0]["id"], json!(app.id));
    assert_eq!(apps[0]["org_id"], json!(fx.org_id));
    assert_eq!(apps[0]["org_name"], "Acme");
    assert_eq!(apps[0]["slug"], json!(app.slug));
    assert_eq!(apps[0]["name"], "Token Test App");
    assert!(apps[0]["org_slug"].as_str().unwrap().starts_with("acme-"));

    // A member who is not staff is offered no app, and the limits all the same.
    let (member, cookie) = another_session(&fx.db, "member").await;
    join_org(&fx.db, fx.org_id, member.id, OrgRole::Owner).await;
    let (status, options) = call(
        flat_api(),
        "GET",
        "/user/token-options",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(options["sandbox_apps"], json!([]));
    assert_eq!(options["sandbox_agent"]["max_apps"], 5);
    assert_eq!(options["can_platform"], false);
}
