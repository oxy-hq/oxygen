//! The real `start_automation_run` / `poll_automation_run` handlers on a
//! custom app's staging host under `workspace-actions: preview`
//! (`projects::automation_run::preview_mode`).
//!
//! - the start answers `202 { run_id, mode: "preview" }` and writes the poll
//!   row and the `workspace_preview_runs` row under one id, on the workspace's
//!   promoted revision — or on the environment's semantic pin when it has one;
//! - `OXY_PREVIEW_RUNS` off answers `409 preview_runs_disabled` and writes
//!   nothing: a preview run never falls back to a live run;
//! - the poll reads `running` until the preview sweep settles the row from
//!   `agentic_runs`, and a run that ended `failed` carries its own error, not
//!   the orphan reconcile's;
//! - a run still queued when the bundle cancels it (or when the stuck-run sweep
//!   fails its poll row) is finished, so it never starts later;
//! - the default `held` mode is unchanged.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use entity::customer_app_procedure_runs as proc_run;
use oxy_app::surface::testing::agentic_wiring::OxyThreadOwnerLookup;
use oxy_app::surface::testing::compile_config_gate::runtime_config_gate;
use oxy_app::surface::testing::previews::runs::sweep;
use oxy_app::surface::testing::projects::automation_run::{
    cancel_automation_run, poll_automation_run, start_automation_run, sweep_terminal_runs,
};
use oxy_app_core::AppState;
use oxy_app_core::serve_mode::ServeMode;
use oxy_compile::{CompileRequest, RevisionKind, compile_workspace, compiler_version};
use sea_orm::{ConnectionTrait, DatabaseBackend, EntityTrait, Statement};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

use crate::common::{demo_workspace_id, examples_path};
use crate::custom_app_functions_fixture::{Tenant, publish_build, seeded_tenant};
use crate::staging_functions::{held_rows, make_guest_staff, staging_host};

const AUTOMATION_ID: &str = "anonymize";
const AUTOMATION_PATH: &str = "procedures/anonymize.automation.yml";

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

fn on_host(host: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("host", HeaderValue::from_str(host).unwrap());
    h
}

fn enable_runs(on: bool) {
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        if on {
            std::env::set_var("OXY_PREVIEW_RUNS", "1");
        } else {
            std::env::remove_var("OXY_PREVIEW_RUNS");
        }
    }
}

/// Compile the one automation these tests start as a revision of the demo
/// workspace; `Main` + promote makes it the promoted revision, `Staging`
/// leaves a ready revision to pin. Returns the revision's id.
async fn compile(t: &Tenant, kind: RevisionKind, promote: bool, sha: Option<&str>) -> Uuid {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("config.yml"), "databases: []\nmodels: []\n").unwrap();
    std::fs::create_dir_all(dir.path().join("procedures")).unwrap();
    std::fs::copy(
        examples_path().join(AUTOMATION_PATH),
        dir.path().join(AUTOMATION_PATH),
    )
    .expect("copy the example automation");
    compile_workspace(CompileRequest {
        db: &t.db,
        workspace_id: demo_workspace_id(),
        workspace_path: dir.path(),
        git_sha: sha.map(str::to_string),
        branch: None,
        compiler_version: compiler_version(),
        promote,
        kind,
        owner_user_id: None,
        config_gate: Some(runtime_config_gate()),
    })
    .await
    .expect("compile the workspace")
    .revision_id
}

async fn exec(t: &Tenant, sql: &str, values: Vec<sea_orm::Value>) -> u64 {
    t.db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .expect(sql)
    .rows_affected()
}

/// A published app whose staging environment runs automations in `mode`.
async fn app_in_mode(t: &Tenant, slug: &str, mode: Option<&str>) -> Uuid {
    let app_id = publish_build(t, slug, demo_workspace_id(), "automation-1", true, &[])
        .await
        .app_id;
    if let Some(mode) = mode {
        let n = exec(
            t,
            "UPDATE app_environments SET planes = $2 WHERE app_id = $1 AND name = 'staging'",
            vec![app_id.into(), json!({ "workspace-actions": mode }).into()],
        )
        .await;
        assert_eq!(n, 1, "the staging environment row exists");
    }
    make_guest_staff();
    app_id
}

async fn json_of(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

async fn start(t: &Tenant, slug: &str) -> (StatusCode, serde_json::Value) {
    start_with(t, slug, None).await
}

/// `start`, with the `x-oxy-app` header naming `claimed_app` when given.
async fn start_with(
    t: &Tenant,
    slug: &str,
    claimed_app: Option<Uuid>,
) -> (StatusCode, serde_json::Value) {
    let mut headers = on_host(&staging_host(t, slug));
    if let Some(app) = claimed_app {
        headers.insert(
            "x-oxy-app",
            HeaderValue::from_str(&app.to_string()).unwrap(),
        );
    }
    let resp = Box::pin(start_automation_run(
        State(state(t)),
        Path((demo_workspace_id(), AUTOMATION_ID.to_string())),
        headers,
        Bytes::from(json!({ "params": { "k": "v" } }).to_string()),
    ))
    .await;
    json_of(resp).await
}

async fn poll(t: &Tenant, slug: &str, run_id: &str) -> serde_json::Value {
    let resp = Box::pin(poll_automation_run(
        State(state(t)),
        Path((demo_workspace_id(), run_id.to_string())),
        on_host(&staging_host(t, slug)),
    ))
    .await;
    let (status, body) = json_of(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn cancel(t: &Tenant, slug: &str, run_id: &str) -> StatusCode {
    Box::pin(cancel_automation_run(
        State(state(t)),
        Path((demo_workspace_id(), run_id.to_string())),
        on_host(&staging_host(t, slug)),
    ))
    .await
    .status()
}

/// Whether the run was ever started: its `agentic_runs` row exists only once
/// the queue started it.
async fn was_started(t: &Tenant, run_id: &str) -> bool {
    let row =
        t.db.query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*) AS n FROM agentic_runs WHERE id = $1",
            [run_id.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get::<i64>("", "n").unwrap() > 0
}

async fn count(t: &Tenant, table: &str) -> i64 {
    let row =
        t.db.query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            format!("SELECT count(*) AS n FROM {table}"),
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get("", "n").unwrap()
}

/// The `workspace_preview_runs` row of `run_id`: (revision, branch, kind,
/// target_ref, state, options).
async fn preview_row(
    t: &Tenant,
    run_id: &str,
) -> (
    Uuid,
    String,
    String,
    Option<String>,
    String,
    serde_json::Value,
) {
    let row =
        t.db.query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT revision_id, branch, kind, target_ref, state, options \
             FROM workspace_preview_runs WHERE run_id = $1",
            [run_id.into()],
        ))
        .await
        .unwrap()
        .expect("a workspace_preview_runs row with the run's id");
    (
        row.try_get("", "revision_id").unwrap(),
        row.try_get("", "branch").unwrap(),
        row.try_get("", "kind").unwrap(),
        row.try_get("", "target_ref").unwrap(),
        row.try_get("", "state").unwrap(),
        row.try_get("", "options").unwrap(),
    )
}

async fn promoted_revision(t: &Tenant) -> Uuid {
    entity::workspaces::Entity::find_by_id(demo_workspace_id())
        .one(&t.db)
        .await
        .unwrap()
        .unwrap()
        .current_revision_id
        .expect("promoted")
}

/// Make the run's `agentic_runs` row terminal, as the global-run driver would
/// (the row exists once the queue started the run; insert it when it does not).
async fn end_agentic_run(t: &Tenant, run_id: &str, status: &str, error: Option<&str>) {
    let n = exec(
        t,
        "UPDATE agentic_runs SET task_status = $2, error_message = $3 WHERE id = $1",
        vec![
            run_id.into(),
            status.into(),
            error.map(str::to_string).into(),
        ],
    )
    .await;
    assert_eq!(
        n, 1,
        "the queue started the run, so its agentic_runs row exists"
    );
}

#[tokio::test]
async fn preview_mode_queues_preview_run() {
    enable_runs(true);
    let t = seeded_tenant().await;
    let slug = "prv-automation-queue";
    let app_id = app_in_mode(&t, slug, Some("preview")).await;
    compile(&t, RevisionKind::Main, true, None).await;
    let promoted = promoted_revision(&t).await;

    let (status, body) = start(&t, slug).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["mode"], "preview", "{body}");
    let run_id = body["run_id"].as_str().expect("run_id").to_string();
    let poll_row = proc_run::Entity::find_by_id(Uuid::parse_str(&run_id).unwrap())
        .one(&t.db)
        .await
        .unwrap()
        .expect("the poll row has the run's id");
    assert_eq!(poll_row.status, "running");
    assert_eq!(poll_row.procedure_id, AUTOMATION_ID);

    let (revision, branch, kind, target, state, options) = preview_row(&t, &run_id).await;
    assert_eq!(revision, promoted, "no pin: the promoted revision");
    assert_eq!(branch, format!("app-env/{app_id}/staging"));
    assert_eq!(kind, "procedure");
    assert_eq!(target.as_deref(), Some(AUTOMATION_PATH));
    assert!(state == "queued" || state == "running", "{state}");
    assert_eq!(options["variables"], json!({ "k": "v" }));
    assert_eq!(options["read_live_only"], false);
    assert_eq!(options["custom_app"]["app_id"], json!(app_id));
    assert_eq!(options["custom_app"]["environment"], "staging");

    let held = held_rows(&t).await;
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0].metadata["mode"], "preview");
}

#[tokio::test]
async fn preview_mode_uses_env_pin() {
    enable_runs(true);
    let t = seeded_tenant().await;
    let slug = "prv-automation-pin";
    let app_id = app_in_mode(&t, slug, Some("preview")).await;
    let promoted = compile(&t, RevisionKind::Main, true, None).await;
    let pinned = compile(&t, RevisionKind::Staging, false, Some("sha-feat")).await;
    assert_ne!(pinned, promoted);
    exec(
        &t,
        "UPDATE app_environments SET semantic_revision_id = $2, semantic_git_sha = 'sha-feat', \
         planes = planes || '{\"semantic\": \"branch:feat\"}'::jsonb \
         WHERE app_id = $1 AND name = 'staging'",
        vec![app_id.into(), pinned.into()],
    )
    .await;

    let (status, body) = start(&t, slug).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let (revision, ..) = preview_row(&t, body["run_id"].as_str().unwrap()).await;
    assert_eq!(revision, pinned, "the environment's pin, not production's");
}

#[tokio::test]
async fn preview_runs_disabled_answers_409() {
    enable_runs(false);
    let t = seeded_tenant().await;
    let slug = "prv-automation-off";
    app_in_mode(&t, slug, Some("preview")).await;
    compile(&t, RevisionKind::Main, true, None).await;

    let (status, body) = start(&t, slug).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "preview_runs_disabled", "{body}");
    assert_eq!(
        count(&t, "customer_app_automation_runs").await,
        0,
        "no live run"
    );
    assert_eq!(count(&t, "workspace_preview_runs").await, 0);
}

#[tokio::test]
async fn preview_run_polls_running_then_done() {
    enable_runs(true);
    let t = seeded_tenant().await;
    let slug = "prv-automation-poll";
    app_in_mode(&t, slug, Some("preview")).await;
    compile(&t, RevisionKind::Main, true, None).await;
    let (_, body) = start(&t, slug).await;
    let run_id = body["run_id"].as_str().unwrap().to_string();

    assert_eq!(poll(&t, slug, &run_id).await["status"], "running");

    end_agentic_run(&t, &run_id, "done", None).await;
    // Terminal in `agentic_runs`, not yet swept: still open for the bundle.
    assert_eq!(poll(&t, slug, &run_id).await["status"], "running");
    sweep(&t.db).await;

    let done = poll(&t, slug, &run_id).await;
    assert_eq!(done["status"], "done", "{done}");
    assert!(done["result"]["summary"].is_string(), "{done}");
}

/// A preview run that ended `failed` is closed from its own error. The orphan
/// reconcile on the poll must leave it alone: it would close it as
/// `automation_run_orphaned`.
#[tokio::test]
async fn preview_run_failure_keeps_its_own_error_not_the_orphan_code() {
    enable_runs(true);
    let t = seeded_tenant().await;
    let slug = "prv-automation-failed";
    app_in_mode(&t, slug, Some("preview")).await;
    compile(&t, RevisionKind::Main, true, None).await;
    let (_, body) = start(&t, slug).await;
    let run_id = body["run_id"].as_str().unwrap().to_string();
    // The queue entry is not held by a claim, the state the reconcile closes on.
    exec(
        &t,
        "DELETE FROM agentic_task_queue WHERE run_id = $1",
        vec![run_id.clone().into()],
    )
    .await;

    end_agentic_run(&t, &run_id, "failed", Some("warehouse said no")).await;
    assert_eq!(poll(&t, slug, &run_id).await["status"], "running");
    sweep(&t.db).await;

    let failed = poll(&t, slug, &run_id).await;
    assert_eq!(failed["status"], "failed", "{failed}");
    assert_eq!(failed["error"]["message"], "warehouse said no", "{failed}");
    assert_ne!(
        failed["error"]["code"], "automation_run_orphaned",
        "{failed}"
    );
}

/// A preview run waits behind the workspace's running one. If the bundle is
/// told it was cancelled (the cancel endpoint), or that it failed (the
/// stuck-run sweep), it must not start when the slot frees: its
/// `workspace_preview_runs` row is finished with the poll row, and no
/// `agentic_runs` row is ever made for it.
#[tokio::test]
async fn a_queued_preview_run_ended_for_the_bundle_never_starts() {
    enable_runs(true);
    let t = seeded_tenant().await;
    let slug = "prv-automation-queued-end";
    app_in_mode(&t, slug, Some("preview")).await;
    compile(&t, RevisionKind::Main, true, None).await;
    let mut run_ids = Vec::new();
    for _ in 0..3 {
        let (status, body) = start(&t, slug).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        run_ids.push(body["run_id"].as_str().expect("run_id").to_string());
    }
    let [running, cancelled, stuck] = [&run_ids[0], &run_ids[1], &run_ids[2]];
    assert_eq!(preview_row(&t, running).await.4, "running");
    assert_eq!(
        preview_row(&t, cancelled).await.4,
        "queued",
        "waits behind it"
    );
    assert_eq!(preview_row(&t, stuck).await.4, "queued");

    // The cancel endpoint: the queued run is finished with its poll row.
    assert_eq!(cancel(&t, slug, cancelled).await, StatusCode::NO_CONTENT);
    assert_eq!(preview_row(&t, cancelled).await.4, "finished");
    assert_eq!(poll(&t, slug, cancelled).await["status"], "cancelled");

    // The stuck-run sweep: a poll row it fails ends its queued run too.
    let aged = exec(
        &t,
        "UPDATE customer_app_automation_runs \
         SET started_at = now() - interval '3 hours' WHERE id = $1",
        vec![Uuid::parse_str(stuck).unwrap().into()],
    )
    .await;
    assert_eq!(aged, 1, "the stuck run's poll row");
    let report = sweep_terminal_runs(&t.db).await.expect("sweep");
    assert!(report.stuck_failed >= 1, "{report:?}");
    assert_eq!(preview_row(&t, stuck).await.4, "finished");

    // The slot frees: the sweep advances the queue, and nothing is waiting.
    end_agentic_run(&t, running, "done", None).await;
    sweep(&t.db).await;
    for run_id in [cancelled, stuck] {
        assert_eq!(preview_row(&t, run_id).await.4, "finished", "{run_id}");
        assert!(!was_started(&t, run_id).await, "{run_id} never started");
    }
}

#[tokio::test]
async fn held_mode_unchanged() {
    enable_runs(true);
    let t = seeded_tenant().await;
    let slug = "prv-automation-held";
    app_in_mode(&t, slug, None).await;

    let (status, body) = start(&t, slug).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "held_in_staging", "{body}");
    assert_eq!(count(&t, "customer_app_automation_runs").await, 0);
    assert_eq!(count(&t, "workspace_preview_runs").await, 0);
    let held = held_rows(&t).await;
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0].metadata["mode"], "automation");
}

#[tokio::test]
async fn refused_mode_answers_404() {
    enable_runs(true);
    let t = seeded_tenant().await;
    let slug = "prv-automation-refused";
    app_in_mode(&t, slug, Some("refused")).await;

    let (status, body) = start(&t, slug).await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "EnvironmentRefused", "{body}");
    assert!(held_rows(&t).await.is_empty());
}

/// The pin is the Host app's, whatever `x-oxy-app` claims: the gate resolves
/// its pin from the header's app, the hold authorizes the Host's, and a run
/// must read the pin of the app it was authorized for.
#[tokio::test]
async fn preview_mode_pin_follows_the_host_app_not_the_x_oxy_app_header() {
    enable_runs(true);
    let t = seeded_tenant().await;
    let slug = "prv-automation-hdr-pin";
    let app_id = app_in_mode(&t, slug, Some("preview")).await;
    let other = app_in_mode(&t, "prv-automation-hdr-other", None).await;
    let promoted = compile(&t, RevisionKind::Main, true, None).await;
    let pinned = compile(&t, RevisionKind::Staging, false, Some("sha-feat")).await;
    assert_ne!(pinned, promoted);
    exec(
        &t,
        "UPDATE app_environments SET semantic_revision_id = $2, semantic_git_sha = 'sha-feat', \
         planes = planes || '{\"semantic\": \"branch:feat\"}'::jsonb \
         WHERE app_id = $1 AND name = 'staging'",
        vec![app_id.into(), pinned.into()],
    )
    .await;

    let (status, body) = start_with(&t, slug, Some(other)).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let (revision, ..) = preview_row(&t, body["run_id"].as_str().unwrap()).await;
    assert_eq!(
        revision, pinned,
        "the Host app's pin, not the header app's (none)"
    );
}

/// A pin that is set but not ready is refused even when a header names an app
/// whose gate pin is empty: never production's revision.
#[tokio::test]
async fn preview_mode_not_ready_pin_is_refused_despite_a_mismatching_header() {
    enable_runs(true);
    let t = seeded_tenant().await;
    let slug = "prv-automation-hdr-notready";
    let app_id = app_in_mode(&t, slug, Some("preview")).await;
    let other = app_in_mode(&t, "prv-automation-hdr-other2", None).await;
    compile(&t, RevisionKind::Main, true, None).await;
    exec(
        &t,
        "UPDATE app_environments SET semantic_revision_id = NULL, semantic_git_sha = 'sha-new', \
         planes = planes || '{\"semantic\": \"branch:feat\"}'::jsonb \
         WHERE app_id = $1 AND name = 'staging'",
        vec![app_id.into()],
    )
    .await;

    let (status, body) = start_with(&t, slug, Some(other)).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"], "semantic_branch_not_ready", "{body}");
    assert_eq!(count(&t, "customer_app_automation_runs").await, 0);
    assert_eq!(count(&t, "workspace_preview_runs").await, 0);
}

/// A staging host cancels only its own environment's preview runs: another
/// app's run (by id) is refused `404`, and stays as it was.
#[tokio::test]
async fn a_staging_cancel_of_another_apps_run_is_refused() {
    enable_runs(true);
    let t = seeded_tenant().await;
    let mine = "prv-automation-cancel-mine";
    let other = "prv-automation-cancel-other";
    app_in_mode(&t, mine, Some("preview")).await;
    app_in_mode(&t, other, Some("preview")).await;
    compile(&t, RevisionKind::Main, true, None).await;
    let (status, body) = start(&t, other).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id = body["run_id"].as_str().expect("run_id").to_string();
    let before = preview_row(&t, &run_id).await.4;

    assert_eq!(cancel(&t, mine, &run_id).await, StatusCode::NOT_FOUND);
    assert_eq!(preview_row(&t, &run_id).await.4, before, "not ended");
    assert_ne!(poll(&t, other, &run_id).await["status"], "cancelled");

    assert_eq!(cancel(&t, other, &run_id).await, StatusCode::NO_CONTENT);
}
