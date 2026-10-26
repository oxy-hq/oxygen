//! A GitHub App installation attaches to an org only for an **org admin** who
//! can show the installation is **theirs** (#3753).
//!
//! Once a namespace is registered, the org lists and reads its repositories
//! with an installation token the Oxy App mints. So whoever attaches one decides
//! which repositories an org can read, and two things have to hold:
//!
//! - **Who:** an owner or admin of the org — the `OrgAdmin` door, asked the same
//!   way on the org route and on the two per-user routes that name the org in a
//!   query or a signed `state` instead of the path.
//! - **Which:** an installation in the caller's own `GET /user/installations`,
//!   read with the GitHub account they connected — and, for an installation on
//!   a personal account, that account must be the caller's own login. Anything
//!   else answers exactly as a missing installation does, so an id cannot be
//!   probed.
//!
//! And the signed `state` names the person it was minted for: a state carried
//! to someone else is refused, whichever flow it starts.
//!
//! Drives the real `oxy_api_github::routes()` (the org middleware included)
//! against a per-test database. The auth layer is stood in for by putting the
//! caller in the request's extensions, as it would. GitHub is a wiremock server
//! behind `GITHUB_API_URL`; nothing here reaches github.com.
//!
//! This module holds the fixtures those cases share: a per-test database
//! with one org, people seeded into it, and a wiremock GitHub.

use axum::Extension;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use entity::org_members::OrgRole;
use entity::{git_namespaces, github_accounts, org_members, organizations, users};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::{header as header_is, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::test_db;

const ORIGIN: &str = "http://localhost:5173";

pub struct Fixture {
    pub db: DatabaseConnection,
    pub github: MockServer,
    pub org: Uuid,
}

pub async fn fixture() -> Fixture {
    let db = test_db().await;
    let github = MockServer::start().await;
    // SAFETY: nextest runs each test in its own process (`test_db` asserts
    // it), and no request has run yet.
    unsafe {
        std::env::set_var("GITHUB_API_URL", github.uri());
        std::env::set_var("GITHUB_STATE_SECRET", "test-state-secret");
        std::env::set_var("GITHUB_APP_SLUG", "oxy-test");
        std::env::set_var("GITHUB_CLIENT_ID", "test-client");
        // Nothing here may reach github.com: without these the App credential
        // and the OAuth exchange fail before they send anything.
        std::env::remove_var("GITHUB_APP_ID");
        std::env::remove_var("GITHUB_APP_PRIVATE_KEY");
        std::env::remove_var("GITHUB_CLIENT_SECRET");
    }
    let org = seed_org(&db).await;
    Fixture { db, github, org }
}

async fn seed_org(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set("Acme".into()),
        slug: ActiveValue::Set(format!("acme-{}", &id.simple().to_string()[..8])),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    id
}

/// A person, a member of `org` with `role` when one is given.
pub async fn seed_person(
    db: &DatabaseConnection,
    org: Uuid,
    role: Option<OrgRole>,
) -> AuthenticatedUser {
    let id = Uuid::new_v4();
    let user = users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(format!("p-{}@acme.test", id.simple()))),
        name: ActiveValue::Set("Pat".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    if let Some(role) = role {
        let now = chrono::Utc::now().fixed_offset();
        org_members::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            org_id: ActiveValue::Set(org),
            user_id: ActiveValue::Set(id),
            role: ActiveValue::Set(role),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        }
        .insert(db)
        .await
        .expect("seed membership");
    }
    AuthenticatedUser {
        id: user.id,
        email: user.email,
        name: user.name,
        picture: None,
        status: user.status,
        credential: None,
    }
}

pub async fn set_role(db: &DatabaseConnection, org: Uuid, user: Uuid, role: OrgRole) {
    let row = org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(org))
        .filter(org_members::Column::UserId.eq(user))
        .one(db)
        .await
        .unwrap()
        .expect("membership");
    let mut active: org_members::ActiveModel = row.into();
    active.role = ActiveValue::Set(role);
    active.update(db).await.expect("change role");
}

/// The GitHub login [`connect_github`] stores for the holder of `token`.
pub fn login_of(token: &str) -> String {
    format!("gh-{token}")
}

/// The caller's connected GitHub account, holding `token`, signed in as
/// [`login_of`]`(token)`.
pub async fn connect_github(db: &DatabaseConnection, user: Uuid, token: &str) {
    let now = chrono::Utc::now().fixed_offset();
    github_accounts::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        user_id: ActiveValue::Set(user),
        github_login: ActiveValue::Set(login_of(token)),
        oauth_token: ActiveValue::Set(token.to_string()),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed github account");
}

/// An installation on the GitHub organization `acct-{id}`.
pub fn installation(id: i64) -> Value {
    json!({
        "id": id,
        "account": { "login": format!("acct-{id}"), "type": "Organization", "id": id * 10 },
        "app_slug": "oxy-test",
    })
}

/// An installation on the personal GitHub account `login`.
pub fn user_installation(id: i64, login: &str) -> Value {
    json!({
        "id": id,
        "account": { "login": login, "type": "User", "id": id * 10 },
        "app_slug": "oxy-test",
    })
}

pub fn installations_page(list: Vec<Value>) -> Value {
    json!({ "total_count": list.len(), "installations": list })
}

/// `GET /user/installations` as the holder of `token` sees it: one
/// organization installation per id.
pub async fn github_lists(github: &MockServer, token: &str, ids: &[i64]) {
    let list = ids.iter().copied().map(installation).collect();
    github_lists_installations(github, token, list).await;
}

/// `GET /user/installations` as the holder of `token` sees it.
pub async fn github_lists_installations(github: &MockServer, token: &str, list: Vec<Value>) {
    let page = ResponseTemplate::new(200).set_body_json(installations_page(list));
    github_answers(github, token, page).await;
}

/// `GET /user/installations` for the holder of `token` answers `response`.
pub async fn github_answers(github: &MockServer, token: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path("/user/installations"))
        .and(header_is(
            "authorization",
            format!("Bearer {token}").as_str(),
        ))
        .respond_with(response)
        .mount(github)
        .await;
}

/// The GitHub surface as `user`'s requests reach it, after authentication.
fn routes_as(user: &AuthenticatedUser) -> Router {
    oxy_api_github::routes()
        .layer(Extension(user.clone()))
        .with_state(oxy_app::surface::bare_app_state())
}

pub async fn call(user: &AuthenticatedUser, req: Request<Body>) -> (StatusCode, Value) {
    let resp = routes_as(user).oneshot(req).await.expect("router answers");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

pub async fn attach(
    user: &AuthenticatedUser,
    org: Uuid,
    installation_id: i64,
) -> (StatusCode, Value) {
    call(
        user,
        post_json(
            &format!("/orgs/{org}/github/namespaces/installation"),
            json!({ "installation_id": installation_id }),
        ),
    )
    .await
}

pub async fn install_url(user: &AuthenticatedUser, org: Uuid) -> (StatusCode, Value) {
    call(
        user,
        get(&format!(
            "/user/github/installations/new-url?org_id={org}&origin={ORIGIN}"
        )),
    )
    .await
}

pub async fn oauth_url(user: &AuthenticatedUser, org: Uuid) -> (StatusCode, Value) {
    call(
        user,
        get(&format!(
            "/user/github/account/oauth-url?org_id={org}&origin={ORIGIN}"
        )),
    )
    .await
}

/// The `state` GitHub would hand back, out of a URL a mint route returned.
pub fn state_of(body: &Value) -> String {
    let url = url::Url::parse(body["url"].as_str().expect("a url")).expect("parses");
    url.query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .expect("the url carries a state")
}

pub async fn complete(user: &AuthenticatedUser, body: Value) -> (StatusCode, Value) {
    call(user, post_json("/user/github/callback", body)).await
}

pub async fn namespaces_of(db: &DatabaseConnection, org: Uuid) -> Vec<git_namespaces::Model> {
    git_namespaces::Entity::find()
        .filter(git_namespaces::Column::OrgId.eq(org))
        .all(db)
        .await
        .unwrap()
}
