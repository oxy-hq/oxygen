//! The real `start_automation_run` handler on a custom app's **staging**
//! host (`projects::automation_run::staging_hold`), called with the state the
//! router hands it.
//!
//! - a caller who may open staging gets `409 held_in_staging` before any
//!   `customer_app_procedure_runs` row is created, and the attempt is listed
//!   by `…/staging/held` as one `app.staging.held` row (`function:
//!   "automation"`, a single `writes` entry: `plane: "automation", verb:
//!   "RUN", namespace: <automation id>`);
//! - production starts the run as before;
//! - a sandbox follows its `workspace-actions` plane, not its kind: held by
//!   default (the same `409`, the held row under the sandbox's own name),
//!   `404 EnvironmentRefused` when its row says `refused`, `409` when the row
//!   cannot be read; so does a staging whose row says `refused`;
//! - a caller who may not open staging gets the same `404 EnvironmentRefused`
//!   a staging ask refuses with, and no run row either.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use entity::customer_app_procedure_runs as proc_run;
use entity::users::UserStatus;
use oxy_api_custom_apps::custom_apps_staging_held::{HeldEntry, HeldQuery, list_held};
use oxy_app::surface::testing::agentic_wiring::OxyThreadOwnerLookup;
use oxy_app::surface::testing::compile_config_gate::runtime_config_gate;
use oxy_app::surface::testing::projects::automation_run::start_automation_run;
use oxy_app_core::AppState;
use oxy_app_core::serve_mode::ServeMode;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use oxy_compile::{CompileRequest, Promotion, RevisionKind, compile_workspace, compiler_version};
use sea_orm::{EntityTrait, PaginatorTrait};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

use crate::common::{demo_workspace_id, examples_path};
use crate::custom_app_functions_fixture::{Tenant, publish_build, seeded_tenant};
use crate::staging_functions::{
    create_sandbox, held_rows, make_guest_staff, production_host, sandbox_host, staging_host,
};

/// A automation declared in `examples/procedures/anonymize.automation.yml`,
/// whose file stem (not its `name:`) is the id this route takes.
const AUTOMATION_ID: &str = "anonymize";

fn state(t: &Tenant) -> AppState {
    let agentic = agentic_http::AgenticState::new(
        tokio_util::sync::CancellationToken::new(),
        t.db.clone(),
        Arc::new(OxyThreadOwnerLookup::new(t.db.clone())),
    );
    AppState {
        enterprise: false,
        internal: false,
        mode: ServeMode::Cloud,
        observability: None,
        startup_cwd: std::path::PathBuf::new(),
        preagg_cache: None,
        preagg_renewal_threshold_secs: None,
        agentic_state: Some(Arc::new(agentic)),
        semantic_layer_cache: oxy_app_core::workspace_cache::new_semantic_layer_cache(),
        semantic_engine_cache: oxy_app_core::workspace_cache::new_semantic_engine_cache(),
    }
}

/// No session cookie: the zero-config guest is the caller.
fn on_host(host: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("host", HeaderValue::from_str(host).unwrap());
    h
}

/// Compile and promote the one automation these tests start, under the demo
/// workspace's id. The start handler answers from the promoted revision and
/// nothing else. Compiling all of `examples/` would be deferred on its
/// `schemas/*.sql` migrations, which need an OLTP database this test has not
/// got, so only the automation and a `config.yml` go in.
async fn compile_demo_workspace(t: &Tenant) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("config.yml"), "databases: []\nmodels: []\n")
        .expect("config.yml");
    std::fs::create_dir_all(dir.path().join("procedures")).expect("mkdir");
    std::fs::copy(
        examples_path().join("procedures/anonymize.automation.yml"),
        dir.path().join("procedures/anonymize.automation.yml"),
    )
    .expect("copy the example automation");
    let outcome = compile_workspace(CompileRequest {
        db: &t.db,
        workspace_id: demo_workspace_id(),
        workspace_path: dir.path(),
        git_sha: None,
        branch: None,
        compiler_version: compiler_version(),
        promote: true,
        kind: RevisionKind::Main,
        owner_user_id: None,
        config_gate: Some(runtime_config_gate()),
    })
    .await
    .expect("compile the workspace");
    assert_eq!(
        outcome.promotion,
        Promotion::Promoted,
        "failures: {:?}",
        outcome.failures
    );
}

async fn app(t: &Tenant, slug: &str) -> Uuid {
    publish_build(t, slug, demo_workspace_id(), "automation-1", true, &[])
        .await
        .app_id
}

async fn json_of(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

async fn start(t: &Tenant, host: &str, automation_id: &str) -> (StatusCode, serde_json::Value) {
    // Boxed: the handler's future is larger than a test thread's stack.
    let resp = Box::pin(start_automation_run(
        State(state(t)),
        Path((demo_workspace_id(), automation_id.to_string())),
        on_host(host),
        Bytes::from(json!({}).to_string()),
    ))
    .await;
    json_of(resp).await
}

async fn run_count(t: &Tenant) -> u64 {
    proc_run::Entity::find().count(&t.db).await.expect("count")
}

async fn held_for(t: &Tenant, app_id: Uuid) -> Vec<HeldEntry> {
    let who = AuthenticatedUserExtractor(AuthenticatedUser {
        id: t.guest_id,
        email: Some(LOCAL_GUEST_EMAIL.to_string()),
        name: "Local User".to_string(),
        picture: None,
        credential: None,
        status: UserStatus::Active,
    });
    list_held(Path(app_id), Query(HeldQuery::default()), who)
        .await
        .map(|Json(rows)| rows)
        .expect("list_held")
}

#[tokio::test]
async fn staging_answers_409_before_any_run_row_and_logs_it_held() {
    let t = seeded_tenant().await;
    let slug = "stg-automation-409";
    let app_id = app(&t, slug).await;
    make_guest_staff();
    let before = run_count(&t).await;

    let (status, body) = start(&t, &staging_host(&t, slug), AUTOMATION_ID).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "held_in_staging", "{body}");
    assert_eq!(body["surface"], "automation", "{body}");
    assert_eq!(
        body["what"], "starting an automation run from an app's staging",
        "{body}"
    );
    assert_eq!(run_count(&t).await, before, "no run row was created");

    let rows = held_for(&t, app_id).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].function, "automation");
    assert_eq!(rows[0].writes.len(), 1, "{:?}", rows[0].writes);
    let w = &rows[0].writes[0];
    assert_eq!(w.plane, "automation");
    assert_eq!(w.verb, "RUN");
    assert_eq!(w.namespace, AUTOMATION_ID);
    assert_eq!(w.table, "");
}

#[tokio::test]
async fn production_starts_the_run_unaffected() {
    let t = seeded_tenant().await;
    let slug = "stg-automation-prod";
    app(&t, slug).await;
    compile_demo_workspace(&t).await;
    let before = run_count(&t).await;

    let (status, body) = start(&t, &production_host(&t, slug), AUTOMATION_ID).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert!(body["run_id"].as_str().is_some(), "{body}");
    assert_eq!(run_count(&t).await, before + 1);
}

#[tokio::test]
async fn staging_refuses_a_caller_who_may_not_open_it() {
    let t = seeded_tenant().await;
    let slug = "stg-automation-404";
    app(&t, slug).await;
    let before = run_count(&t).await;

    // The guest is an org Owner but not Oxy staff: no `AppNonProduction`. The
    // console's own list would 404 for this same caller, so the "nothing was
    // logged" check reads the audit table directly rather than through it.
    let (status, body) = start(&t, &staging_host(&t, slug), AUTOMATION_ID).await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "EnvironmentRefused", "{body}");
    assert_eq!(run_count(&t).await, before, "no run row was created");
    assert!(
        held_rows(&t).await.is_empty(),
        "a refusal this caller cannot open logs nothing"
    );
}

/// The audit rows' environments, oldest first.
async fn held_environments(t: &Tenant) -> Vec<String> {
    let mut rows = held_rows(t).await;
    rows.sort_by_key(|r| r.seq);
    rows.into_iter().map(|r| r.environment).collect()
}

/// A sandbox's default is staging's: the run is held, never started, and the
/// held row names the sandbox.
#[tokio::test]
async fn a_sandbox_holds_by_default_like_staging() {
    let t = seeded_tenant().await;
    let slug = "sbx-automation-409";
    let app_id = app(&t, slug).await;
    create_sandbox(&t, app_id, "a1", None).await;
    make_guest_staff();
    let before = run_count(&t).await;

    let (status, body) = start(&t, &sandbox_host(&t, slug, "a1"), AUTOMATION_ID).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "held_in_staging", "{body}");
    assert_eq!(body["surface"], "automation", "{body}");
    assert_eq!(run_count(&t).await, before, "no run row was created");
    assert_eq!(held_environments(&t).await, ["dev-a1"]);
    let rows = held_rows(&t).await;
    assert_eq!(rows[0].metadata["mode"], "automation");
}

/// A sandbox set to `refused` keeps today's answer.
#[tokio::test]
async fn a_sandbox_set_to_refused_keeps_todays_refusal() {
    let t = seeded_tenant().await;
    let slug = "sbx-automation-refused";
    let app_id = app(&t, slug).await;
    create_sandbox(
        &t,
        app_id,
        "a1",
        Some(json!({"workspace-actions": "refused"})),
    )
    .await;
    make_guest_staff();
    let before = run_count(&t).await;

    let (status, body) = start(&t, &sandbox_host(&t, slug, "a1"), AUTOMATION_ID).await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "EnvironmentRefused", "{body}");
    assert_eq!(body["environment"], "dev-a1", "{body}");
    assert_eq!(run_count(&t).await, before);
    assert!(
        held_rows(&t).await.is_empty(),
        "nothing is held, nothing is logged"
    );
}

/// The plane decides, not the kind: a staging whose row says `refused` is
/// refused like a refusing sandbox.
#[tokio::test]
async fn a_staging_set_to_refused_is_refused() {
    let t = seeded_tenant().await;
    let slug = "stg-automation-refused";
    let app_id = app(&t, slug).await;
    make_guest_staff();
    set_staging_planes(&t, app_id, json!({"workspace-actions": "refused"})).await;

    let (status, body) = start(&t, &staging_host(&t, slug), AUTOMATION_ID).await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "EnvironmentRefused", "{body}");
    assert!(held_rows(&t).await.is_empty());
}

/// A plane nobody can read refuses the run (`409`) and never falls back to
/// the default that would have held it.
#[tokio::test]
async fn an_unreadable_plane_refuses_the_run() {
    let t = seeded_tenant().await;
    let slug = "sbx-automation-unreadable";
    let app_id = app(&t, slug).await;
    create_sandbox(
        &t,
        app_id,
        "a1",
        Some(json!({"workspace-actions": "teleport"})),
    )
    .await;
    make_guest_staff();
    let before = run_count(&t).await;

    let (status, body) = start(&t, &sandbox_host(&t, slug, "a1"), AUTOMATION_ID).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "EnvironmentRefused", "{body}");
    assert!(
        body["message"].as_str().unwrap_or("").contains("planes"),
        "{body}"
    );
    assert_eq!(run_count(&t).await, before);
    assert!(held_rows(&t).await.is_empty());
}

/// A caller who may not open the sandbox learns nothing of its planes: the
/// same `404` as for staging.
#[tokio::test]
async fn a_sandbox_refuses_a_caller_who_may_not_open_it() {
    let t = seeded_tenant().await;
    let slug = "sbx-automation-404";
    let app_id = app(&t, slug).await;
    create_sandbox(
        &t,
        app_id,
        "a1",
        Some(json!({"workspace-actions": "teleport"})),
    )
    .await;

    let (status, body) = start(&t, &sandbox_host(&t, slug, "a1"), AUTOMATION_ID).await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "EnvironmentRefused", "{body}");
}

/// Store `planes` on staging's row, creating the row as a publish would.
async fn set_staging_planes(t: &Tenant, app_id: Uuid, planes: serde_json::Value) {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    let changed =
        t.db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE app_environments SET planes = $2 WHERE app_id = $1 AND name = 'staging'",
            [app_id.into(), planes.into()],
        ))
        .await
        .expect("store the planes")
        .rows_affected();
    assert_eq!(changed, 1, "the publish created staging's row");
}
