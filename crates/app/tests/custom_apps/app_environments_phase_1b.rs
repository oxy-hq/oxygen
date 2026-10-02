//! Phase 1b of the custom-app environments design
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §3.2, §7):
//! staging hosts serve staging HTML, and nothing a staging page does reaches
//! production.
//!
//! Each test drives the real serve route (`serve_dispatch`, the functions
//! fixture's copy of production's mount) against a published app with a
//! promoted build (production) and a newer unpromoted one (staging):
//!
//! - a staging host serves the staging build to Oxy staff, with
//!   `window.__OXY_APP__.environment` saying so, and refuses everyone else;
//! - the production host, for the same viewer, serves exactly what it did;
//! - `/fn` on staging runs the staging build for staff (Phase 3 of the previews
//!   plan — its writes are held, see `staging_functions`) and is refused to
//!   everyone else; production's `/fn` still runs;
//! - a sandbox (`dev-<handle>`, `internal-docs/custom-app-sandboxes.md`) takes
//!   the same arm: its host serves its own build to staff, read per request;
//!   `/fn` runs that build for staff and is refused to everyone else; a name
//!   nobody created runs nothing;
//! - an `X-Oxy-App-Env` header on a cookie request is a 400, never a silent
//!   production call;
//! - a cookie request from a staging page cannot run a production function,
//!   and the same request with a bearer token and no `Origin` is served;
//! - a queued task naming staging is refused by the runner, not run.
//!
//! The `/api` write refusal runs in the outer stack, before any route; it is
//! pinned through the shipped stack in `cli::commands::serve`'s tests and in
//! `oxy_app_core::custom_app_env_request`.

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::TaskExecutor;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use entity::app_builds;
use oxy_app::server::api::custom_apps_environments::{self, EnvAction};
use oxy_app::server::api::custom_apps_publish::{OrgRef, PublishInput, publish};
use oxy_app::server::app_function_executor::{APP_FUNCTION_KIND, AppFunctionTaskExecutor};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    FunctionSpec, ORG_SLUG, Tenant, bundle, call_function_with, invocations, publish_app,
    seeded_tenant, serve_router,
};

const APP: &str = "env-1b";
const STAGING_BUILD: &str = "env-1b-staging";

fn staging_host() -> String {
    format!("staging--{ORG_SLUG}--{APP}.customer-apps.oxygen-hq.com")
}

fn production_host() -> String {
    format!("{ORG_SLUG}--{APP}.customer-apps.oxygen-hq.com")
}

fn functions() -> Vec<FunctionSpec> {
    vec![FunctionSpec {
        name: "stamp",
        manifest: json!({ "route": true }),
        js: "export default async () => Response.json({ ran: true });",
    }]
}

/// The app with a promoted build (production) and a newer one published
/// without promote (staging). Returns (app_id, production build pk, staging
/// build pk).
async fn two_environments(t: &Tenant) -> (Uuid, Uuid, Uuid) {
    let app_id = publish_app(t, APP, demo_workspace_id(), &functions())
        .await
        .app_id;
    publish(PublishInput {
        org_ref: Some(OrgRef::Id(t.org_id)),
        app_slug: APP.to_string(),
        project_id: demo_workspace_id(),
        branch: None,
        build_id: STAGING_BUILD.to_string(),
        name: None,
        promote: false,
        tarball: bundle(APP, &functions()),
        manifest: None,
        source_repo: None,
        commit_sha: None,
        published_by: Some(t.guest_id),
        published_by_email: Some(LOCAL_GUEST_EMAIL.to_string()),
        machine_app_id: None,
        published_via: None,
        semantic_revision_id: None,
    })
    .await
    .expect("an unpromoted publish lands in staging");
    let builds = app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .all(&t.db)
        .await
        .expect("read builds");
    let pk = |id: &str| {
        builds
            .iter()
            .find(|b| b.build_id == id)
            .unwrap_or_else(|| panic!("build {id}"))
            .id
    };
    (
        app_id,
        pk(crate::custom_app_functions_fixture::BUILD_ID),
        pk(STAGING_BUILD),
    )
}

/// Oxy staff: the Global Owner, the standing `OXY_OWNER` grants. nextest runs
/// each test in its own process, so this reaches no other test.
fn make_guest_staff() {
    unsafe { std::env::set_var("OXY_OWNER", LOCAL_GUEST_EMAIL) };
}

async fn get_html(host: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .uri(format!("/customer-apps/{ORG_SLUG}/{APP}/"))
        .header("host", host)
        .header("accept", "text/html")
        .body(Body::empty())
        .expect("request");
    let response = serve_router().oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn a_staging_host_serves_the_staging_build_to_staff() {
    let t = seeded_tenant().await;
    let (_, production_build, staging_build) = two_environments(&t).await;
    make_guest_staff();

    let (status, html) = get_html(&staging_host()).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        html.contains(&format!("\"buildId\":\"{staging_build}\"")),
        "the staging host serves the staging build: {html}"
    );
    assert!(
        html.contains("\"environment\":\"staging\""),
        "window.__OXY_APP__.environment names the environment: {html}"
    );

    // Production, for the same staff viewer, is what it was: the promoted build.
    let (status, html) = get_html(&production_host()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(&format!("\"buildId\":\"{production_build}\"")),
        "the production host serves the promoted build: {html}"
    );
    assert!(html.contains("\"environment\":\"production\""), "{html}");
}

/// Customers never open an unreleased build: the org's own Owner, who opens
/// production, is refused on the staging host.
#[tokio::test]
async fn a_staging_host_refuses_a_viewer_who_is_not_staff() {
    let t = seeded_tenant().await;
    two_environments(&t).await;

    let (status, _) = get_html(&staging_host()).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an org owner is not Oxy staff"
    );

    let (status, html) = get_html(&production_host()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "production is unchanged for them: {html}"
    );
}

/// Staging `/fn` is Oxy staff's, like staging HTML: an org Owner who runs
/// production's functions is refused on the staging host, and nothing runs.
#[tokio::test]
async fn fn_in_staging_is_refused_to_a_viewer_who_is_not_staff() {
    let t = seeded_tenant().await;
    let (app_id, ..) = two_environments(&t).await;

    let staging = call_function_with(
        ORG_SLUG,
        APP,
        "stamp",
        json!({}),
        &[("host", &staging_host())],
    )
    .await;
    assert_eq!(staging.status, StatusCode::FORBIDDEN, "{}", staging.raw);
    assert!(
        staging.raw.contains("EnvironmentRefused"),
        "{}",
        staging.raw
    );
    assert!(
        invocations(&t.db, app_id, "stamp").await.is_empty(),
        "nothing ran outside production"
    );
}

/// Phase 3: staff run the staging build's functions — by host and by a
/// bearer-named environment (`oxyc fn call --app-env`). Production is
/// unchanged.
#[tokio::test]
async fn fn_runs_in_staging_for_staff() {
    let t = seeded_tenant().await;
    let (app_id, production_build, staging_build) = two_environments(&t).await;
    make_guest_staff();

    let staging = call_function_with(
        ORG_SLUG,
        APP,
        "stamp",
        json!({}),
        &[("host", &staging_host())],
    )
    .await;
    assert_eq!(staging.status, StatusCode::OK, "{}", staging.raw);
    let by_header = call_function_with(
        ORG_SLUG,
        APP,
        "stamp",
        json!({}),
        &[("authorization", "Bearer t"), ("x-oxy-app-env", "staging")],
    )
    .await;
    assert_eq!(by_header.status, StatusCode::OK, "{}", by_header.raw);

    let production = call_function_with(
        ORG_SLUG,
        APP,
        "stamp",
        json!({}),
        &[("host", &production_host())],
    )
    .await;
    assert_eq!(production.status, StatusCode::OK, "{}", production.raw);
    assert_eq!(production.frame("data"), Some(&json!({ "ran": true })));
    let rows = invocations(&t.db, app_id, "stamp").await;
    assert_eq!(
        rows.iter()
            .map(|r| (r.environment.as_str(), r.build_id, r.status.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("staging", staging_build, "success"),
            ("staging", staging_build, "success"),
            ("production", production_build, "success"),
        ]
    );
}

const SANDBOX: &str = "dev-luong";

fn sandbox_host() -> String {
    format!("{SANDBOX}--{ORG_SLUG}--{APP}.customer-apps.oxygen-hq.com")
}

fn sandbox() -> AppEnvironment {
    AppEnvironment::parse(SANDBOX).expect("a sandbox name")
}

/// A sandbox of the app, pointed at `build`.
async fn sandbox_serving(t: &Tenant, app_id: Uuid, build: Uuid) {
    crate::app_environments::seed_sandbox(&t.db, app_id, SANDBOX, t.guest_id).await;
    custom_apps_environments::record_move(
        &t.db,
        app_id,
        &sandbox(),
        Some(build),
        EnvAction::Publish,
        Some(t.guest_id),
    )
    .await
    .expect("point the sandbox");
}

/// A sandbox runs its own build's functions for staff — by host and by a
/// bearer-named environment — recorded under the sandbox's name. It is no
/// longer refused outright: it takes the arm staging does.
#[tokio::test]
async fn fn_runs_in_a_sandbox_for_staff_on_the_sandboxs_own_build() {
    let t = seeded_tenant().await;
    let (app_id, production_build, staging_build) = two_environments(&t).await;
    // The sandbox serves production's build, so "its own" is distinguishable
    // from staging's — the environment a sandbox must never borrow from.
    sandbox_serving(&t, app_id, production_build).await;
    make_guest_staff();

    let by_host = call_function_with(
        ORG_SLUG,
        APP,
        "stamp",
        json!({}),
        &[("host", &sandbox_host())],
    )
    .await;
    assert_eq!(by_host.status, StatusCode::OK, "{}", by_host.raw);
    assert_eq!(by_host.frame("data"), Some(&json!({ "ran": true })));
    let by_header = call_function_with(
        ORG_SLUG,
        APP,
        "stamp",
        json!({}),
        &[("authorization", "Bearer t"), ("x-oxy-app-env", SANDBOX)],
    )
    .await;
    assert_eq!(by_header.status, StatusCode::OK, "{}", by_header.raw);

    let rows = invocations(&t.db, app_id, "stamp").await;
    assert_eq!(
        rows.iter()
            .map(|r| (r.environment.as_str(), r.build_id, r.status.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (SANDBOX, production_build, "success"),
            (SANDBOX, production_build, "success"),
        ],
        "never staging's build ({staging_build})"
    );
}

/// A sandbox is staff's, like staging: an org Owner who runs production's
/// functions is refused, and nothing runs.
#[tokio::test]
async fn fn_in_a_sandbox_is_refused_to_a_viewer_who_is_not_staff() {
    let t = seeded_tenant().await;
    let (app_id, production_build, _) = two_environments(&t).await;
    sandbox_serving(&t, app_id, production_build).await;

    let viewer = call_function_with(
        ORG_SLUG,
        APP,
        "stamp",
        json!({}),
        &[("host", &sandbox_host())],
    )
    .await;
    assert_eq!(viewer.status, StatusCode::FORBIDDEN, "{}", viewer.raw);
    assert!(viewer.raw.contains("EnvironmentRefused"), "{}", viewer.raw);
    assert!(invocations(&t.db, app_id, "stamp").await.is_empty());
}

/// A name nobody created runs nothing, even for staff — it is never
/// downgraded to production's build or to staging's.
#[tokio::test]
async fn a_sandbox_nobody_created_runs_nothing_even_for_staff() {
    let t = seeded_tenant().await;
    let (app_id, ..) = two_environments(&t).await;
    make_guest_staff();

    let unknown = call_function_with(
        ORG_SLUG,
        APP,
        "stamp",
        json!({}),
        &[
            ("authorization", "Bearer t"),
            ("x-oxy-app-env", "dev-nobody"),
        ],
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.raw);
    assert!(
        invocations(&t.db, app_id, "stamp").await.is_empty(),
        "nothing ran"
    );
}

/// A sandbox host serves the sandbox's own build to staff, naming the
/// environment; the moment the sandbox is marked deleting it serves nothing —
/// its build is read per request, not from the app-resolution cache.
#[tokio::test]
async fn a_sandbox_host_serves_its_own_build_to_staff_until_it_is_deleted() {
    let t = seeded_tenant().await;
    let (app_id, production_build, staging_build) = two_environments(&t).await;
    sandbox_serving(&t, app_id, production_build).await;
    make_guest_staff();

    let (status, html) = get_html(&sandbox_host()).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        html.contains(&format!("\"buildId\":\"{production_build}\"")),
        "the sandbox host serves the sandbox's build: {html}"
    );
    assert!(
        html.contains(&format!("\"environment\":\"{SANDBOX}\"")),
        "{html}"
    );

    // Move the pointer: the very next request serves the new build.
    custom_apps_environments::record_move(
        &t.db,
        app_id,
        &sandbox(),
        Some(staging_build),
        EnvAction::Publish,
        None,
    )
    .await
    .expect("republish");
    let (status, html) = get_html(&sandbox_host()).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        html.contains(&format!("\"buildId\":\"{staging_build}\"")),
        "a publish to a sandbox shows at once: {html}"
    );

    t.db.execute_unprepared(
        "UPDATE app_environments SET deleting_at = now() WHERE name = 'dev-luong'",
    )
    .await
    .expect("mark deleting");
    let (status, _) = get_html(&sandbox_host()).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a deleting sandbox serves nothing"
    );

    // Control: staging and production are untouched by any of it.
    let (status, html) = get_html(&staging_host()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(&format!("\"buildId\":\"{staging_build}\"")));
}

/// A viewer who is not staff never opens a sandbox host.
#[tokio::test]
async fn a_sandbox_host_refuses_a_viewer_who_is_not_staff() {
    let t = seeded_tenant().await;
    let (app_id, production_build, _) = two_environments(&t).await;
    sandbox_serving(&t, app_id, production_build).await;

    let (status, _) = get_html(&sandbox_host()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Never downgraded: a header the server cannot honour is a 400.
#[tokio::test]
async fn an_environment_header_on_a_cookie_request_is_a_400() {
    let t = seeded_tenant().await;
    let (app_id, ..) = two_environments(&t).await;

    let call = call_function_with(
        ORG_SLUG,
        APP,
        "stamp",
        json!({}),
        &[("cookie", "oxy_session=jwt"), ("x-oxy-app-env", "staging")],
    )
    .await;
    assert_eq!(call.status, StatusCode::BAD_REQUEST, "{}", call.raw);
    assert!(invocations(&t.db, app_id, "stamp").await.is_empty());
}

/// §3.2's origin check. Every app host shares the session cookie, so without it
/// a staging page could run production's function as the viewer.
#[tokio::test]
async fn a_staging_page_cannot_run_a_production_function_with_the_cookie() {
    let t = seeded_tenant().await;
    let (app_id, ..) = two_environments(&t).await;
    let staging_origin = format!("https://{}", staging_host());
    let production_origin = format!("https://{}", production_host());

    let cross = call_function_with(
        ORG_SLUG,
        APP,
        "stamp",
        json!({}),
        &[
            ("host", &production_host()),
            ("cookie", "oxy_session=jwt"),
            ("origin", &staging_origin),
        ],
    )
    .await;
    assert_eq!(cross.status, StatusCode::FORBIDDEN, "{}", cross.raw);
    assert!(
        cross.raw.contains("CrossEnvironmentOrigin"),
        "{}",
        cross.raw
    );
    assert!(invocations(&t.db, app_id, "stamp").await.is_empty());

    // The same page's own environment is fine, and a bearer request with no
    // Origin (oxyc, the checks workflow) skips the check.
    let same = call_function_with(
        ORG_SLUG,
        APP,
        "stamp",
        json!({}),
        &[
            ("host", &production_host()),
            ("cookie", "oxy_session=jwt"),
            ("origin", &production_origin),
        ],
    )
    .await;
    assert_eq!(same.status, StatusCode::OK, "{}", same.raw);
    let bearer = call_function_with(
        ORG_SLUG,
        APP,
        "stamp",
        json!({}),
        &[("host", &production_host()), ("authorization", "Bearer t")],
    )
    .await;
    assert_eq!(bearer.status, StatusCode::OK, "{}", bearer.raw);
    assert_eq!(invocations(&t.db, app_id, "stamp").await.len(), 2);
}

/// The scheduled / queued path takes the same gate: a task naming staging is
/// refused by the runner rather than run on production's build.
#[tokio::test]
async fn a_queued_task_for_staging_is_refused_not_run() {
    let t = seeded_tenant().await;
    let (app_id, ..) = two_environments(&t).await;
    let executor = AppFunctionTaskExecutor {
        db: t.db.clone(),
        preagg: Default::default(),
    };
    let assignment = TaskAssignment {
        task_id: "t1".into(),
        parent_task_id: None,
        run_id: "r1".into(),
        spec: TaskSpec::Custom {
            kind: APP_FUNCTION_KIND.into(),
            payload: json!({
                "app_id": app_id.to_string(),
                "function_name": "stamp",
                "trigger": "manual",
                "environment": "staging",
            }),
        },
        policy: None,
    };
    let mut task = executor
        .execute(assignment)
        .await
        .expect("the task is read");
    let outcome = task.outcomes.recv().await.expect("an outcome");
    match outcome {
        TaskOutcome::Failed(message) => {
            assert!(message.contains("staging"), "{message}")
        }
        other => panic!("a staging task must not run: {other:?}"),
    }
    assert!(
        invocations(&t.db, app_id, "stamp").await.is_empty(),
        "the runner refused before writing an invocation"
    );
}
