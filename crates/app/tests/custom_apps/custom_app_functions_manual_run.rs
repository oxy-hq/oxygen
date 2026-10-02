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
//! **Children.** `environment_checks` (a check run in a named app environment)
//! and `environment_refusals` (every refusal around one); `readback` (`/fn`'s
//! invocation id, and the fixture the read-backs share), `invocation_listings`,
//! `invocation_reach`, `sandbox_build_reach`, `held_readback`, `run_readback`
//! and `log_reads`;
//! `publish_token_router` (a real token through real authentication),
//! `run_stream_scope` (a custom app's own run stream and cancel) and
//! `partner_audit` (what a partner reads of the audit trail). They hang here
//! rather than in `main.rs`: they drive the same router and driver.
//!
//! **Needs** Postgres only.

mod callers;
mod environment_checks;
mod environment_refusals;
mod held_readback;
mod invocation_listings;
mod invocation_reach;
mod log_reads;
mod partner_audit;
mod publish_token_router;
mod readback;
mod run_readback;
mod run_stream_scope;
mod sandbox_build_reach;

pub(crate) use log_reads::{ERRORS_ROUTE, LOGS_ROUTE};

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
use oxy_app::server::api::admin::apps::{
    functions as admin_functions, handlers, held_writes, invocations as admin_invocations,
};
use oxy_app::server::api::admin::assume::block_admin_while_acting;
use oxy_app::server::api::middlewares::app_publish_token_scope::app_publish_token_scope_middleware;
use oxy_app::server::api::middlewares::oxy_owner_or_app_admin_guard::oxy_owner_or_app_admin_guard_middleware;
use oxy_app::server::api::middlewares::{app_scope_guard, platform_cap_guard};
use oxy_app::server::app_function_executor::{APP_FUNCTION_KIND, AppFunctionTaskExecutor};
use oxy_app::server::authz::Action;
use oxy_auth::middleware::{AuthState, auth_middleware};
use oxy_auth::types::AuthenticatedUser;
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

/// The paths as `admin::apps::router()` mounts them, below `/api/admin`.
/// `custom_app_functions_manual_run_guards` checks production still does.
pub(crate) const RUNS_ROUTE: &str = "/apps/{id}/functions/{name}/runs";
pub(crate) const RUN_DETAIL_ROUTE: &str = "/apps/{id}/function-runs/{run_id}";
pub(crate) const FUNCTIONS_ROUTE: &str = "/apps/{id}/functions";
pub(crate) const FUNCTION_INVOCATIONS_ROUTE: &str = "/apps/{id}/functions/{name}/invocations";
pub(crate) const INVOCATIONS_ROUTE: &str = "/apps/{id}/invocations";
pub(crate) const HELD_ROUTE: &str = "/apps/{id}/invocations/{invocation_id}/held";

fn admin_router() -> Router {
    guarded_admin().layer(middleware::from_fn_with_state(
        AuthState::built_in(),
        auth_middleware,
    ))
}

/// The two reads `router/global.rs` also mounts on the `/customer-apps/{id}/…`
/// surface — the one a publish token may `GET` — relative to that nest.
/// `custom_app_functions_manual_run_guards` checks production still does.
pub(crate) const TOKEN_INVOCATIONS_ROUTE: &str = "/{id}/functions/{name}/invocations";
pub(crate) const TOKEN_RUN_DETAIL_ROUTE: &str = "/{id}/function-runs/{run_id}";

/// That surface as a request reaches it: real authentication, the publish
/// token's own scope middleware, then the nest's guards in production order.
fn customer_apps_router() -> Router {
    let apps = Router::new()
        .route(
            TOKEN_INVOCATIONS_ROUTE,
            get(admin_functions::list_invocations),
        )
        .route(
            TOKEN_RUN_DETAIL_ROUTE,
            get(admin_functions::get_function_run),
        )
        .layer(middleware::from_fn(block_admin_while_acting))
        .layer(middleware::from_fn(app_scope_guard::enforce_app_scope))
        .layer(middleware::from_fn(platform_cap_guard::require(
            Action::PlatformApps,
        )))
        .layer(middleware::from_fn(oxy_owner_or_app_admin_guard_middleware));
    let api = Router::new()
        .nest("/customer-apps", apps)
        .layer(middleware::from_fn(app_publish_token_scope_middleware))
        .layer(middleware::from_fn_with_state(
            AuthState::built_in(),
            auth_middleware,
        ));
    Router::new().nest("/api", api)
}

/// `GET /api/customer-apps<path>`, as the guest — or, with `token`, as
/// whoever that publish token authenticates.
async fn get_customer_apps(path: &str, token: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::get(format!("/api/customer-apps{path}"));
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = request.body(Body::empty()).unwrap();
    answer(
        customer_apps_router()
            .oneshot(request)
            .await
            .expect("oneshot"),
    )
    .await
}

/// The admin stack below authentication: every guard, and no decision yet
/// about who is asking.
fn guarded_admin() -> Router {
    let apps = Router::new()
        .route(RUNS_ROUTE, post(handlers::run_function_job))
        .route(RUN_DETAIL_ROUTE, get(admin_functions::get_function_run))
        .route(FUNCTIONS_ROUTE, get(admin_functions::list_functions))
        .route(
            FUNCTION_INVOCATIONS_ROUTE,
            get(admin_functions::list_invocations),
        )
        .route(
            INVOCATIONS_ROUTE,
            get(admin_invocations::list_app_invocations),
        )
        .route(HELD_ROUTE, get(held_writes::get_held_writes))
        .route_layer(middleware::from_fn(app_scope_guard::enforce_app_scope))
        .route_layer(middleware::from_fn(platform_cap_guard::require(
            Action::PlatformApps,
        )))
        .route_layer(middleware::from_fn(block_admin_while_acting));
    Router::new().nest(
        "/api/admin",
        apps.layer(middleware::from_fn(oxy_owner_or_app_admin_guard_middleware)),
    )
}

/// `GET /api/admin<path>` through the same guards as `caller`, whom
/// authentication is taken to have resolved — for a caller the built-in guest
/// cannot be (a staff grant scoped to one org).
async fn get_admin_as(caller: &AuthenticatedUser, path: &str) -> (StatusCode, Value) {
    let request = Request::get(format!("/api/admin{path}"))
        .body(Body::empty())
        .unwrap();
    let router = guarded_admin().layer(axum::Extension(caller.clone()));
    answer(router.oneshot(request).await.expect("oneshot")).await
}

async fn send(request: Request<Body>) -> (StatusCode, Value) {
    answer(admin_router().oneshot(request).await.expect("oneshot")).await
}

/// A response's status and JSON body (`Null` for an empty or non-JSON one).
async fn answer(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("read body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// `GET /api/admin<path>` through the admin stack, as the guest.
pub(crate) async fn get_admin(path: &str) -> (StatusCode, Value) {
    send(
        Request::get(format!("/api/admin{path}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

/// `POST /api/admin<path>` with no body through the admin stack, as the guest.
pub(crate) async fn post_admin(path: &str) -> (StatusCode, Value) {
    send(
        Request::post(format!("/api/admin{path}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

/// The driver's `PlatformContext`, over an empty workspace. The `app_function`
/// kind never reads it — its executor builds each invocation's context from the
/// app's own workspace — but the driver's signature requires one.
pub(crate) async fn platform() -> (Arc<dyn PlatformContext>, tempfile::TempDir) {
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
pub(crate) fn spawn_driver(
    db: DatabaseConnection,
    platform: Arc<dyn PlatformContext>,
) -> JoinHandle<()> {
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
                Arc::new(agentic_pipeline::platform::IdentityResolver),
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
pub(crate) async fn wait_for_run(app_id: Uuid, run_id: &str) -> Value {
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
