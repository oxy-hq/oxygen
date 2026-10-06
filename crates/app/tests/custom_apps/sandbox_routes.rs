//! The sandbox management routes, `/api/customer-apps/{id}/environments`
//! (`internal-docs/custom-app-sandboxes.md` §5.1), through a copy of the staff
//! console's guards: create, list, show and delete; every name that is not a
//! sandbox's, with its code. The limit of 20 is `sandbox_routes_limit`'s;
//! who is let through, `sandbox_routes_guards`'.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Router, middleware};
use oxy_app::server::api::admin::assume::block_admin_while_acting;
use oxy_app::server::api::custom_apps_sandboxes::handlers;
use oxy_app::server::api::custom_apps_sandboxes::teardown::SANDBOX_TEARDOWN_KIND;
use oxy_app::server::api::middlewares::oxy_owner_or_app_admin_guard::oxy_owner_or_app_admin_guard_middleware;
use oxy_app::server::api::middlewares::{app_scope_guard, platform_cap_guard};
use oxy_app::server::authz::Action;
use oxy_auth::middleware::{AuthState, auth_middleware};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    BUILD_ID, FunctionSpec, Tenant, publish_app, seeded_tenant,
};

const APP: &str = "sbx";

/// The four routes as `router::global` mounts them in the `/customer-apps`
/// nest, behind a copy of that nest's four layers in production order and
/// under `auth_middleware` (`build_global_routes` is `pub(super)`, hence the
/// copy). [`the_routes_are_mounted_inside_the_console_guards`] fails when
/// production stops mounting them so.
pub(crate) fn console() -> Router {
    let nest = Router::new()
        .route(
            "/{id}/environments",
            get(handlers::list).post(handlers::create),
        )
        .route(
            "/{id}/environments/{name}",
            get(handlers::show).delete(handlers::delete),
        )
        .layer(middleware::from_fn(block_admin_while_acting))
        .layer(middleware::from_fn(app_scope_guard::enforce_app_scope))
        .layer(middleware::from_fn(platform_cap_guard::require(
            Action::PlatformApps,
        )))
        .layer(middleware::from_fn(oxy_owner_or_app_admin_guard_middleware));
    Router::new()
        .nest("/api/customer-apps", nest)
        .layer(middleware::from_fn_with_state(
            AuthState::built_in(oxy_auth::token::SandboxAgent::Refuse),
            auth_middleware,
        ))
}

pub(crate) async fn send(method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(format!("/api/customer-apps/{path}"))
        .header("content-type", "application/json")
        .body(match body {
            Some(body) => Body::from(body.to_string()),
            None => Body::empty(),
        })
        .expect("request");
    let response = console().oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("read body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A published app (production and staging serve `BUILD_ID`), and the
/// guest made Oxy staff — the Global Owner, as `OXY_OWNER` grants.
pub(crate) async fn staffed_app(t: &Tenant) -> Uuid {
    let app = published_app(t).await;
    // SAFETY: nextest runs each test in its own process; set before any
    // console request, after the publish (authorized as the org Owner).
    unsafe {
        std::env::set_var("OXY_OWNER", LOCAL_GUEST_EMAIL);
        std::env::set_var("OXY_API_URL", "https://app-dev.oxygen-hq.com");
    }
    app
}

pub(crate) async fn published_app(t: &Tenant) -> Uuid {
    let noop = FunctionSpec {
        name: "noop",
        manifest: json!({ "route": true }),
        js: "export default async () => Response.json({});",
    };
    publish_app(t, APP, demo_workspace_id(), &[noop])
        .await
        .app_id
}

/// `(name, has a build, is deleting)` for each sandbox row, by name.
pub(crate) async fn sandbox_rows(db: &DatabaseConnection, app: Uuid) -> Vec<(String, bool, bool)> {
    db.query_all_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT name, build_id IS NOT NULL AS built, deleting_at IS NOT NULL AS deleting \
         FROM app_environments WHERE app_id = $1 AND kind = 'dev' ORDER BY name",
        [app.into()],
    ))
    .await
    .expect("read sandboxes")
    .iter()
    .map(|row| {
        (
            row.try_get("", "name").expect("name"),
            row.try_get("", "built").expect("built"),
            row.try_get("", "deleting").expect("deleting"),
        )
    })
    .collect()
}

async fn count(db: &DatabaseConnection, sql: &str, app: Uuid) -> i64 {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [app.to_string().into()],
    ))
    .await
    .expect("count")
    .expect("a row")
    .try_get("", "n")
    .expect("n")
}

async fn queued_teardowns(db: &DatabaseConnection, app: Uuid) -> i64 {
    count(
        db,
        "SELECT count(*) AS n FROM agentic_task_queue \
         WHERE queue_status = 'queued' AND spec->>'kind' = 'custom_app_sandbox_teardown' \
           AND spec->'payload'->>'app_id' = $1",
        app,
    )
    .await
}

async fn audit_rows(db: &DatabaseConnection, app: Uuid, action: &str) -> i64 {
    count(
        db,
        &format!(
            "SELECT count(*) AS n FROM audit_events \
             WHERE action = '{action}' AND target_id LIKE $1 || '/%'"
        ),
        app,
    )
    .await
}

/// The whole lifecycle through the console: create answers the
/// Environment with no build; list is production, staging, then the
/// sandboxes; a duplicate is refused; delete marks the row, queues the
/// teardown and keeps the name taken; a second delete answers the run
/// already on its way, and queues one of its own only once that has ended.
#[tokio::test]
async fn create_list_show_and_delete_a_sandbox_through_the_console() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;

    let (status, created) = send(
        "POST",
        &format!("{app}/environments"),
        Some(json!({ "name": "dev-a1" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["name"], "dev-a1");
    assert_eq!(created["kind"], "dev");
    assert_eq!(created["status"], "active");
    for null in ["build_id", "build_uuid", "semantic_revision_id"] {
        assert!(
            created[null].is_null(),
            "{null}: a sandbox starts with no build"
        );
    }
    assert_eq!(created["owner"]["user_id"], t.guest_id.to_string());
    assert_eq!(created["owner"]["email"], LOCAL_GUEST_EMAIL);
    assert_eq!(created["last_activity_at"], created["updated_at"]);
    let at = |field: &str| {
        chrono::DateTime::parse_from_rfc3339(created[field].as_str().expect(field)).expect(field)
    };
    assert_eq!(
        at("expires_at") - at("last_activity_at"),
        chrono::Duration::days(7)
    );
    assert_eq!(
        created["url"],
        format!("https://dev-a1--local--{APP}.customer-apps-dev.oxygen-hq.com/")
    );
    assert_eq!(audit_rows(&t.db, app, "app.environment.created").await, 1);

    send(
        "POST",
        &format!("{app}/environments"),
        Some(json!({ "name": "dev-0b" })),
    )
    .await;
    let (status, listed) = send("GET", &format!("{app}/environments"), None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let environments = listed["environments"].as_array().expect("environments");
    let names: Vec<&str> = environments
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["production", "staging", "dev-0b", "dev-a1"]);
    for fixed in &environments[..2] {
        assert_eq!(fixed["build_id"], BUILD_ID, "{fixed}");
        assert_eq!(fixed["status"], "active");
        for null in ["owner", "last_activity_at", "expires_at"] {
            assert!(fixed[null].is_null(), "{null} is a sandbox's: {fixed}");
        }
    }
    assert_eq!(environments[0]["kind"], "production");
    assert_eq!(
        environments[1]["url"],
        format!("https://staging--local--{APP}.customer-apps-dev.oxygen-hq.com/")
    );
    let (status, shown) = send("GET", &format!("{app}/environments/dev-a1"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(shown, created, "show answers what create did");
    let (status, staging) = send("GET", &format!("{app}/environments/staging"), None).await;
    assert_eq!(
        (status, &staging["build_id"]),
        (StatusCode::OK, &json!(BUILD_ID))
    );

    let (status, duplicate) = send(
        "POST",
        &format!("{app}/environments"),
        Some(json!({ "name": "dev-a1" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(duplicate["error"], "environment_exists");

    let (status, deleting) = send("DELETE", &format!("{app}/environments/dev-a1"), None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{deleting}");
    assert_eq!(deleting["name"], "dev-a1");
    assert_eq!(deleting["status"], "deleting");
    let run_id = deleting["teardown_run_id"].as_str().expect("a run id");
    assert!(
        run_id.starts_with(&format!("{SANDBOX_TEARDOWN_KIND}:{app}:dev-a1:")),
        "{run_id}"
    );
    assert_eq!(
        sandbox_rows(&t.db, app).await,
        vec![
            ("dev-0b".to_string(), false, false),
            ("dev-a1".to_string(), false, true)
        ],
        "the row stays, marked, until the teardown removes it"
    );
    assert_eq!(queued_teardowns(&t.db, app).await, 1);
    let (status, shown) = send("GET", &format!("{app}/environments/dev-a1"), None).await;
    assert_eq!(
        (status, &shown["status"]),
        (StatusCode::OK, &json!("deleting"))
    );

    // The name stays taken while it is being torn down.
    let (status, taken) = send(
        "POST",
        &format!("{app}/environments"),
        Some(json!({ "name": "dev-a1" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(taken["error"], "environment_deleting");

    // A second DELETE while that teardown is on its way answers it again:
    // no second run, no second audit row.
    let (status, again) = send("DELETE", &format!("{app}/environments/dev-a1"), None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{again}");
    assert_eq!(again, deleting, "the same answer, the same run");
    assert_eq!(queued_teardowns(&t.db, app).await, 1);
    assert_eq!(audit_rows(&t.db, app, "app.environment.deleted").await, 1);

    // Once that run has ended and the sandbox is still there — a failed
    // teardown — a DELETE is the retry: a run of its own, still one audit row.
    t.db.execute_unprepared(
        "UPDATE agentic_task_queue SET queue_status = 'failed' \
         WHERE spec->>'kind' = 'custom_app_sandbox_teardown'",
    )
    .await
    .expect("fail the teardown");
    let (status, retried) = send("DELETE", &format!("{app}/environments/dev-a1"), None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{retried}");
    assert_ne!(retried["teardown_run_id"], deleting["teardown_run_id"]);
    assert_eq!(queued_teardowns(&t.db, app).await, 1);
    assert_eq!(audit_rows(&t.db, app, "app.environment.deleted").await, 1);
}

/// What is refused, and with which code. Nothing is written by any of it.
#[tokio::test]
async fn names_that_are_not_a_sandboxs_are_refused_with_their_code() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    let environments = format!("{app}/environments");

    for (name, code) in [
        ("staging", "not_a_sandbox"),
        ("production", "not_a_sandbox"),
        ("dev--x", "invalid_environment_name"),
        ("a1", "invalid_environment_name"),
        ("dev-abcdefghijklm", "invalid_environment_name"),
    ] {
        let (status, refused) = send("POST", &environments, Some(json!({ "name": name }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "create {name}: {refused}");
        assert_eq!(refused["error"], code, "create {name}");
        assert!(refused["message"].is_string(), "{refused}");
    }
    for (name, status, code) in [
        ("staging", StatusCode::BAD_REQUEST, "not_a_sandbox"),
        ("production", StatusCode::BAD_REQUEST, "not_a_sandbox"),
        (
            "dev-UPPER",
            StatusCode::BAD_REQUEST,
            "invalid_environment_name",
        ),
        ("dev-nobody", StatusCode::NOT_FOUND, "environment_not_found"),
    ] {
        let (got, refused) = send("DELETE", &format!("{environments}/{name}"), None).await;
        assert_eq!(got, status, "delete {name}: {refused}");
        assert_eq!(refused["error"], code, "delete {name}");
    }
    let (status, missing) = send("GET", &format!("{environments}/dev-nobody"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing["error"], "environment_not_found");
    let (status, bad) = send("GET", &format!("{environments}/nope"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(bad["error"], "invalid_environment_name");

    let stranger = Uuid::new_v4();
    let (status, unknown) = send("GET", &format!("{stranger}/environments"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(unknown["error"], "app_not_found");

    assert!(sandbox_rows(&t.db, app).await.is_empty());
    // Staging still serves: naming it in a DELETE cleared nothing.
    let (_, staging) = send("GET", &format!("{environments}/staging"), None).await;
    assert_eq!(staging["build_id"], BUILD_ID);
}
