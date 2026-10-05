//! The real `start_ask` and `cancel_ask` handlers on a custom app's staging
//! host (`projects::agent_ask`, `agent_ask_staging`), called with the state
//! the router hands them. No LLM: the cases stop at a refusal, or at an agent
//! that does not exist.
//!
//! - a caller who may not open staging gets `404 EnvironmentRefused` from
//!   `start_ask`, and no thread or message row is written;
//! - staff with an unknown agent get `agent_not_found`: the hold decision
//!   passed and `builder.start` ran (inside the hold);
//! - the staging host's `cancel_ask` reaches only a run this app's staging ask
//!   stamped — a production run and another app's are 404 — and production's
//!   cancel is unchanged;
//! - chat history is per environment: a staging ask's thread is
//!   `custom-app-staging:<app_id>`, absent from production's `list_threads`
//!   and present in staging's; a production thread id sent from staging is
//!   the ownership miss's 404, and a staging follow-up on its own staging
//!   thread reaches the pipeline.

use std::sync::Arc;

use agentic_pipeline::platform::preview_stamp::RUN_STAMP;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use entity::{messages, threads};
use oxy_app::agentic_wiring::OxyThreadOwnerLookup;
use oxy_app::server::api::custom_apps_threads::{get_thread_transcript, list_threads};
use oxy_app::server::api::projects::agent_ask::{cancel_ask, start_ask};
use oxy_app::server::router::AppState;
use oxy_app_core::serve_mode::ServeMode;
use sea_orm::{ActiveValue, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};
use serde_json::json;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{Tenant, publish_build, seeded_tenant};
use crate::staging_functions::{make_guest_staff, production_host, staging_host};

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
    publish_build(t, slug, demo_workspace_id(), "ask-1", true, &[])
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

async fn ask(t: &Tenant, host: &str, agent: &str) -> (StatusCode, serde_json::Value) {
    ask_on(t, host, agent, None).await
}

/// An ask continuing `thread` when set.
async fn ask_on(
    t: &Tenant,
    host: &str,
    agent: &str,
    thread: Option<Uuid>,
) -> (StatusCode, serde_json::Value) {
    let mut body = json!({ "v": 1, "question": "how many orders?" });
    if let Some(tid) = thread {
        body["thread_id"] = json!(tid.to_string());
    }
    // Boxed, as axum boxes a handler's future.
    let resp = Box::pin(start_ask(
        State(state(t)),
        Path((demo_workspace_id(), agent.to_string())),
        on_host(host),
        Bytes::from(body.to_string()),
    ))
    .await;
    json_of(resp).await
}

async fn rows(t: &Tenant) -> (u64, u64) {
    (
        threads::Entity::find().count(&t.db).await.unwrap(),
        messages::Entity::find().count(&t.db).await.unwrap(),
    )
}

#[tokio::test]
async fn start_ask_on_staging_refuses_a_non_staff_caller_before_writing_a_thread() {
    let t = seeded_tenant().await;
    let slug = "stg-ask-h404";
    app(&t, slug).await;
    let before = rows(&t).await;

    let (status, body) = ask(&t, &staging_host(&t, slug), "no_such_agent").await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "EnvironmentRefused", "{body}");
    assert_eq!(rows(&t).await, before, "no thread or message was written");
}

/// Drive `fut` as a task on a worker of a runtime built the way the server's
/// is (`crates/server/src/main.rs`): multi-thread, tokio's default 2 MiB
/// worker stack. Not `#[tokio::test]`, whose `block_on` polls on the test
/// thread instead of a worker. Do not give this runtime a bigger stack: a
/// handler that needs one aborts the real server (`start_ask` once did, in
/// a debug build, with "has overflowed its stack"), and this is the check
/// that says so first.
fn on_a_server_stack<F>(fut: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async { tokio::spawn(fut).await.expect("test task") });
}

/// The thread and question are written before `builder.start` (as in
/// production), so this proves the order: the hold decision passed, and
/// `builder.start` — which `start_ask` runs inside the hold — answered.
#[test]
fn start_ask_on_staging_for_staff_reaches_the_pipeline() {
    on_a_server_stack(staff_reaches_the_pipeline("stg-ask-hstaff", Env::Staging));
}

/// The production host takes the same path with no hold, on the same stack.
#[test]
fn start_ask_on_production_reaches_the_pipeline() {
    on_a_server_stack(staff_reaches_the_pipeline("stg-ask-hprod", Env::Production));
}

enum Env {
    Staging,
    Production,
}

async fn staff_reaches_the_pipeline(slug: &str, env: Env) {
    let t = seeded_tenant().await;
    app(&t, slug).await;
    make_guest_staff();
    let host = match env {
        Env::Staging => staging_host(&t, slug),
        Env::Production => production_host(&t, slug),
    };

    let (status, body) = ask(&t, &host, "no_such_agent").await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "agent_not_found", "{body}");
}

/// A run in the demo workspace, with `metadata`.
async fn run(t: &Tenant, metadata: serde_json::Value) -> String {
    let id = format!("ask-{}", Uuid::new_v4().simple());
    agentic_runtime::crud::insert_run(
        &t.db,
        &id,
        "q",
        None,
        "analytics",
        Some(metadata),
        demo_workspace_id(),
    )
    .await
    .expect("insert run");
    id
}

fn stamped(app: Uuid) -> serde_json::Value {
    json!({ "agent_id": "a", RUN_STAMP: { "revision_id": null, "app_id": app } })
}

async fn cancel(t: &Tenant, host: &str, run_id: &str) -> StatusCode {
    Box::pin(cancel_ask(
        State(state(t)),
        Path((demo_workspace_id(), run_id.to_string())),
        on_host(host),
    ))
    .await
    .status()
}

#[tokio::test]
async fn staging_cancel_reaches_only_this_apps_staging_runs() {
    let t = seeded_tenant().await;
    let slug = "stg-ask-cancel";
    let app_id = app(&t, slug).await;
    make_guest_staff();
    let staging = staging_host(&t, slug);

    let production_run = run(&t, json!({ "agent_id": "a" })).await;
    let other_apps_run = run(&t, stamped(Uuid::new_v4())).await;
    let own_run = run(&t, stamped(app_id)).await;

    assert_eq!(
        cancel(&t, &staging, &production_run).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        cancel(&t, &staging, &other_apps_run).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(cancel(&t, &staging, &own_run).await, StatusCode::NO_CONTENT);
    let row = agentic_runtime::crud::get_run(&t.db, &production_run)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.task_status.as_deref(), Some("running"), "untouched");

    // Production's cancel is unchanged.
    assert_eq!(
        cancel(&t, &production_host(&t, slug), &production_run).await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn staging_cancel_refuses_a_caller_who_may_not_open_staging() {
    let t = seeded_tenant().await;
    let slug = "stg-ask-cancel404";
    let app_id = app(&t, slug).await;
    let own_run = run(&t, stamped(app_id)).await;
    assert_eq!(
        cancel(&t, &staging_host(&t, slug), &own_run).await,
        StatusCode::NOT_FOUND
    );
}

/// The guest's thread in the demo workspace, with `source`.
async fn guest_thread(t: &Tenant, source: &str) -> Uuid {
    // The guest the gates attach to a cookieless request.
    let user = oxy_auth::user::UserService::get_or_create_user(&oxy_auth::types::Identity {
        user_id: None,
        email: oxy_auth::user::LOCAL_GUEST_EMAIL.to_string(),
        name: Some("Local User".to_string()),
        picture: None,
    })
    .await
    .expect("guest user");
    let id = Uuid::new_v4();
    threads::Entity::insert(threads::ActiveModel {
        id: Set(id),
        user_id: Set(Some(user.id)),
        title: Set(format!("{source} thread")),
        input: Set("q".to_string()),
        output: Set(String::new()),
        source: Set(source.to_string()),
        source_type: Set("analytics".to_string()),
        references: Set("[]".to_string()),
        is_processing: Set(false),
        created_at: ActiveValue::not_set(),
        project_id: Set(demo_workspace_id()),
        sandbox_info: Set(None),
    })
    .exec(&t.db)
    .await
    .expect("insert thread");
    id
}

async fn listed(host: &str) -> Vec<Uuid> {
    let (status, body) =
        json_of(list_threads(Path(demo_workspace_id()), on_host(host)).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body.as_array()
        .expect("array")
        .iter()
        .map(|t| Uuid::parse_str(t["id"].as_str().unwrap()).unwrap())
        .collect()
}

async fn transcript(t: &Tenant, host: &str, thread: Uuid) -> StatusCode {
    get_thread_transcript(
        State(state(t)),
        Path((demo_workspace_id(), thread)),
        on_host(host),
    )
    .await
    .status()
}

#[tokio::test]
async fn chat_history_is_per_environment() {
    let t = seeded_tenant().await;
    let slug = "stg-ask-history";
    let app_id = app(&t, slug).await;
    make_guest_staff();
    let (staging, production) = (staging_host(&t, slug), production_host(&t, slug));

    let prod_thread = guest_thread(&t, "custom-app").await;
    let own_staging = guest_thread(&t, &format!("custom-app-staging:{app_id}")).await;
    let other_apps = guest_thread(&t, &format!("custom-app-staging:{}", Uuid::new_v4())).await;

    let on_prod = listed(&production).await;
    assert!(on_prod.contains(&prod_thread));
    assert!(
        !on_prod.contains(&own_staging),
        "staging thread in production history"
    );
    assert!(!on_prod.contains(&other_apps));

    let on_staging = listed(&staging).await;
    assert!(on_staging.contains(&own_staging));
    assert!(
        !on_staging.contains(&prod_thread),
        "production thread in staging history"
    );
    assert!(
        !on_staging.contains(&other_apps),
        "another app's staging thread"
    );

    assert_eq!(
        transcript(&t, &production, own_staging).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        transcript(&t, &staging, prod_thread).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        transcript(&t, &staging, other_apps).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(transcript(&t, &staging, own_staging).await, StatusCode::OK);
    assert_eq!(
        transcript(&t, &production, prod_thread).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn staging_history_refuses_a_caller_who_may_not_open_staging() {
    let t = seeded_tenant().await;
    let slug = "stg-ask-history404";
    app(&t, slug).await;
    let resp = list_threads(Path(demo_workspace_id()), on_host(&staging_host(&t, slug))).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// A staging ask writes a `custom-app-staging:<app_id>` thread; its follow-up
/// on that thread reaches the pipeline; a production thread id sent from
/// staging — and the staging thread sent from production — is the ownership
/// miss's 404, with no message appended.
#[test]
fn staging_follow_ups_stay_on_staging_threads() {
    on_a_server_stack(follow_ups("stg-ask-follow"));
}

async fn follow_ups(slug: &str) {
    let t = seeded_tenant().await;
    let app_id = app(&t, slug).await;
    make_guest_staff();
    let (staging, production) = (staging_host(&t, slug), production_host(&t, slug));
    let staging_source = format!("custom-app-staging:{app_id}");

    let (status, body) = ask(&t, &staging, "no_such_agent").await;
    assert_eq!(body["code"], "agent_not_found", "{status} {body}");
    let staged = threads::Entity::find()
        .filter(threads::Column::Source.eq(staging_source.as_str()))
        .all(&t.db)
        .await
        .unwrap();
    assert_eq!(staged.len(), 1, "the staging ask wrote one staging thread");
    let staging_thread = staged[0].id;

    // Its own staging thread: the follow-up passes the ownership check.
    let (status, body) = ask_on(&t, &staging, "no_such_agent", Some(staging_thread)).await;
    assert_eq!(body["code"], "agent_not_found", "{status} {body}");

    let prod_thread = guest_thread(&t, "custom-app").await;
    let before = rows(&t).await;
    let (status, body) = ask_on(&t, &staging, "no_such_agent", Some(prod_thread)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["message"], "thread not found", "{body}");
    let (status, body) = ask_on(&t, &production, "no_such_agent", Some(staging_thread)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["message"], "thread not found", "{body}");
    assert_eq!(rows(&t).await, before, "no message was appended");
}
