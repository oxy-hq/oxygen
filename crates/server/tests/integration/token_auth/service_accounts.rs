//! Phase 3: org-owned service accounts and their `oxy_sat_` tokens (design
//! §3.3), driven through the served routers.
//!
//! What a service account must never be is asserted against the real
//! enumerations, not a model of them: the member list route, the seat count
//! the Stripe sync pushes, the app-audience resolution, an invitation, and the
//! identity lookup every login path ends in.

use axum::http::StatusCode;
use entity::org_members::{self, OrgRole};
use entity::users::{self, UserStatus};
use entity::{org_invitations, service_accounts};
use oxy_app::server::api::custom_apps_functions::host::app_audience;
use oxy_app::server::authz::Caller;
use oxy_app::server::authz::loader::load_principal_facts;
use oxy_auth::authenticator::Authenticator;
use oxy_auth::built_in::BuiltInAuthenticator;
use oxy_auth::types::{AuthenticatedUser, Identity};
use oxy_auth::user::UserService;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::stack::{
    flat_api, get_as, join_org, make_staff, published_app, workspace_api, workspace_external,
};
use super::{Fixture, audit_rows, call, fixture, seed_org, seed_user, seed_workspace_in};

// ── Shared with the org-inventory tests ──────────────────────────────────────

/// A browser session for `user`.
pub(crate) async fn session_of(user: &users::Model) -> String {
    let jwt = oxy_app::server::api::auth::create_auth_token(user.clone())
        .await
        .expect("mint a session");
    format!("oxy_session={jwt}")
}

/// One request on the flat tree under a browser session.
pub(crate) async fn in_session(
    cookie: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    call(flat_api(), method, uri, &[("cookie", cookie)], body).await
}

/// One request on the flat tree with a token.
pub(crate) async fn with_token(
    secret: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let bearer = format!("Bearer {secret}");
    call(flat_api(), method, uri, &[("authorization", &bearer)], body).await
}

fn id_of(value: &Value) -> Uuid {
    value["id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .expect("an id")
}

/// The fixture's user as the org's Owner — the admin every test acts as.
pub(crate) async fn admin_fixture() -> Fixture {
    let fx = fixture().await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Owner).await;
    fx
}

fn accounts_uri(org_id: Uuid) -> String {
    format!("/orgs/{org_id}/service-accounts")
}

/// Create a service account through the API, as the fixture's admin.
pub(crate) async fn create_account(fx: &Fixture, name: &str, role: &str) -> Uuid {
    let body = json!({ "name": name, "org_role": role });
    let (status, account) =
        in_session(&fx.cookie, "POST", &accounts_uri(fx.org_id), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "create {name}: {account}");
    id_of(&account)
}

/// Mint a token for the account through the API: `(id, secret)`.
pub(crate) async fn mint_account_token(fx: &Fixture, sa_id: Uuid, body: Value) -> (Uuid, String) {
    let uri = format!("{}/{sa_id}/tokens", accounts_uri(fx.org_id));
    let (status, minted) = in_session(&fx.cookie, "POST", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "mint: {minted}");
    let secret = minted["secret"].as_str().expect("a secret").to_string();
    (id_of(&minted["token"]), secret)
}

/// The status of `method /{workspace}/{route}` on `/api` with a token.
pub(crate) async fn workspace_status(
    secret: &str,
    workspace: Uuid,
    method: &str,
    route: &str,
) -> StatusCode {
    let bearer = format!("Bearer {secret}");
    let uri = format!("/{workspace}/{route}");
    call(
        workspace_api(),
        method,
        &uri,
        &[("authorization", &bearer)],
        None,
    )
    .await
    .0
}

/// A token's `api_tokens` row, by its own id. (`super::token_row` finds a
/// legacy key's mirror by the *key's* id, which no account token has.)
async fn token_row(db: &DatabaseConnection, id: Uuid) -> Option<entity::api_tokens::Model> {
    entity::api_tokens::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
}

async fn account_row(db: &DatabaseConnection, sa_id: Uuid) -> Option<service_accounts::Model> {
    service_accounts::Entity::find_by_id(sa_id)
        .one(db)
        .await
        .unwrap()
}

async fn user_row(db: &DatabaseConnection, id: Uuid) -> users::Model {
    users::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .expect("the user row")
}

/// Flip `disabled_at` behind the server's back — what another pod's write
/// looks like to this one's credential cache.
async fn set_disabled(db: &DatabaseConnection, sa_id: Uuid, disabled: bool) {
    let row = account_row(db, sa_id).await.expect("the account");
    let mut active: service_accounts::ActiveModel = row.into();
    active.disabled_at = ActiveValue::Set(disabled.then(|| chrono::Utc::now().fixed_offset()));
    active.update(db).await.expect("flip disabled_at");
}

// ── What a service account is, and is not ────────────────────────────────────

#[tokio::test]
async fn an_admin_creates_a_service_account_and_the_contract_holds() {
    let fx = admin_fixture().await;
    let uri = accounts_uri(fx.org_id);
    let body =
        json!({ "name": "deploy-bot", "description": " ships the app ", "org_role": "admin" });
    let (status, account) = in_session(&fx.cookie, "POST", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{account}");
    let sa_id = id_of(&account);
    assert_eq!(account["org_id"], fx.org_id.to_string());
    assert_eq!(account["name"], "deploy-bot");
    assert_eq!(account["description"], "ships the app");
    assert_eq!(account["org_role"], "admin");
    assert_eq!(account["created_by"]["id"], fx.user.id.to_string());
    assert_eq!(account["token_count"], 0);
    assert_eq!(account["trust_policy_count"], 0);
    assert!(account["disabled_at"].is_null());

    // `org_role` defaults to member.
    let (status, plain) =
        in_session(&fx.cookie, "POST", &uri, Some(json!({ "name": "reader" }))).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(plain["org_role"], "member");

    // The name is unique in the org, and a slug.
    let (status, taken) = in_session(
        &fx.cookie,
        "POST",
        &uri,
        Some(json!({ "name": "deploy-bot" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(taken["code"], "name_taken");
    for bad in ["Deploy", "a", "ops@acme.com", "ci--bot"] {
        let (status, _) = in_session(&fx.cookie, "POST", &uri, Some(json!({ "name": bad }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }

    let (status, listed) = in_session(&fx.cookie, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = listed["service_accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["deploy-bot", "reader"]);

    let (status, one) = in_session(&fx.cookie, "GET", &format!("{uri}/{sa_id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(one["id"], sa_id.to_string());
    // Another org's admin cannot see it: it reads as not found.
    let other_org = seed_org(&fx.db).await;
    join_org(&fx.db, other_org, fx.user.id, OrgRole::Owner).await;
    let foreign = format!("{}/{sa_id}", accounts_uri(other_org));
    assert_eq!(
        in_session(&fx.cookie, "GET", &foreign, None).await.0,
        StatusCode::NOT_FOUND
    );

    let created = audit_rows(&fx.db, "service_account.created").await;
    assert_eq!(created.len(), 2);
    assert_eq!(created[0].org_id, Some(fx.org_id));
    assert_eq!(
        created[0].target_id.as_deref(),
        Some(sa_id.to_string().as_str())
    );
    assert_eq!(created[0].actor_user_id, Some(fx.user.id));
}

#[tokio::test]
async fn a_service_account_is_a_user_with_no_email_and_no_membership() {
    let fx = admin_fixture().await;
    let sa_id = create_account(&fx, "deploy-bot", "admin").await;

    // A `users` row with a NULL email, and the account row beside it.
    let user = user_row(&fx.db, sa_id).await;
    assert_eq!(user.email, None);
    assert_eq!(user.name, "deploy-bot");
    assert!(!user.label().contains('@'));
    assert!(account_row(&fx.db, sa_id).await.is_some());

    // No `org_members` row — so it is in none of the things that enumerate one.
    let memberships = org_members::Entity::find()
        .filter(org_members::Column::UserId.eq(sa_id))
        .count(&fx.db)
        .await
        .unwrap();
    assert_eq!(memberships, 0);

    // The member list route.
    let (status, members) = in_session(
        &fx.cookie,
        "GET",
        &format!("/orgs/{}/members", fx.org_id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed: Vec<&str> = members
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["user_id"].as_str().unwrap())
        .collect();
    assert_eq!(listed, [fx.user.id.to_string().as_str()]);

    // The seat count the Stripe sync pushes: one person, however many accounts.
    create_account(&fx, "second-bot", "member").await;
    let seats = oxy_billing::service::billable_seats(&fx.db, fx.org_id)
        .await
        .unwrap();
    assert_eq!(seats, 1, "a service account is not a seat");

    // An app's audience — what `ctx.org.people()` may name.
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let audience = app_audience(&fx.db, fx.org_id, app.id).await.unwrap();
    assert!(audience.ids.contains(&fx.user.id));
    assert!(!audience.ids.contains(&sa_id));
    assert!(audience.members.iter().all(|m| m.user_id != sa_id));
}

#[tokio::test]
async fn a_service_account_cannot_be_invited_or_signed_in_to() {
    let fx = admin_fixture().await;
    let sa_id = create_account(&fx, "deploy-bot", "admin").await;

    // An invitation is keyed by address, and the account has none: inviting
    // its name — whatever the route answers — never makes it a member.
    let invite = json!({ "email": "deploy-bot", "role": "member" });
    let uri = format!("/orgs/{}/invitations", fx.org_id);
    let _ = in_session(&fx.cookie, "POST", &uri, Some(invite)).await;
    let memberships = org_members::Entity::find()
        .filter(org_members::Column::UserId.eq(sa_id))
        .count(&fx.db)
        .await
        .unwrap();
    assert_eq!(memberships, 0);
    let invitations = org_invitations::Entity::find()
        .filter(org_invitations::Column::OrgId.eq(fx.org_id))
        .all(&fx.db)
        .await
        .unwrap();
    assert!(
        invitations.iter().all(|i| i.email.contains('@')),
        "no invitation can name a service account: {invitations:?}"
    );

    // Every login path — magic link, Google, Okta, GitHub — ends in a lookup by
    // the address the provider vouched for. None resolves to the account.
    for address in ["deploy-bot", "", "deploy-bot@example.com"] {
        let identity = Identity {
            user_id: None,
            email: address.to_string(),
            name: None,
            picture: None,
        };
        let found = UserService::find_user_by_identity(&identity).await.unwrap();
        assert!(
            found.is_none_or(|u| u.id != sa_id),
            "{address:?} must not resolve to the service account"
        );
    }
    // And it holds no magic-link token to redeem.
    let user = user_row(&fx.db, sa_id).await;
    assert_eq!(user.magic_link_token, None);
    assert!(!user.email_verified);
}

#[tokio::test]
async fn a_service_account_is_never_an_owner() {
    let fx = admin_fixture().await;
    let uri = accounts_uri(fx.org_id);
    let owner = json!({ "name": "root-bot", "org_role": "owner" });
    let (status, refused) = in_session(&fx.cookie, "POST", &uri, Some(owner)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");

    let sa_id = create_account(&fx, "deploy-bot", "admin").await;
    let patch = json!({ "org_role": "owner" });
    let (status, _) = in_session(&fx.cookie, "PATCH", &format!("{uri}/{sa_id}"), Some(patch)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The table itself refuses it: nothing that writes the row can make one.
    let forced = fx
        .db
        .execute_raw(Statement::from_sql_and_values(
            fx.db.get_database_backend(),
            "UPDATE service_accounts SET org_role = 'owner' WHERE user_id = $1",
            [sa_id.into()],
        ))
        .await;
    assert!(forced.is_err(), "the CHECK constraint refuses an owner");
    assert_eq!(account_row(&fx.db, sa_id).await.unwrap().org_role, "admin");

    // An admin account's token does not pass an owner-only route either.
    let (_, secret) = mint_account_token(&fx, sa_id, json!({ "name": "t" })).await;
    let (status, _) = with_token(&secret, "DELETE", &format!("/orgs/{}", fx.org_id), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ── Its tokens ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_service_account_token_works_within_its_grants_and_nowhere_else() {
    let fx = admin_fixture().await;
    let ws2 = seed_workspace_in(&fx.db, fx.org_id).await;
    let org_b = seed_org(&fx.db).await;
    let ws_b = seed_workspace_in(&fx.db, org_b).await;
    let sa_id = create_account(&fx, "deploy-bot", "member").await;

    let body = json!({ "name": "one workspace", "grants": [{ "workspace_id": fx.workspace_id }] });
    let uri = format!("{}/{sa_id}/tokens", accounts_uri(fx.org_id));
    let (status, minted) = in_session(&fx.cookie, "POST", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{minted}");
    let secret = minted["secret"].as_str().unwrap().to_string();
    assert!(secret.starts_with("oxy_sat_"));
    let token = &minted["token"];
    assert_eq!(token["kind"], "service_account");
    assert_eq!(token["owner"]["type"], "service_account");
    assert_eq!(token["owner"]["id"], sa_id.to_string());
    assert_eq!(token["owner"]["label"], "deploy-bot");
    assert_eq!(token["all_access"], false);
    assert_eq!(token["platform"], false);
    assert_eq!(token["partner"], false);
    assert_eq!(token["grants"][0]["role_ceiling"], "member");
    assert_eq!(token["grants"][0]["org_id"], fx.org_id.to_string());

    // Inside the grant: a member — reads and writes, does not administer.
    let ws = fx.workspace_id;
    assert_eq!(
        workspace_status(&secret, ws, "GET", "read").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        workspace_status(&secret, ws, "POST", "write").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        workspace_status(&secret, ws, "POST", "manage").await,
        StatusCode::FORBIDDEN
    );
    let external = call(
        workspace_external(),
        "GET",
        &format!("/{ws}/read"),
        &[("x-api-key", &secret)],
        None,
    )
    .await;
    assert_eq!(
        external.0,
        StatusCode::NO_CONTENT,
        "the same on /external/api"
    );

    // Outside it: 404 — a sibling workspace, the org's own routes, another org.
    assert_eq!(
        workspace_status(&secret, ws2, "GET", "read").await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        workspace_status(&secret, ws_b, "GET", "read").await,
        StatusCode::NOT_FOUND
    );
    for org in [fx.org_id, org_b] {
        let (status, _) = get_as(&secret, &format!("/orgs/{org}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "org {org}");
    }

    // It describes itself as what it is.
    let (status, me) = get_as(&secret, "/auth/token").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["kind"], "service_account");
    assert_eq!(me["owner"]["type"], "service_account");

    // The mint is in the org's chain, attributed to the admin who did it.
    let created = audit_rows(&fx.db, "token.created").await;
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].org_id, Some(fx.org_id));
    assert_eq!(created[0].metadata["token_kind"], "service_account");
    assert_eq!(created[0].metadata["service_account"], "deploy-bot");
    assert_eq!(created[0].actor_user_id, Some(fx.user.id));
}

#[tokio::test]
async fn omitted_grants_mean_the_whole_org_at_the_accounts_role() {
    let fx = admin_fixture().await;
    let ws2 = seed_workspace_in(&fx.db, fx.org_id).await;
    let org_b = seed_org(&fx.db).await;
    let admin = create_account(&fx, "admin-bot", "admin").await;
    let member = create_account(&fx, "member-bot", "member").await;

    let (id, secret) = mint_account_token(&fx, admin, json!({ "name": "whole org" })).await;
    let row = token_row(&fx.db, id).await.expect("the token row");
    assert_eq!(row.kind, "service_account");
    assert_eq!(row.principal_user_id, admin);
    assert_eq!(row.created_by, Some(fx.user.id));
    assert!(!row.all_access && !row.platform && !row.partner);

    // An admin of the org: every workspace, and the org's admin routes.
    for ws in [fx.workspace_id, ws2] {
        assert_eq!(
            workspace_status(&secret, ws, "POST", "manage").await,
            StatusCode::NO_CONTENT
        );
    }
    let members_uri = format!("/orgs/{}/members", fx.org_id);
    assert_eq!(get_as(&secret, &members_uri).await.0, StatusCode::OK);
    assert_eq!(
        get_as(&secret, &accounts_uri(fx.org_id)).await.0,
        StatusCode::OK
    );
    // And no other org.
    assert_eq!(
        get_as(&secret, &format!("/orgs/{org_b}")).await.0,
        StatusCode::NOT_FOUND
    );

    // A member account's whole-org token is a member everywhere in it.
    let (_, plain) =
        mint_account_token(&fx, member, json!({ "name": "whole org", "grants": [] })).await;
    assert_eq!(
        workspace_status(&plain, ws2, "POST", "write").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        workspace_status(&plain, ws2, "POST", "manage").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(get_as(&plain, &members_uri).await.0, StatusCode::OK);
    assert_eq!(
        get_as(&plain, &accounts_uri(fx.org_id)).await.0,
        StatusCode::FORBIDDEN,
        "a member account is no org admin"
    );
}

#[tokio::test]
async fn a_grant_above_the_accounts_role_is_refused_at_mint() {
    let fx = admin_fixture().await;
    let org_b = seed_org(&fx.db).await;
    let ws_b = seed_workspace_in(&fx.db, org_b).await;
    let member = create_account(&fx, "member-bot", "member").await;
    let admin = create_account(&fx, "admin-bot", "admin").await;
    let mint = |sa: Uuid, grants: Value| {
        let cookie = fx.cookie.clone();
        let uri = format!("{}/{sa}/tokens", accounts_uri(fx.org_id));
        async move {
            let body = json!({ "name": "t", "grants": grants });
            in_session(&cookie, "POST", &uri, Some(body)).await.0
        }
    };

    assert_eq!(
        mint(member, json!([{ "role_ceiling": "admin" }])).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        mint(member, json!([{ "role_ceiling": "member" }])).await,
        StatusCode::CREATED
    );
    assert_eq!(
        mint(admin, json!([{ "role_ceiling": "admin" }])).await,
        StatusCode::CREATED
    );
    // `owner` is never valid, whatever the account is.
    for sa in [member, admin] {
        assert_eq!(
            mint(sa, json!([{ "role_ceiling": "owner" }])).await,
            StatusCode::BAD_REQUEST
        );
    }
    // Every grant is in the account's own org.
    assert_eq!(
        mint(admin, json!([{ "org_id": org_b }])).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        mint(admin, json!([{ "workspace_id": ws_b }])).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        mint(
            admin,
            json!([{ "kind": "app_publish", "app_id": Uuid::new_v4() }])
        )
        .await,
        StatusCode::BAD_REQUEST
    );

    // Lowering the account's role caps the tokens it already has.
    let (_, secret) = mint_account_token(&fx, admin, json!({ "name": "was admin" })).await;
    let manage = || workspace_status(&secret, fx.workspace_id, "POST", "manage");
    assert_eq!(manage().await, StatusCode::NO_CONTENT);
    let uri = format!("{}/{admin}", accounts_uri(fx.org_id));
    let (status, _) = in_session(
        &fx.cookie,
        "PATCH",
        &uri,
        Some(json!({ "org_role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        manage().await,
        StatusCode::FORBIDDEN,
        "the role is the ceiling"
    );
    assert_eq!(
        workspace_status(&secret, fx.workspace_id, "POST", "write").await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn a_token_is_extended_regenerated_and_revoked_by_an_admin() {
    let fx = admin_fixture().await;
    let sa_id = create_account(&fx, "deploy-bot", "member").await;
    let body = json!({ "name": "release", "expires_in_days": 30 });
    let (id, secret) = mint_account_token(&fx, sa_id, body).await;
    let base = format!("{}/{sa_id}/tokens", accounts_uri(fx.org_id));
    let read = || workspace_status(&secret, fx.workspace_id, "GET", "read");
    assert_eq!(read().await, StatusCode::NO_CONTENT);

    let (status, listed) = in_session(&fx.cookie, "GET", &base, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["tokens"].as_array().unwrap().len(), 1);
    assert_eq!(listed["tokens"][0]["id"], id.to_string());

    let extend = format!("{base}/{id}/extend");
    let (status, extended) = in_session(
        &fx.cookie,
        "POST",
        &extend,
        Some(json!({ "expires_at": null })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{extended}");
    assert!(extended["expires_at"].is_null());

    let (status, regenerated) =
        in_session(&fx.cookie, "POST", &format!("{base}/{id}/regenerate"), None).await;
    assert_eq!(status, StatusCode::OK, "{regenerated}");
    let fresh = regenerated["secret"].as_str().unwrap().to_string();
    assert!(fresh.starts_with("oxy_sat_") && fresh != secret);
    assert_eq!(regenerated["token"]["id"], id.to_string());
    assert_eq!(
        read().await,
        StatusCode::UNAUTHORIZED,
        "the old secret is dead"
    );
    assert_eq!(
        workspace_status(&fresh, fx.workspace_id, "GET", "read").await,
        StatusCode::NO_CONTENT
    );

    let (status, activity) =
        in_session(&fx.cookie, "GET", &format!("{base}/{id}/activity"), None).await;
    assert_eq!(status, StatusCode::OK);
    let actions: Vec<&str> = activity["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert_eq!(
        actions,
        ["token.regenerated", "token.extended", "token.created"]
    );

    let (status, _) = in_session(&fx.cookie, "DELETE", &format!("{base}/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        workspace_status(&fresh, fx.workspace_id, "GET", "read").await,
        StatusCode::UNAUTHORIZED
    );
    // Nothing more happens to a revoked token.
    let (status, refused) =
        in_session(&fx.cookie, "POST", &extend, Some(json!({ "days": 30 }))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["code"], "revoked");
    for action in ["token.extended", "token.regenerated", "token.revoked"] {
        let rows = audit_rows(&fx.db, action).await;
        assert_eq!(rows.len(), 1, "{action}");
        assert_eq!(rows[0].org_id, Some(fx.org_id), "{action}");
        assert_eq!(
            rows[0].metadata["token_kind"], "service_account",
            "{action}"
        );
    }

    // A token of another account is not reachable through this one.
    let other = create_account(&fx, "other-bot", "member").await;
    let (other_token, _) = mint_account_token(&fx, other, json!({ "name": "t" })).await;
    let (status, _) =
        in_session(&fx.cookie, "DELETE", &format!("{base}/{other_token}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── Disabling and deleting ───────────────────────────────────────────────────

#[tokio::test]
async fn disabling_the_account_stops_its_tokens_at_once() {
    let fx = admin_fixture().await;
    let sa_id = create_account(&fx, "deploy-bot", "member").await;
    let (_, secret) = mint_account_token(&fx, sa_id, json!({ "name": "t" })).await;
    let read = || workspace_status(&secret, fx.workspace_id, "GET", "read");
    // Twice, so the second is served from the credential cache.
    assert_eq!(read().await, StatusCode::NO_CONTENT);
    assert_eq!(read().await, StatusCode::NO_CONTENT);

    // Disabled by ANOTHER pod — a write this process's cache never hears of.
    // The token must stop on the very next request, not at the cache's TTL.
    set_disabled(&fx.db, sa_id, true).await;
    assert_eq!(read().await, StatusCode::UNAUTHORIZED);
    set_disabled(&fx.db, sa_id, false).await;
    assert_eq!(
        read().await,
        StatusCode::NO_CONTENT,
        "and it comes back when re-enabled"
    );

    // Through the API.
    let uri = format!("{}/{sa_id}", accounts_uri(fx.org_id));
    let (status, disabled) =
        in_session(&fx.cookie, "PATCH", &uri, Some(json!({ "disabled": true }))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(disabled["disabled_at"].is_string());
    assert_eq!(read().await, StatusCode::UNAUTHORIZED);
    let rows = audit_rows(&fx.db, "service_account.disabled").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].org_id, Some(fx.org_id));

    let (status, enabled) = in_session(
        &fx.cookie,
        "PATCH",
        &uri,
        Some(json!({ "disabled": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(enabled["disabled_at"].is_null());
    assert_eq!(read().await, StatusCode::NO_CONTENT);
    assert_eq!(audit_rows(&fx.db, "service_account.updated").await.len(), 1);
}

#[tokio::test]
async fn deleting_the_account_revokes_its_tokens() {
    let fx = admin_fixture().await;
    let sa_id = create_account(&fx, "deploy-bot", "admin").await;
    let (id, secret) = mint_account_token(&fx, sa_id, json!({ "name": "t" })).await;
    let read = || workspace_status(&secret, fx.workspace_id, "GET", "read");
    assert_eq!(read().await, StatusCode::NO_CONTENT);

    let uri = format!("{}/{sa_id}", accounts_uri(fx.org_id));
    let (status, _) = in_session(&fx.cookie, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(read().await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        in_session(&fx.cookie, "GET", &uri, None).await.0,
        StatusCode::NOT_FOUND
    );

    let row = token_row(&fx.db, id)
        .await
        .expect("the token row stays, revoked");
    assert!(row.revoked_at.is_some());
    assert_eq!(row.revoked_by, Some(fx.user.id));
    assert!(account_row(&fx.db, sa_id).await.is_none());
    // The `users` row stays for what references it, and can authenticate nowhere.
    assert_eq!(user_row(&fx.db, sa_id).await.status, UserStatus::Deleted);
    assert_eq!(audit_rows(&fx.db, "service_account.deleted").await.len(), 1);
    assert_eq!(audit_rows(&fx.db, "token.revoked").await.len(), 1);

    // The name is free again.
    create_account(&fx, "deploy-bot", "member").await;
}

// ── No standing, and no acting as a person ───────────────────────────────────

/// The caller a request carrying `secret` is, as the served auth stack builds it.
async fn caller_of(fx: &Fixture, secret: &str, sa_id: Uuid) -> Caller {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", format!("Bearer {secret}").parse().unwrap());
    let (identity, credential) = BuiltInAuthenticator::new()
        .authenticate_with_credential(&headers)
        .await
        .expect("the token authenticates");
    assert_eq!(identity.user_id, Some(sa_id));
    assert_eq!(
        identity.email, "",
        "an account's identity carries no address"
    );
    let user = AuthenticatedUser::from(user_row(&fx.db, sa_id).await).with_credential(credential);
    Caller::from_user(&user)
}

#[tokio::test]
async fn a_service_account_holds_no_platform_or_partner_standing() {
    let fx = admin_fixture().await;
    let sa_id = create_account(&fx, "deploy-bot", "admin").await;
    let (_, secret) = mint_account_token(&fx, sa_id, json!({ "name": "t" })).await;
    // Somebody later adds a staff grant keyed to the account's name. It must
    // reach nothing: the account has no address for it to match.
    make_staff(&fx.db, "deploy-bot").await;

    assert_eq!(
        get_as(&secret, "/admin/orgs-meta").await.0,
        StatusCode::FORBIDDEN,
        "/api/admin/* is refused"
    );
    let caller = caller_of(&fx, &secret, sa_id).await;
    assert!(caller.is_service_account());
    assert!(!caller.carries_platform() && !caller.carries_partner());
    let facts = load_principal_facts(&fx.db, &caller).await.expect("facts");
    assert!(!facts.is_staff() && !facts.is_root() && !facts.is_partner());
    assert!(facts.member_orgs.is_empty(), "standing is not membership");
    assert_eq!(
        facts.service_account.map(|a| (a.org_id, a.admin)),
        Some((fx.org_id, true))
    );
}

#[tokio::test]
async fn surfaces_that_act_as_the_caller_refuse_a_service_account() {
    let fx = admin_fixture().await;
    let sa_id = create_account(&fx, "deploy-bot", "admin").await;
    let (_, secret) = mint_account_token(&fx, sa_id, json!({ "name": "t" })).await;
    let ws = fx.workspace_id;
    // The flat routes that answer for "me": a grant-bound token — which a
    // service-account token always is — gets 404, as for any unknown route.
    for uri in [
        format!("/airhouse/me/connection?workspace_id={ws}"),
        format!("/airhouse/me/credentials?workspace_id={ws}"),
        format!("/oltp/me/connection?workspace_id={ws}"),
        "/work".to_string(),
        "/chat/channels".to_string(),
        "/notifications".to_string(),
        "/invitations/mine".to_string(),
        "/user/tokens".to_string(),
    ] {
        let (status, _) = get_as(&secret, &uri).await;
        assert!(
            matches!(status, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN),
            "{uri}: {status}"
        );
        assert_ne!(status, StatusCode::OK, "{uri}");
    }
}

// ── Who may call the routes ──────────────────────────────────────────────────

/// Every org API-access route, with a body that would be valid.
fn api_access_routes(
    org: Uuid,
    sa: Uuid,
    token: Uuid,
) -> Vec<(&'static str, String, Option<Value>)> {
    let accounts = accounts_uri(org);
    vec![
        ("GET", accounts.clone(), None),
        (
            "POST",
            accounts.clone(),
            Some(json!({ "name": "another-bot" })),
        ),
        ("GET", format!("{accounts}/{sa}"), None),
        (
            "PATCH",
            format!("{accounts}/{sa}"),
            Some(json!({ "description": "x" })),
        ),
        ("DELETE", format!("{accounts}/{sa}"), None),
        ("GET", format!("{accounts}/{sa}/tokens"), None),
        (
            "POST",
            format!("{accounts}/{sa}/tokens"),
            Some(json!({ "name": "t" })),
        ),
        (
            "POST",
            format!("{accounts}/{sa}/tokens/{token}/extend"),
            Some(json!({ "days": 7 })),
        ),
        (
            "POST",
            format!("{accounts}/{sa}/tokens/{token}/regenerate"),
            None,
        ),
        ("DELETE", format!("{accounts}/{sa}/tokens/{token}"), None),
        (
            "GET",
            format!("{accounts}/{sa}/tokens/{token}/activity"),
            None,
        ),
        ("GET", format!("/orgs/{org}/tokens"), None),
        ("GET", format!("/orgs/{org}/tokens/{token}/activity"), None),
        (
            "POST",
            format!("/orgs/{org}/tokens/{token}/revoke-grant"),
            None,
        ),
    ]
}

#[tokio::test]
async fn a_non_admin_gets_403_on_every_org_api_access_route() {
    let fx = admin_fixture().await;
    let sa_id = create_account(&fx, "deploy-bot", "admin").await;
    let (token, _) = mint_account_token(&fx, sa_id, json!({ "name": "t" })).await;
    let member = seed_user(&fx.db, "member").await;
    join_org(&fx.db, fx.org_id, member.id, OrgRole::Member).await;
    let cookie = session_of(&member).await;

    for (method, uri, body) in api_access_routes(fx.org_id, sa_id, token) {
        let (status, _) = in_session(&cookie, method, &uri, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
    }
    // Nothing happened: the account and its token are as they were.
    assert!(account_row(&fx.db, sa_id).await.is_some());
    assert!(token_row(&fx.db, token).await.unwrap().revoked_at.is_none());
}

#[tokio::test]
async fn a_token_cannot_manage_api_access() {
    let fx = admin_fixture().await;
    let sa_id = create_account(&fx, "deploy-bot", "admin").await;
    let (token, account_secret) = mint_account_token(&fx, sa_id, json!({ "name": "t" })).await;
    // An admin's own all-access token, and the admin service account's token.
    let (_, personal) =
        super::stack::mint(&fx.db, fx.user.id, super::stack::Reach::all_access()).await;

    for secret in [&personal, &account_secret] {
        for (method, uri, body) in api_access_routes(fx.org_id, sa_id, token) {
            let (status, answer) = with_token(secret, method, &uri, body).await;
            if method == "GET" {
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "{method} {uri}: reads are open to an admin token"
                );
            } else {
                assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
                assert_eq!(answer["code"], "session_required", "{method} {uri}");
            }
        }
    }
    assert!(account_row(&fx.db, sa_id).await.is_some());
    assert!(token_row(&fx.db, token).await.unwrap().revoked_at.is_none());
}

#[tokio::test]
async fn what_a_service_account_does_is_audited_as_the_account() {
    let fx = admin_fixture().await;
    let sa_id = create_account(&fx, "deploy-bot", "admin").await;
    let (token, secret) = mint_account_token(&fx, sa_id, json!({ "name": "release" })).await;

    // It revokes itself — the one mutation a token may perform, on itself.
    let (status, _) = with_token(&secret, "DELETE", "/auth/token", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let rows = audit_rows(&fx.db, "token.revoked").await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.org_id, Some(fx.org_id));
    assert_eq!(row.actor_user_id, Some(sa_id));
    assert_eq!(row.actor_email, "deploy-bot");
    assert!(!row.actor_email.contains('@'));
    assert_eq!(row.actor_type, "api_key");
    assert_eq!(row.metadata["token_id"], token.to_string());
    assert_eq!(row.metadata["token_kind"], "service_account");
    assert_eq!(
        workspace_status(&secret, fx.workspace_id, "GET", "read").await,
        StatusCode::UNAUTHORIZED
    );
}
