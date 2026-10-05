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
//! - a caller who may not open staging gets the same `404 EnvironmentRefused`
//!   a staging ask refuses with, and no run row either.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use entity::customer_app_procedure_runs as proc_run;
use entity::users::UserStatus;
use oxy_app::agentic_wiring::OxyThreadOwnerLookup;
use oxy_app::server::api::custom_apps_staging_held::{HeldEntry, HeldQuery, list_held};
use oxy_app::server::api::projects::automation_run::start_automation_run;
use oxy_app::server::router::AppState;
use oxy_app_core::serve_mode::ServeMode;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::{EntityTrait, PaginatorTrait};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{Tenant, publish_build, seeded_tenant};
use crate::staging_functions::{held_rows, make_guest_staff, production_host, staging_host};

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
