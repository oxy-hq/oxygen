//! The admin **Run now** for a published function, end to end:
//! `POST /api/admin/apps/{id}/functions/{name}/runs` queues it, the global-run
//! driver executes it in the isolate, and
//! `GET /api/admin/apps/{id}/function-runs/{run_id}` reports the run done with
//! the function's answer and its logs.
//!
//! **Routes.** The two handlers mount at their production paths (`RUNS_ROUTE`,
//! `RUN_DETAIL_ROUTE` from `admin::apps::router()`, nested at `/admin` by
//! `router::global` and under `/api` by `serve.rs`). They sit behind a copy of
//! the admin guards in production order: the `/admin` door
//! `oxy_owner_or_app_admin_guard`, then `block_admin_while_acting`,
//! `platform_cap_guard::require(PlatformApps)` and `enforce_app_scope`, all under
//! `auth_middleware`. `admin::router()` is `pub(crate)`, hence the copy;
//! `custom_app_functions_manual_run_guards` fails when production and the copy
//! diverge. The copy leaves out three outer layers of the protected tree
//! (`router/protected.rs`): `api_key_query_middleware`, `timeout_middleware` and
//! `app_publish_token_scope_middleware`. Global Owner standing comes from
//! `OXY_OWNER`, as in `admin_app_workspace_in_org`, so only the allow path runs.
//!
//! **Driver.** Run now enqueues a `TaskScope::Global` `app_function` task that
//! no request drives. The test drains it through `recover_pending_global_runs`
//! (the entry point `router::recovery::drive_pending` calls) and the production
//! `AppFunctionTaskExecutor`, but registered in the test's own
//! `CustomTaskRegistry`: production's `build_custom_task_registry` is private,
//! and the source scan guards its registration instead. The loop around it is
//! the test's too, so it does not cover the LISTEN/NOTIFY wake (it uses
//! `noop_router()`), the per-role `excluded_source_types`, the per-workspace
//! cloud tick (`tick_cloud`), a separate `oxy worker` fleet, or the scheduler.
//!
//! **Needs** Postgres only.

use std::sync::Arc;
use std::time::{Duration, Instant};

use agentic_pipeline::platform::PlatformContext;
use agentic_pipeline::recovery::recover_pending_global_runs;
use agentic_runtime::router::noop_router;
use agentic_runtime::state::RuntimeState;
use agentic_runtime::worker::CustomTaskRegistry;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware;
use axum::routing::{get, post};
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::config::OnMissing;
use oxy_app::agentic_wiring::OxyProjectContext;
use oxy_app::server::api::admin::apps::{functions as admin_functions, handlers};
use oxy_app::server::api::admin::assume::block_admin_while_acting;
use oxy_app::server::api::middlewares::oxy_owner_or_app_admin_guard::oxy_owner_or_app_admin_guard_middleware;
use oxy_app::server::api::middlewares::{app_scope_guard, platform_cap_guard};
use oxy_app::server::app_function_executor::{APP_FUNCTION_KIND, AppFunctionTaskExecutor};
use oxy_app::server::authz::Action;
use oxy_auth::middleware::{AuthState, auth_middleware};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tower::ServiceExt;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{FunctionSpec, invocations, publish_app, seeded_tenant};

const APP: &str = "fn-e2e-jobs";
/// One driver pass and an isolate start take well under a second; the rest is
/// slack for a cold CI box. Past it, the poll fails naming the run's last state.
const RUN_DEADLINE: Duration = Duration::from_secs(60);

const TALLY_STORE_JS: &str = r#"
export default async (req) => {
  const { store, counts } = JSON.parse(req.body);
  const tally = counts.reduce((sum, n) => sum + n, 0);
  console.log(`tallied store ${store}: ${tally}`);
  return Response.json({ store, tally });
};
"#;

fn functions() -> Vec<FunctionSpec> {
    // `route: false`: a job-only function, reachable by Run now and nothing else.
    vec![FunctionSpec {
        name: "tally-store",
        manifest: json!({ "route": false }),
        js: TALLY_STORE_JS,
    }]
}

/// The two paths as `admin::apps::router()` mounts them, below `/api/admin`.
/// `custom_app_functions_manual_run_guards` checks production still does.
pub(crate) const RUNS_ROUTE: &str = "/apps/{id}/functions/{name}/runs";
pub(crate) const RUN_DETAIL_ROUTE: &str = "/apps/{id}/function-runs/{run_id}";

fn admin_router() -> Router {
    let apps = Router::new()
        .route(RUNS_ROUTE, post(handlers::run_function_job))
        .route(RUN_DETAIL_ROUTE, get(admin_functions::get_function_run))
        .route_layer(middleware::from_fn(app_scope_guard::enforce_app_scope))
        .route_layer(middleware::from_fn(platform_cap_guard::require(
            Action::PlatformApps,
        )))
        .route_layer(middleware::from_fn(block_admin_while_acting));
    Router::new()
        .nest(
            "/api/admin",
            apps.layer(middleware::from_fn(oxy_owner_or_app_admin_guard_middleware)),
        )
        .layer(middleware::from_fn_with_state(
            AuthState::built_in(),
            auth_middleware,
        ))
}

async fn send(request: Request<Body>) -> (StatusCode, Value) {
    let response = admin_router().oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("read body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The driver's `PlatformContext`, over an empty workspace. The `app_function`
/// kind never reads it — its executor builds each invocation's context from the
/// app's own workspace — but the driver's signature requires one.
async fn platform() -> (Arc<dyn PlatformContext>, tempfile::TempDir) {
    let root = tempfile::tempdir().expect("platform dir");
    std::fs::write(
        root.path().join("config.yml"),
        "databases: []\nmodels: []\n",
    )
    .expect("write config.yml");
    let manager = WorkspaceBuilder::new(Uuid::new_v4())
        .with_working_copy(root.path(), None, OnMissing::Fail)
        .await
        .expect("config.yml loads")
        .build()
        .await
        .expect("workspace manager");
    (Arc::new(OxyProjectContext::new(manager)), root)
}

/// Polls the queue for the demo workspace's Global runs, as the latency worker does.
fn spawn_driver(db: DatabaseConnection, platform: Arc<dyn PlatformContext>) -> JoinHandle<()> {
    let mut registry = CustomTaskRegistry::new();
    registry.register(
        APP_FUNCTION_KIND,
        Arc::new(AppFunctionTaskExecutor {
            db: db.clone(),
            preagg: Default::default(),
        }),
    );
    let registry = Arc::new(registry);
    let state = Arc::new(RuntimeState::new());
    let router = noop_router();
    tokio::spawn(async move {
        loop {
            recover_pending_global_runs(
                db.clone(),
                state.clone(),
                platform.clone(),
                None,
                None,
                None,
                None,
                router.clone(),
                Some(demo_workspace_id()),
                Some(registry.clone()),
                &[],
            )
            .await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
}

/// The run's detail once it leaves `queued`/`running`; panics at the deadline.
async fn wait_for_run(app_id: Uuid, run_id: &str) -> Value {
    let uri = format!("/api/admin/apps/{app_id}/function-runs/{run_id}");
    let deadline = Instant::now() + RUN_DEADLINE;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        let (status, body) = send(Request::get(&uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::OK, "GET {uri}: {body}");
        if !matches!(body["status"].as_str(), Some("queued" | "running")) {
            return body;
        }
        last = body;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!(
        "function run {run_id} did not finish within {RUN_DEADLINE:?}; the run endpoint \
         last reported {last}"
    );
}

#[tokio::test]
async fn run_now_queues_the_function_and_the_driver_runs_it_to_done() {
    let t = seeded_tenant().await;
    let published = publish_app(&t, APP, demo_workspace_id(), &functions()).await;
    // SAFETY: process-per-test (asserted by `test_db`), before any admin request.
    // After publish, so the publish is authorized as the org Owner it is elsewhere.
    unsafe { std::env::set_var("OXY_OWNER", LOCAL_GUEST_EMAIL) };
    let (platform, _platform_dir) = platform().await;
    let driver = spawn_driver(t.db.clone(), platform);

    let (status, queued) = send(
        Request::post(format!(
            "/api/admin/apps/{}/functions/tally-store/runs",
            published.app_id
        ))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "store": "s-7", "counts": [10, 12, 20] }).to_string(),
        ))
        .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "run now: {queued}");
    let run_id = queued["run_id"].as_str().expect("run now answers {run_id}");

    let run = wait_for_run(published.app_id, run_id).await;
    driver.abort();

    assert_eq!(run["status"], "done", "run: {run}");
    assert_eq!(run["trigger"], "manual");
    let answer = run["answer"]
        .as_str()
        .expect("a done run carries the function's body");
    assert_eq!(
        serde_json::from_str::<Value>(answer).expect("the function answered JSON"),
        json!({ "store": "s-7", "tally": 42 })
    );
    let logs: Vec<&str> = run["logs"]
        .as_array()
        .expect("logs")
        .iter()
        .filter_map(|line| line["message"].as_str())
        .collect();
    assert_eq!(logs, vec!["tallied store s-7: 42"]);

    let rows = invocations(&t.db, published.app_id, "tally-store").await;
    let seen: Vec<_> = rows
        .iter()
        .map(|r| (r.mode.as_str(), r.status.as_str(), r.user_id))
        .collect();
    assert_eq!(seen, vec![("manual", "success", None)]);
}
