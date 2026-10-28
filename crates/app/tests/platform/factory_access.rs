//! Who may use the Oxygen Factory — end to end through the real
//! `workspace_middleware` over a per-test database.
//!
//! The Factory is for Oxy staff, the org's owners and admins, and the members an
//! operator granted a workspace to; a plain org membership or a workspace role does
//! not mean it. With the switch (`apps_first_experience`) on, a Factory route
//! refuses a plain customer member with `403 factory_access_required`, and lets in
//! the org's admin and owner, a granted member and Oxy staff; a route a customer's
//! surfaces call stays open to all of them. With the switch off nothing changes for
//! anyone.
//!
//! The routes are mounted at the paths the real router mounts them at, with stub
//! handlers: what is under test is the gate and the table it reads, not what a
//! handler does. `oxy-route-catalog`'s `factory_routes` test is what ties the table to
//! the routes that are really mounted.

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use entity::org_members::OrgRole;
use entity::users::UserStatus;
use entity::workspaces::WorkspaceStatus;
use entity::{organizations, users, workspace_factory_grants, workspaces};
use oxy_app::server::api::middlewares::workspace_context::workspace_middleware;
use oxy_app::server::factory_access::{APPS_FIRST_EXPERIENCE, FactoryAccess};
use oxy_app::server::feature_flags::cache;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const STAFF: &str = "staff@oxy.test";

struct Fx {
    ws: Uuid,
    member: AuthenticatedUser,
    admin: AuthenticatedUser,
    owner: AuthenticatedUser,
    granted: AuthenticatedUser,
    /// An Oxy staff member who is also a plain member of the org.
    staff: AuthenticatedUser,
    /// A member granted ANOTHER workspace of the same org.
    granted_elsewhere: AuthenticatedUser,
}

async fn user(db: &DatabaseConnection, org: Uuid, email: &str, role: OrgRole) -> AuthenticatedUser {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(email.into())),
        name: ActiveValue::Set(email.into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    entity::org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(id),
        role: ActiveValue::Set(role),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed membership");
    AuthenticatedUser {
        id,
        email: Some(email.into()),
        name: email.into(),
        picture: None,
        status: UserStatus::Active,
        credential: None,
    }
}

async fn workspace(db: &DatabaseConnection, org: Uuid) -> Uuid {
    let ws = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(ws),
        name: ActiveValue::Set("ws".into()),
        org_id: ActiveValue::Set(Some(org)),
        // No path: the middleware authorizes and builds no manager.
        path: ActiveValue::Set(None),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    ws
}

async fn grant(db: &DatabaseConnection, ws: Uuid, user: &AuthenticatedUser, by: Uuid) {
    workspace_factory_grants::ActiveModel {
        workspace_id: ActiveValue::Set(ws),
        user_id: ActiveValue::Set(user.id),
        granted_by: ActiveValue::Set(Some(by)),
        granted_at: ActiveValue::Set(chrono::Utc::now().fixed_offset()),
    }
    .insert(db)
    .await
    .expect("seed grant");
}

/// Seed the org, two workspaces, and one caller of each shape; start the flag cache
/// with the switch `on` or off.
async fn setup(on: bool) -> Fx {
    let db = crate::common::test_db().await;
    // SAFETY: nextest runs each test in its own process; set before any request.
    unsafe {
        std::env::set_var("OXY_OWNER", STAFF);
    }
    // A serve replica: no working copy, so the middleware attaches no manager.
    oxy::workspace_fs_probe::set_process_owns_workspace_files(false);

    let now = chrono::Utc::now().fixed_offset();
    let org = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org),
        name: ActiveValue::Set("acme".into()),
        slug: ActiveValue::Set(format!("acme-{}", org.simple())),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(&db)
    .await
    .expect("seed org");

    let ws = workspace(&db, org).await;
    let other_ws = workspace(&db, org).await;
    let fx = Fx {
        ws,
        member: user(&db, org, "member@acme.test", OrgRole::Member).await,
        admin: user(&db, org, "admin@acme.test", OrgRole::Admin).await,
        owner: user(&db, org, "owner@acme.test", OrgRole::Owner).await,
        granted: user(&db, org, "granted@acme.test", OrgRole::Member).await,
        staff: user(&db, org, STAFF, OrgRole::Member).await,
        granted_elsewhere: user(&db, org, "elsewhere@acme.test", OrgRole::Member).await,
    };
    grant(&db, ws, &fx.granted, fx.staff.id).await;
    grant(&db, other_ws, &fx.granted_elsewhere, fx.staff.id).await;

    cache::init().await.expect("feature flag cache init");
    cache::set(APPS_FIRST_EXPERIENCE, on);
    fx
}

async fn stub() -> Json<Value> {
    Json(json!({ "ran": true }))
}

async fn meta(FactoryAccess(factory_access): FactoryAccess) -> Json<Value> {
    Json(json!({ "factory_access": factory_access }))
}

/// The workspace tree as the real router mounts it, at the paths it mounts them,
/// behind the real middleware — nested at `prefix` the way the main API
/// (`/api`) and the external API (`/external/api`) are.
fn app(user: &AuthenticatedUser, prefix: &str) -> Router {
    let state = oxy_app::server::router::bare_app_state();
    let inner = Router::new()
        // The Factory's.
        .route("/files/{pathb64}", post(stub))
        .route("/files", get(stub))
        .route("/compile", post(stub))
        .route("/databases", get(stub).post(stub))
        .route("/not-a-route-anyone-classified", get(stub))
        // A customer's.
        .route("/threads", get(stub).post(stub))
        .route("/analytics/runs", post(stub))
        .route("/meta", get(meta))
        // The world model: its graph is a customer's (the home's preview and the
        // read-only page), what explores it is the Factory's.
        .route("/semantic/world-model", get(stub))
        .route("/semantic/world-model/instances", get(stub))
        .route("/semantic/world-model/filter-counts", post(stub))
        // The working copy's git state and config status: the Factory's since the
        // web app stopped asking for them for customers.
        .route("/git-state", get(stub))
        .route("/status", get(stub))
        .route("/onboarding-readiness", get(stub))
        .route("/onboarding/github-setup", get(stub))
        .layer(middleware::from_fn_with_state(state, workspace_middleware));
    Router::new()
        .nest(prefix, Router::new().nest("/{workspace_id}", inner))
        .layer(Extension(user.clone()))
}

struct Reply {
    status: StatusCode,
    body: Value,
}

async fn call(user: &AuthenticatedUser, method: &str, uri: String) -> Reply {
    let prefix = if uri.starts_with("/external/api/") {
        "/external/api"
    } else {
        "/api"
    };
    let resp = app(user, prefix)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    Reply {
        status,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

fn api(fx: &Fx, route: &str) -> String {
    format!("/api/{}{route}", fx.ws)
}

fn assert_refused(r: &Reply, who: &str) {
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{who}: {}", r.body);
    assert_eq!(r.body["code"], "factory_access_required", "{who}");
    assert_eq!(r.body["error"], "factory_access_required", "{who}");
}

fn assert_through(r: &Reply, who: &str) {
    assert_eq!(r.status, StatusCode::OK, "{who}: {}", r.body);
    assert_eq!(r.body["ran"], true, "{who}");
}

// ── Switch on ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn with_the_switch_on_a_plain_customer_member_is_refused_a_factory_route() {
    let fx = setup(true).await;
    for (who, user) in [("member", &fx.member)] {
        for (method, route) in [
            ("POST", "/files/abc"),
            ("GET", "/files"),
            ("POST", "/compile"),
            ("POST", "/databases"),
        ] {
            let r = call(user, method, api(&fx, route)).await;
            assert_refused(&r, &format!("{who} {method} {route}"));
            assert_eq!(r.body["workspace_id"], fx.ws.to_string());
        }
    }
}

#[tokio::test]
async fn with_the_switch_on_org_officers_a_granted_member_and_oxy_staff_get_through() {
    let fx = setup(true).await;
    for (who, user) in [
        ("org admin without a grant", &fx.admin),
        ("org owner without a grant", &fx.owner),
        ("granted member", &fx.granted),
        ("staff", &fx.staff),
    ] {
        for (method, route) in [
            ("POST", "/files/abc"),
            ("GET", "/files"),
            ("POST", "/compile"),
            ("POST", "/databases"),
        ] {
            let r = call(user, method, api(&fx, route)).await;
            assert_through(&r, &format!("{who} {method} {route}"));
        }
    }
}

#[tokio::test]
async fn with_the_switch_on_a_customer_route_still_works_for_a_plain_member() {
    let fx = setup(true).await;
    for (who, user) in [
        ("member", &fx.member),
        ("admin", &fx.admin),
        ("owner", &fx.owner),
        ("granted", &fx.granted),
        ("staff", &fx.staff),
    ] {
        for (method, route) in [
            ("GET", "/threads"),
            ("POST", "/threads"),
            ("POST", "/analytics/runs"),
            // The same path splits by verb: the launcher reads it, creating one is
            // the Factory's.
            ("GET", "/databases"),
        ] {
            let r = call(user, method, api(&fx, route)).await;
            assert_through(&r, &format!("{who} {method} {route}"));
        }
    }
}

/// The world model is the customer's to read and the Factory's to change. A plain
/// member reads the graph the home and the read-only page draw; the model itself is
/// `.world-model.yml` and the views, so changing it is a write through `/files`, and
/// the exploration routes beside the graph stay the Factory's too.
#[tokio::test]
async fn with_the_switch_on_a_plain_member_reads_the_world_model_but_cannot_change_it() {
    let fx = setup(true).await;
    assert_through(
        &call(&fx.member, "GET", api(&fx, "/semantic/world-model")).await,
        "member GET /semantic/world-model",
    );
    // `.world-model.yml`, base64url — the file that defines the model.
    let model_file = api(&fx, "/files/LndvcmxkLW1vZGVsLnltbA");
    assert_refused(
        &call(&fx.member, "POST", model_file.clone()).await,
        "member POST /files/<.world-model.yml>",
    );
    for (method, route) in [
        ("GET", "/semantic/world-model/instances"),
        ("POST", "/semantic/world-model/filter-counts"),
    ] {
        assert_refused(
            &call(&fx.member, method, api(&fx, route)).await,
            &format!("member {method} {route}"),
        );
    }
    // The control: someone the Factory admits changes it.
    assert_through(
        &call(&fx.admin, "POST", model_file).await,
        "admin POST /files/<.world-model.yml>",
    );
}

/// The shell's git state and config status, and the setup checks, are the Factory's:
/// a plain member is refused them, an org officer is not. (The web app no longer asks
/// for them for a customer; this is what makes that the server's rule too.)
#[tokio::test]
async fn with_the_switch_on_the_shells_git_and_setup_reads_are_the_factorys() {
    let fx = setup(true).await;
    for route in [
        "/git-state",
        "/status",
        "/onboarding-readiness",
        "/onboarding/github-setup",
    ] {
        assert_refused(
            &call(&fx.member, "GET", api(&fx, route)).await,
            &format!("member GET {route}"),
        );
        assert_through(
            &call(&fx.admin, "GET", api(&fx, route)).await,
            &format!("admin GET {route}"),
        );
    }
}

/// A grant is for one workspace: holding another's opens nothing here.
#[tokio::test]
async fn with_the_switch_on_a_grant_for_another_workspace_opens_nothing() {
    let fx = setup(true).await;
    let r = call(&fx.granted_elsewhere, "POST", api(&fx, "/files/abc")).await;
    assert_refused(&r, "granted elsewhere");
}

/// A route nobody classified is the Factory's, so a gap in the table refuses a
/// customer a page instead of handing them the IDE.
#[tokio::test]
async fn with_the_switch_on_an_unlisted_route_is_the_factorys() {
    let fx = setup(true).await;
    let refused = call(
        &fx.member,
        "GET",
        api(&fx, "/not-a-route-anyone-classified"),
    )
    .await;
    assert_refused(&refused, "member");
    let through = call(
        &fx.granted,
        "GET",
        api(&fx, "/not-a-route-anyone-classified"),
    )
    .await;
    assert_through(&through, "granted");
}

/// The external API is the customer's programmatic access: the gate lets it through
/// whole, whatever a route is called.
#[tokio::test]
async fn with_the_switch_on_the_external_api_is_not_gated() {
    let fx = setup(true).await;
    let r = call(
        &fx.member,
        "POST",
        format!("/external/api/{}/files/abc", fx.ws),
    )
    .await;
    assert_through(&r, "member on the external API");
}

/// What the rail reads: false for a plain member, true for the org's owner and admin,
/// a granted member and staff.
#[tokio::test]
async fn with_the_switch_on_the_workspace_response_says_who_may_use_the_factory() {
    let fx = setup(true).await;
    for (who, user, want) in [
        ("member", &fx.member, false),
        ("admin", &fx.admin, true),
        ("owner", &fx.owner, true),
        ("granted", &fx.granted, true),
        ("staff", &fx.staff, true),
    ] {
        let r = call(user, "GET", api(&fx, "/meta")).await;
        assert_eq!(r.status, StatusCode::OK, "{who}: {}", r.body);
        assert_eq!(r.body["factory_access"], want, "{who}");
    }
}

// ── Switch off ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn with_the_switch_off_nothing_changes_for_anyone() {
    let fx = setup(false).await;
    for (who, user) in [
        ("member", &fx.member),
        ("admin", &fx.admin),
        ("owner", &fx.owner),
        ("granted", &fx.granted),
        ("granted elsewhere", &fx.granted_elsewhere),
        ("staff", &fx.staff),
    ] {
        for (method, route) in [
            ("POST", "/files/abc"),
            ("GET", "/files"),
            ("POST", "/compile"),
            ("POST", "/databases"),
            ("GET", "/not-a-route-anyone-classified"),
            ("GET", "/threads"),
            ("POST", "/analytics/runs"),
            ("GET", "/semantic/world-model"),
            ("GET", "/git-state"),
            ("GET", "/status"),
        ] {
            let r = call(user, method, api(&fx, route)).await;
            assert_through(&r, &format!("{who} {method} {route}, switch off"));
        }
        let r = call(user, "GET", api(&fx, "/meta")).await;
        assert_eq!(
            r.body["factory_access"], true,
            "{who}: the rail is unchanged while the switch is off"
        );
    }
}

/// The switch is read per request: turning it on takes effect on the next one, and
/// turning it off lets everyone back in.
#[tokio::test]
async fn the_switch_takes_effect_on_the_next_request() {
    let fx = setup(false).await;
    assert_through(
        &call(&fx.member, "POST", api(&fx, "/files/abc")).await,
        "before",
    );
    cache::set(APPS_FIRST_EXPERIENCE, true);
    assert_refused(
        &call(&fx.member, "POST", api(&fx, "/files/abc")).await,
        "after turning it on",
    );
    cache::set(APPS_FIRST_EXPERIENCE, false);
    assert_through(
        &call(&fx.member, "POST", api(&fx, "/files/abc")).await,
        "after turning it off",
    );
}
