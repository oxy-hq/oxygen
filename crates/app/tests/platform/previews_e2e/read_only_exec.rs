//! What a preview request EXECUTES cannot write — the execution-layer half of
//! the read-only rule (`previews::request_hold`).
//!
//! The routes here are ALLOWED by `previews::read_only`: they are worth
//! serving in a preview, and they run the branch's own SQL or automations. So
//! a write they reach has to be stopped where it would execute, not by the
//! route's name. Through the real `workspace_middleware`, against the branch's
//! compiled revision, with this suite's recording warehouse and egress proxy:
//!
//! * (a) chat — the platform an analytics run is built on gets no automation
//!   runner, and the branch procedure a delegation would run, run on that
//!   platform off the request's task, sends its SELECT and never its INSERT;
//! * (b) a data app whose task deletes is refused; one that reads renders, and
//!   its POST task is not sent;
//! * (c) `/sql/{path}` and `/sql/query` refuse DML and serve a SELECT;
//! * (d) without the header, each is production as before.

use std::path::PathBuf;
use std::sync::Arc;

use agentic_pipeline::platform::PlatformContext;
use axum::extract::Extension;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use serde_json::{Value, json};
use uuid::Uuid;

use super::fakes::FakeEgress;
use super::fixture::{Fx, HOOK, PROCEDURE, WRITE_SQL, call, ready_preview, revision_of, setup};
use super::worker;

const WRITES_APP: &str = "apps/writes.app.yml";
const READS_APP: &str = "apps/reads.app.yml";
const CLEANUP_SQL: &str = "queries/cleanup.sql";
const DELETE_SQL: &str = "DELETE FROM analytics.orders WHERE 1";
const READ_SQL: &str = "SELECT count() AS n FROM analytics.orders";
/// A statement the fake warehouse received that no preview may send.
const WRITE_VERBS: [&str; 3] = ["DELETE", "INSERT", "ALTER"];

/// The branch's files this module reads, beside the fixture's procedure: a
/// data app that deletes, one that reads (and POSTs), a `.sql` file with DML.
pub(super) fn branch_files() -> Vec<(&'static str, String)> {
    let app = |name: &str, tasks: &str| {
        format!("name: {name}\ntasks:\n{tasks}display:\n  - type: markdown\n    content: {name}\n")
    };
    vec![
        (
            WRITES_APP,
            app(
                "writes",
                &format!(
                    "  - name: purge\n    type: execute_sql\n    database: warehouse\n    sql_query: {DELETE_SQL}\n"
                ),
            ),
        ),
        (
            READS_APP,
            app(
                "reads",
                &format!(
                    "  - name: orders\n    type: execute_sql\n    database: warehouse\n    sql_query: {READ_SQL}\n\
                     \x20 - name: notify\n    type: http_request\n    method: post\n    url: {HOOK}\n    body: '{{\"viewed\": true}}'\n"
                ),
            ),
        ),
        (CLEANUP_SQL, format!("{DELETE_SQL};\n")),
    ]
}

/// Mounted beside the fixture's `/databases` behind the real workspace
/// middleware, as `build_protected_routes` mounts them.
pub(super) fn routes(state: &oxy_app::server::router::AppState) -> Router {
    use oxy_app::server::api::{app, data};
    use oxy_app::server::router::{FleetState, IdeState};
    Router::new()
        .route(
            "/apps/{pathb64}/run",
            post(app::run_app).with_state(IdeState(state.clone())),
        )
        .route(
            "/sql/query",
            post(data::execute_sql_query).with_state(FleetState(state.clone())),
        )
        .route(
            "/sql/{pathb64}",
            post(data::execute_sql).with_state(FleetState(state.clone())),
        )
        // A GET, so the route table lets it through: what is under test is
        // the platform, not the route.
        .route("/probe/delegation", get(delegation_probe))
}

/// The platform the middleware installs is the one `POST /analytics/runs`
/// builds its run on. Reports whether the pipeline's gate would give it an
/// automation runner, then runs the branch procedure on it the way a
/// delegated child would — on a spawned task, off the request's, where the
/// run is driven.
async fn delegation_probe(Extension(platform): Extension<Arc<dyn PlatformContext>>) -> Json<Value> {
    let runner = agentic_pipeline::platform::preview::automation_subrun_runner(
        &platform,
        vec![PathBuf::from(PROCEDURE)],
    )
    .is_some();
    let preview = platform.is_workspace_preview();
    // The metadata a chat run started on this platform is inserted with.
    let mut stamp = json!({});
    agentic_pipeline::platform::preview_stamp::stamp(platform.as_ref(), &mut stamp);
    // Only the branch has the procedure; live reports that and runs nothing.
    let delegated = match platform.resolve_automation_yaml(PROCEDURE).await {
        Ok(yaml) => run_off_task(platform, &yaml).await,
        Err(e) => format!("not in this revision: {e}"),
    };
    Json(json!({
        "runner": runner,
        "preview": preview,
        "delegated": delegated,
        "stamp": stamp,
    }))
}

/// Run `yaml` on `platform` on a spawned task; `"ok"` or the run's error.
async fn run_off_task(platform: Arc<dyn PlatformContext>, yaml: &str) -> String {
    let config: agentic_automation::AutomationConfig =
        serde_yaml::from_str(yaml).expect("the procedure parses");
    tokio::spawn(async move {
        let workspace: Arc<dyn agentic_automation::WorkspaceContext> = platform;
        agentic_pipeline::automation_run::run_inline_automation(workspace.as_ref(), config, None)
            .await
            .map_or_else(|e| e.to_string(), |_| "ok".to_string())
    })
    .await
    .unwrap()
}

fn b64(path: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(path)
}

fn writes_sent(fx: &Fx) -> Vec<String> {
    fx.warehouse
        .statements()
        .into_iter()
        .filter(|s| WRITE_VERBS.iter().any(|v| s.to_uppercase().contains(v)))
        .collect()
}

async fn staging(fx: &Fx) -> Uuid {
    revision_of(&ready_preview(fx).await)
}

/// (a) + (d): chat in a preview cannot reach a branch automation, and what a
/// delegation would run holds its write; production still delegates.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_in_a_preview_cannot_delegate_a_write() {
    let fx = setup().await;
    let _worker = worker::start(&fx).await;
    let staging = staging(&fx).await;
    let probe = format!("/{}/probe/delegation", fx.ws);

    let pinned = call(&fx.staff, "GET", probe.clone(), None, Some(staging)).await;
    assert_eq!(pinned.status, StatusCode::OK, "{}", pinned.body);
    assert_eq!(pinned.body["preview"], true, "{}", pinned.body);
    assert_eq!(
        pinned.body["runner"], false,
        "an agent in a preview gets no automation runner: {}",
        pinned.body
    );
    let delegated = pinned.body["delegated"].as_str().unwrap();
    assert!(
        delegated.contains("cannot be written in a workspace preview"),
        "the branch procedure's INSERT is refused: {delegated}"
    );
    let sent = fx.warehouse.statements();
    assert!(
        sent.iter().any(|s| s.contains("analytics.orders")),
        "its SELECT still runs: {sent:?}"
    );
    assert_eq!(
        writes_sent(&fx),
        Vec::<String>::new(),
        "{WRITE_SQL} never sent"
    );
    // A chat run started here is stamped with the revision it reads, so
    // recovery retires it rather than resuming it on production.
    assert_eq!(
        pinned.body["stamp"]["workspace_preview"]["revision_id"],
        staging.to_string(),
        "{}",
        pinned.body
    );

    // (d) Production: no header, the same platform delegates as before.
    let live = call(&fx.staff, "GET", probe, None, None).await;
    assert_eq!(live.status, StatusCode::OK, "{}", live.body);
    assert_eq!(live.body["preview"], false);
    assert_eq!(live.body["runner"], true, "production still delegates");
    assert_eq!(
        live.body["stamp"],
        json!({}),
        "a production run is unstamped"
    );
}

/// (b): rendering a branch data app whose task deletes is refused and sends
/// nothing; one that reads renders, and its POST task is not sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_data_app_in_a_preview_reads_and_cannot_write() {
    let fx = setup().await;
    let egress = FakeEgress::start().await;
    egress.route_https_egress_here();
    let _worker = worker::start(&fx).await;
    let staging = staging(&fx).await;
    let run = |app: &str| format!("/{}/apps/{}/run", fx.ws, b64(app));

    let writes = call(
        &fx.staff,
        "POST",
        run(WRITES_APP),
        Some(json!({})),
        Some(staging),
    )
    .await;
    assert_eq!(writes.status, StatusCode::OK, "{}", writes.body);
    let error = writes.body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("cannot be written in a workspace preview"),
        "the DELETE task is refused: {}",
        writes.body
    );
    assert_eq!(
        writes_sent(&fx),
        Vec::<String>::new(),
        "no DELETE reached the warehouse"
    );

    let reads = call(
        &fx.staff,
        "POST",
        run(READS_APP),
        Some(json!({})),
        Some(staging),
    )
    .await;
    assert_eq!(reads.status, StatusCode::OK, "{}", reads.body);
    assert_eq!(
        reads.body["error"],
        Value::Null,
        "the reading app renders: {}",
        reads.body
    );
    assert!(
        fx.warehouse
            .statements()
            .iter()
            .any(|s| s.contains(READ_SQL)),
        "its SELECT ran: {:?}",
        fx.warehouse.statements()
    );
    assert_eq!(
        egress.tunnels(),
        Vec::<String>::new(),
        "its POST task was held, not sent"
    );
}

/// (c) + (d): `/sql/{path}` and `/sql/query` refuse DML in a preview and serve
/// a SELECT; without the header the same DML is sent, as production always has.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sql_in_a_preview_serves_reads_and_refuses_dml() {
    let fx = setup().await;
    let _worker = worker::start(&fx).await;
    let staging = staging(&fx).await;
    let file = format!("/{}/sql/{}", fx.ws, b64(CLEANUP_SQL));
    let query = format!("/{}/sql/query", fx.ws);
    let body = |sql: &str| Some(json!({ "sql": sql, "database": "warehouse" }));

    for uri in [&file, &query] {
        let dml = call(
            &fx.staff,
            "POST",
            uri.clone(),
            body(DELETE_SQL),
            Some(staging),
        )
        .await;
        // The preview's refusal, as a refused route answers it — not a 500.
        assert_eq!(dml.status, StatusCode::CONFLICT, "{uri}: {}", dml.body);
        assert_eq!(dml.body["code"], "preview_read_only", "{uri}: {}", dml.body);
        assert!(
            dml.body
                .to_string()
                .contains("cannot be written in a workspace preview"),
            "{uri}: {}",
            dml.body
        );
        let select = call(
            &fx.staff,
            "POST",
            uri.clone(),
            body(READ_SQL),
            Some(staging),
        )
        .await;
        assert_eq!(select.status, StatusCode::OK, "{uri}: {}", select.body);
    }
    assert_eq!(
        writes_sent(&fx),
        Vec::<String>::new(),
        "no DML reached the warehouse"
    );
    assert!(
        fx.warehouse
            .statements()
            .iter()
            .any(|s| s.contains(READ_SQL)),
        "the SELECTs ran"
    );

    // (d) Production is unchanged: the same DML, no header, is sent.
    let live = call(&fx.staff, "POST", query, body(DELETE_SQL), None).await;
    assert_ne!(live.status, StatusCode::CONFLICT, "{}", live.body);
    assert_eq!(
        writes_sent(&fx).len(),
        1,
        "without the header the DELETE goes to the warehouse: {:?}",
        fx.warehouse.statements()
    );
}
