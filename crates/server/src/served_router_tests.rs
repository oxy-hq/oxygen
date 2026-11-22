//! The router `serve` builds, from the seams this composition root hands it.
//!
//! `oxy-app` cannot depend on the surface crates (the dependency runs the other
//! way), so its own router tests build `api_router` with `SurfaceSeams::empty()`.
//! Once #3460 moved `/orgs` into `oxy-api-tenancy`, an assertion there that cloud
//! mode serves `/orgs` could only fail, and its local-mode mirror could only
//! pass. Both halves — `api_router` mounting the api seam in cloud mode and
//! dropping it in local mode, and this crate putting tenancy on that seam — are
//! visible only here, so the pair lives here.
//!
//! The routes under one org, and the GitHub namespace routes that hang off the
//! same prefix from `oxy-api-github`, came over from `oxy-app`'s
//! `local_mode_router` for the same reason, with one more: that file asked for
//! `/organizations…` and `/github/namespaces…`, which no crate registers in any
//! mode, so its 404s said nothing about either surface.
//!
//! # Why the org id in these paths is not a UUID
//!
//! "Mounted" is read off "not the router's 404", so every probe has to be a
//! request whose only possible 404 IS the router's. A well-formed org id is not
//! one: with no auth configured the built-in authenticator attaches the guest
//! user, the request reaches `org_middleware`, and that answers 404 for an org
//! that does not exist — the status of a route nobody mounted. An id the
//! middleware cannot parse is rejected (400) before anything is looked up, so
//! with [`ORG`] a mounted route is never 404 and an unmounted one always is.
//!
//! Each helper also asserts its control first — an unmounted path is 404 in
//! cloud mode, the local router serves `/live` — so if either stops holding the
//! control fails, instead of the assertion passing for nothing.
//!
//! `api_router` connects to `OXY_DATABASE_URL` while it builds, and runs the
//! unscoped `cleanup_stale_runs` over `agentic_runs` — so these skip without a
//! database, and `.config/nextest.toml` pins this module into `serial-db`.

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use oxy_app::server::router::api_router;
use oxy_app_core::serve_mode::ServeMode;
use tower::ServiceExt;

use crate::surface_seams;

/// An org id no route can parse. See the module docs.
const ORG: &str = "not-a-uuid";

/// A path under the same prefix that no crate registers.
const UNMOUNTED: &str = "/orgs-nothing-mounts-this";

fn db_unavailable() -> bool {
    std::env::var("OXY_DATABASE_URL").is_err()
}

/// `oxy-api-tenancy`'s org tree: the list, and the routes under one org.
fn organization_routes() -> Vec<(Method, String)> {
    vec![
        (Method::GET, "/orgs".to_string()),
        (Method::GET, format!("/orgs/{ORG}")),
        (Method::GET, format!("/orgs/{ORG}/members")),
        (Method::GET, format!("/orgs/{ORG}/invitations")),
    ]
}

/// `oxy-api-github`'s namespace routes, under the same org prefix from a
/// different crate.
fn github_namespace_routes() -> Vec<(Method, String)> {
    vec![
        (Method::GET, format!("/orgs/{ORG}/github/namespaces")),
        (Method::POST, format!("/orgs/{ORG}/github/namespaces/pat")),
    ]
}

/// `oxy-api-frontline`'s org-admin crew routes, whose handlers
/// (`frontline_admin`) moved out of `oxy-app` into that crate: the crew list,
/// a worker's apps and a PIN reset.
fn frontline_crew_routes() -> Vec<(Method, String)> {
    let worker = "00000000-0000-0000-0000-000000000000";
    vec![
        (Method::GET, format!("/orgs/{ORG}/frontline/workers")),
        (
            Method::PUT,
            format!("/orgs/{ORG}/frontline/workers/{worker}/apps"),
        ),
        (
            Method::POST,
            format!("/orgs/{ORG}/frontline/workers/{worker}/pin"),
        ),
    ]
}

/// `oxy-api-notifications`' routes, moved out of `oxy-app`'s
/// `build_global_routes` (cloud only, like the rest of the global tree): the
/// inbox, the badge, settings and a device.
fn notification_routes() -> Vec<(Method, String)> {
    let id = "00000000-0000-0000-0000-000000000000";
    vec![
        (Method::GET, "/notifications".to_string()),
        (Method::GET, "/notifications/unread".to_string()),
        (Method::GET, "/notifications/settings".to_string()),
        (Method::PATCH, format!("/notifications/devices/{id}")),
        // Not `/notifications/{id}/read`: that handler answers 404 for an id
        // that is not the caller's, so a 404 there proves nothing about mounting.
        (Method::POST, "/notifications/read-all".to_string()),
    ]
}

/// The router `serve` builds in `mode`, from the seams boot hands it.
async fn served(mode: ServeMode) -> Router {
    let (router, _external_router, _preagg) = api_router(
        mode,
        false,
        None,
        std::path::PathBuf::new(),
        tokio_util::sync::CancellationToken::new(),
        false,
        surface_seams(),
    )
    .await
    .expect("router built");
    router
}

async fn status(router: &Router, method: &Method, path: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    router.clone().oneshot(req).await.expect("oneshot").status()
}

/// Cloud mode serves every one of `routes`: each gets some answer other than
/// the 404 a path nobody mounts gets from the same router.
async fn assert_cloud_mounts(routes: Vec<(Method, String)>) {
    let router = served(ServeMode::Cloud).await;
    assert_eq!(
        status(&router, &Method::GET, UNMOUNTED).await,
        StatusCode::NOT_FOUND,
        "control: an unmounted path must be 404, or a non-404 below proves nothing"
    );
    for (method, path) in routes {
        let got = status(&router, &method, &path).await;
        assert_ne!(
            got,
            StatusCode::NOT_FOUND,
            "cloud mode must keep {method} {path} mounted, got {got}"
        );
    }
}

/// Local mode serves none of `routes`: it never mounts the org tree, so it must
/// drop the api seam they ride — while still serving its own routes.
async fn assert_local_drops(routes: Vec<(Method, String)>) {
    let router = served(ServeMode::Local).await;
    assert_eq!(
        status(&router, &Method::GET, "/live").await,
        StatusCode::OK,
        "control: the local router must serve something, or a 404 below proves nothing"
    );
    for (method, path) in routes {
        let got = status(&router, &method, &path).await;
        assert_eq!(
            got,
            StatusCode::NOT_FOUND,
            "local mode must not serve {method} {path}, got {got}"
        );
    }
}

#[tokio::test]
async fn cloud_mode_mounts_the_organizations_surface() {
    if db_unavailable() {
        return;
    }
    assert_cloud_mounts(organization_routes()).await;
}

#[tokio::test]
async fn local_mode_drops_the_organizations_surface() {
    if db_unavailable() {
        return;
    }
    assert_local_drops(organization_routes()).await;
}

#[tokio::test]
async fn cloud_mode_mounts_the_github_namespace_surface() {
    if db_unavailable() {
        return;
    }
    assert_cloud_mounts(github_namespace_routes()).await;
}

#[tokio::test]
async fn local_mode_drops_the_github_namespace_surface() {
    if db_unavailable() {
        return;
    }
    assert_local_drops(github_namespace_routes()).await;
}

#[tokio::test]
async fn cloud_mode_mounts_the_frontline_crew_routes() {
    if db_unavailable() {
        return;
    }
    assert_cloud_mounts(frontline_crew_routes()).await;
}

#[tokio::test]
async fn local_mode_drops_the_frontline_crew_routes() {
    if db_unavailable() {
        return;
    }
    assert_local_drops(frontline_crew_routes()).await;
}

/// `oxy-api-custom-apps`'s public seam. Unlike the routes above, "mounted" cannot
/// be read off "not 404" here: the Host-resolved health read answers 404 itself
/// when the `Host` names no custom app. What tells the handler's 404 from the
/// router's is the body — the router's is empty, the handler explains itself.
async fn custom_app_health_answers_from_its_handler(mode: ServeMode) {
    let router = served(mode).await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/customer-apps/health")
        .header(axum::http::header::HOST, "app.oxygen-hq.com")
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(req).await.expect("oneshot");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    assert!(
        String::from_utf8_lossy(&body).contains("not a custom-app subdomain"),
        "`/customer-apps/health` must reach `oxy-api-custom-apps`'s handler in {mode:?}, got {:?}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn cloud_mode_mounts_the_custom_app_health_read() {
    if db_unavailable() {
        return;
    }
    custom_app_health_answers_from_its_handler(ServeMode::Cloud).await;
}

/// The public seam is merged in BOTH modes, unlike the api seam above.
#[tokio::test]
async fn local_mode_mounts_the_custom_app_health_read_too() {
    if db_unavailable() {
        return;
    }
    custom_app_health_answers_from_its_handler(ServeMode::Local).await;
}

#[tokio::test]
async fn cloud_mode_mounts_the_notification_routes() {
    if db_unavailable() {
        return;
    }
    assert_cloud_mounts(notification_routes()).await;
}

#[tokio::test]
async fn local_mode_drops_the_notification_routes() {
    if db_unavailable() {
        return;
    }
    assert_local_drops(notification_routes()).await;
}

/// The composition hands `serve` and `worker` everything the notifications
/// surface needs to deliver, so neither refuses to boot: both queue kinds and
/// the boot hooks.
#[test]
fn the_composition_carries_the_notifications_surface() {
    let seams = surface_seams();
    assert_eq!(seams.require_notifications(), Ok(()));
    assert_eq!(
        oxy_app::server::router::require_task_kinds(&seams.tasks),
        Ok(())
    );
    let kinds: Vec<&str> = seams.tasks.iter().map(|t| t.kind).collect();
    for kind in oxy_app::server::router::NOTIFICATION_TASK_KINDS {
        assert!(kinds.contains(&kind), "no executor for `{kind}`: {kinds:?}");
    }
}

/// `serve` and `worker` both drain the health eval the schedule tick queues.
#[test]
fn the_composition_carries_the_health_eval() {
    let kinds: Vec<&str> = surface_seams().tasks.iter().map(|t| t.kind).collect();
    for kind in oxy_app::server::router::HEALTH_TASK_KINDS {
        assert!(kinds.contains(&kind), "no executor for `{kind}`: {kinds:?}");
    }
}

/// `serve` and `worker` both drain the insights delivery a monitor scan
/// queues. Without it every `notify:` block would enqueue and nothing would
/// ever deliver, so `require_task_kinds` holds it too (`ANOMALY_TASK_KINDS`).
#[test]
fn the_composition_carries_the_insights_delivery() {
    let seams = surface_seams();
    let kinds: Vec<&str> = seams.tasks.iter().map(|t| t.kind).collect();
    for kind in oxy_app::server::router::ANOMALY_TASK_KINDS {
        assert!(kinds.contains(&kind), "no executor for `{kind}`: {kinds:?}");
    }
    assert_eq!(
        oxy_app::server::router::require_task_kinds(&seams.tasks),
        Ok(())
    );
}

/// One request with an optional API key and JSON body, for the world-model
/// routes, whose handlers reject an empty body before they answer.
async fn status_with(
    router: &Router,
    method: &Method,
    path: &str,
    api_key: Option<&str>,
    json: &str,
) -> StatusCode {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(key) = api_key {
        req = req.header("x-api-key", key);
    }
    router
        .clone()
        .oneshot(req.body(Body::from(json.to_string())).unwrap())
        .await
        .expect("oneshot")
        .status()
}

/// `oxy-api-world-model`'s routes, as `(method, path under /{workspace_id}, body)`.
/// "Served" is "not the router's 404", as above: foot traffic answers 503 with
/// no BestTime key configured, which is its handler speaking. (The feed's SSE is
/// a stream, so it is left out; the external tree also checks the LLM proxy.)
fn world_model_routes() -> Vec<(Method, &'static str, &'static str)> {
    vec![
        (
            Method::POST,
            "/world-model/competitors",
            r#"{"anchors":[]}"#,
        ),
        (Method::POST, "/world-model/foot-traffic/current", "[]"),
        (Method::POST, "/world-model/foot-traffic/radar", "[]"),
    ]
}

/// The camera list and the weather, `oxy-api-world-model`'s since 2026-11-15
/// (out of `oxy-app`'s `api/video.rs`), on both trees. Probed with TRACE, which
/// none of them takes: a mounted path answers 405 from its method router
/// without calling UniFi or OpenWeatherMap (whose handlers may answer their own
/// 404 or 503 with no key configured), an unmounted one the router's 404.
fn world_model_video_routes() -> Vec<&'static str> {
    vec![
        "/world-model/cameras",
        "/world-model/weather/precipitation_new/3/4/5",
        "/world-model/weather/current",
    ]
}

/// The IDE's World Model Graph and peer cohorts, `oxy-api-world-model`'s since
/// they left `oxy-app`, on the workspace tree only. Each probe is a request its
/// extractors reject (a missing query or body field: 400 / 422) or the model
/// load refuses (no compiled model: 503), so none can be its handler's own 404.
fn world_model_graph_routes() -> Vec<(Method, &'static str, &'static str)> {
    vec![
        (Method::GET, "/semantic/world-model", ""),
        (Method::GET, "/semantic/world-model/instances", ""),
        (Method::GET, "/semantic/world-model/filter-instances", ""),
        (Method::POST, "/semantic/world-model/filter-counts", "{}"),
        (Method::GET, "/semantic/world-model/instance-detail", ""),
        (Method::GET, "/semantic/world-model/measure-breakdown", ""),
        (Method::POST, "/semantic/cohort", "{}"),
    ]
}

/// The workspace seam carries the world-model app into the `/{workspace_id}`
/// tree. Local mode, because there the guest user and the nil workspace get a
/// request past auth and workspace context to the router, so a 404 can only be
/// the router's own.
#[tokio::test]
async fn the_workspace_tree_mounts_the_world_model_app() {
    if db_unavailable() {
        return;
    }
    let router = served(ServeMode::Local).await;
    let ws = oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
    assert_eq!(
        status_with(
            &router,
            &Method::POST,
            &format!("/{ws}/world-model/nothing-mounts-this"),
            None,
            "{}"
        )
        .await,
        StatusCode::NOT_FOUND,
        "control: an unmounted path must be 404, or a non-404 below proves nothing"
    );
    for (method, path, body) in world_model_routes()
        .into_iter()
        .chain(world_model_graph_routes())
    {
        let got = status_with(&router, &method, &format!("/{ws}{path}"), None, body).await;
        assert_ne!(
            got,
            StatusCode::NOT_FOUND,
            "{method} /{{ws}}{path} must be served, got {got}"
        );
    }
    for path in world_model_video_routes() {
        let got = status_with(&router, &Method::TRACE, &format!("/{ws}{path}"), None, "").await;
        assert_eq!(
            got,
            StatusCode::METHOD_NOT_ALLOWED,
            "/{{ws}}{path} must be mounted, got {got}"
        );
    }
}

/// `oxy-api-metric-monitoring`'s routes under `/{workspace_id}`: the metric
/// tree, `/semantic/monitors` and the anomaly inbox. Probed with TRACE, which no
/// route takes: a mounted path answers 405 from its method router without
/// running a handler (a scan would otherwise run, and a handler can answer its
/// own 404 for a missing anomaly), an unmounted one the router's 404.
fn metric_monitoring_routes() -> Vec<&'static str> {
    vec![
        "/semantic/metric-tree",
        "/semantic/metric-tree/revenue/sensitivity",
        "/semantic/metric-tree/predict",
        "/semantic/metric-tree/explain",
        "/semantic/metric-tree/opportunity",
        "/semantic/metric-tree/drill",
        "/semantic/metric-tree/time-dimensions",
        "/semantic/metric-tree/distribution",
        "/semantic/metric-tree/baseline",
        "/semantic/metric-tree/projection",
        "/semantic/monitors",
    ]
}

/// The anomaly inbox, on both the workspace and the external tree.
fn anomaly_inbox_routes() -> Vec<&'static str> {
    vec![
        "/semantic/anomalies",
        "/semantic/anomalies/scan",
        "/semantic/anomalies/preview",
        "/semantic/anomalies/status",
        "/semantic/anomalies/3f2504e0-4f89-11d3-9a0c-0305e82c3301/status",
        "/semantic/anomalies/3f2504e0-4f89-11d3-9a0c-0305e82c3301/explain",
    ]
}

/// One GET, answered as `(status, body)`, with an optional API key.
async fn get_with_body(router: &Router, path: &str, api_key: Option<&str>) -> (StatusCode, String) {
    let mut req = Request::builder().method(Method::GET).uri(path);
    if let Some(key) = api_key {
        req = req.header("x-api-key", key);
    }
    let response = router
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .expect("oneshot");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// What axum answers when a handler's `Extension<T>` is absent: a 500 with this
/// body. The anomaly handlers extract `Extension<Arc<AgenticState>>`, which the
/// composition (`oxy-app`'s `with_agentic_state`, over the workspace and
/// external seams) must supply; the surface cannot layer it itself.
const MISSING_EXTENSION: &str = "Missing request extension";

/// The anomaly inbox, served through the composition `serve` uses, still gets
/// the `AgenticState` extension its handlers extract. Before the lift the
/// inbox's own builder layered it; now the seams do, and dropping that layer
/// compiles fine and fails every inbox request with a 500 at run time. A real
/// GET of the list runs the handler (it reads `metric_anomalies` through the
/// state's database), so anything but the extension rejection proves the layer.
#[tokio::test]
async fn the_workspace_tree_hands_the_anomaly_inbox_its_agentic_state() {
    if db_unavailable() {
        return;
    }
    // A real (empty) workspace for local mode to serve, so the workspace
    // middleware and `WorkspaceManagerReadOnly` resolve and the handler's
    // `Extension` extractor is actually reached. Without one the middleware
    // answers 503 first, and this check would pass with the layer dropped.
    // Local mode finds its workspace by walking up from the process's current
    // directory (`resolve_local_workspace_path`), so the test moves there.
    // Safe because nextest runs each test in its own process (the repo never
    // runs `cargo test`), as the `set_var` role tests rely on too.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n")
        .expect("write config.yml");
    std::env::set_current_dir(dir.path()).expect("enter the workspace");
    let (router, _external, _preagg) = api_router(
        ServeMode::Local,
        false,
        None,
        dir.path().to_path_buf(),
        tokio_util::sync::CancellationToken::new(),
        false,
        surface_seams(),
    )
    .await
    .expect("router built");
    let ws = oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
    for path in [
        format!("/{ws}/semantic/anomalies?limit=1"),
        format!("/{ws}/semantic/monitors"),
    ] {
        let (status, body) = get_with_body(&router, &path, None).await;
        assert!(
            !body.contains(MISSING_EXTENSION),
            "GET {path} reached its handler without the AgenticState extension \
             ({status}: {body}) — the seam layer was dropped"
        );
        // The handler ran: the list and the monitor listing both answer 200
        // from an empty workspace. Pinned so a refusal upstream of the
        // extractors (the 503 above) can never make this check vacuous again.
        assert_eq!(
            status,
            StatusCode::OK,
            "GET {path} must reach its handler and answer from the empty workspace, \
             got {status}: {body}"
        );
    }
}

/// The workspace seam carries the metric tree and the anomaly inbox into the
/// `/{workspace_id}` tree. Local mode, for the reason
/// `the_workspace_tree_mounts_the_world_model_app` gives.
#[tokio::test]
async fn the_workspace_tree_mounts_metric_monitoring() {
    if db_unavailable() {
        return;
    }
    let router = served(ServeMode::Local).await;
    let ws = oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
    assert_eq!(
        status_with(
            &router,
            &Method::TRACE,
            &format!("/{ws}/semantic/nothing-mounts-this"),
            None,
            ""
        )
        .await,
        StatusCode::NOT_FOUND,
        "control: an unmounted path must be 404, or a 405 below proves nothing"
    );
    for path in metric_monitoring_routes()
        .into_iter()
        .chain(anomaly_inbox_routes())
    {
        let got = status_with(&router, &Method::TRACE, &format!("/{ws}{path}"), None, "").await;
        assert_eq!(
            got,
            StatusCode::METHOD_NOT_ALLOWED,
            "/{{ws}}{path} must be mounted, got {got}"
        );
    }
}

/// `oxy-api-observability`'s three nests under `/{workspace_id}`, one concrete
/// route each and the parameterised ones, probed with TRACE as above.
fn observability_routes() -> Vec<&'static str> {
    vec![
        "/traces",
        "/traces/0af7651916cd43dd8448eb211c80319c",
        "/metrics/analytics",
        "/metrics/list",
        "/metrics/revenue",
        "/execution-analytics/summary",
        "/execution-analytics/time-series",
        "/execution-analytics/agents",
        "/execution-analytics/executions",
        "/execution-analytics/percentiles",
        "/execution-analytics/histogram",
        "/execution-analytics/cost",
    ]
}

/// The workspace seam carries the observability console into the
/// `/{workspace_id}` tree, and with it the six FleetOk declarations `oxy-app`'s
/// `nest_all` used to write. Local mode, for the reason
/// `the_workspace_tree_mounts_the_world_model_app` gives.
#[tokio::test]
async fn the_workspace_tree_mounts_the_observability_console() {
    // The declarations reach the seam (no database needed for this half).
    let decls = surface_seams().workspace.decls;
    for path in [
        "/traces",
        "/traces/{*rest}",
        "/metrics",
        "/metrics/{*rest}",
        "/execution-analytics",
        "/execution-analytics/{*rest}",
    ] {
        assert!(
            decls.iter().any(|d| d.method == "*"
                && d.path == path
                && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
            "the workspace seam must carry `* {path}` as FleetOk"
        );
    }
    if db_unavailable() {
        return;
    }
    let router = served(ServeMode::Local).await;
    let ws = oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
    // The control sits outside the three prefixes: `/traces/{trace_id}` and
    // `/metrics/{metric_name}` take any one segment, so a made-up path under
    // them would answer 405 too.
    assert_eq!(
        status_with(
            &router,
            &Method::TRACE,
            &format!("/{ws}/observability-nothing-mounts-this"),
            None,
            ""
        )
        .await,
        StatusCode::NOT_FOUND,
        "control: an unmounted path must be 404, or a 405 below proves nothing"
    );
    for path in observability_routes() {
        let got = status_with(&router, &Method::TRACE, &format!("/{ws}{path}"), None, "").await;
        assert_eq!(
            got,
            StatusCode::METHOD_NOT_ALLOWED,
            "/{{ws}}{path} must be mounted, got {got}"
        );
    }
}

/// A GET of the trace list, through the composition `serve` uses, reaches the
/// moved handler with the router state it reads (`AppState::observability`).
/// The handler answers 200 when a store is configured and its own
/// `Failed to query traces` 500 when not; anything else (the router's 404, the
/// workspace middleware's 503) means the request never got there.
#[tokio::test]
async fn the_trace_list_answers_through_the_composed_router() {
    if db_unavailable() {
        return;
    }
    // A real (empty) workspace, so the workspace middleware lets the request
    // through (see `the_workspace_tree_hands_the_anomaly_inbox_its_agentic_state`).
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n")
        .expect("write config.yml");
    std::env::set_current_dir(dir.path()).expect("enter the workspace");
    let (router, _external, _preagg) = api_router(
        ServeMode::Local,
        false,
        None,
        dir.path().to_path_buf(),
        tokio_util::sync::CancellationToken::new(),
        false,
        surface_seams(),
    )
    .await
    .expect("router built");
    let ws = oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
    let (status, body) = get_with_body(&router, &format!("/{ws}/traces?limit=1"), None).await;
    let handled = status == StatusCode::OK
        || (status == StatusCode::INTERNAL_SERVER_ERROR && body == "Failed to query traces");
    assert!(
        handled,
        "GET /{{ws}}/traces must reach the trace-list handler, got {status}: {body}"
    );
}

/// `oxy-api-automations`' routes under `/{workspace_id}`, probed with TRACE as
/// above (a mounted path answers 405 without running a handler).
fn automation_routes() -> Vec<&'static str> {
    vec![
        "/procedures",
        "/procedures/YXV0b21hdGlvbnMvbmlnaHRseS5hdXRvbWF0aW9uLnltbA",
        "/automations",
        "/automations/YXV0b21hdGlvbnMvbmlnaHRseS5hdXRvbWF0aW9uLnltbA",
        "/agentic-schedules",
        "/agentic-schedules/65c121b2-0000-0000-0000-000000000000",
        "/agentic-schedules/65c121b2-0000-0000-0000-000000000000/run-now",
        "/agentic-schedules/65c121b2-0000-0000-0000-000000000000/backfill",
    ]
}

/// The workspace seam carries the automation listing and the schedules into
/// the `/{workspace_id}` tree, with the eight FleetOk declarations `oxy-app`'s
/// `route_fleet` used to write; and the schedule list, served through the
/// composition `serve` uses, still gets the `AgenticState` extension its
/// handlers extract. Their own builder in `oxy-app` layered it before the lift;
/// now `with_agentic_state` over the seam does, and dropping that compiles fine
/// and fails every schedule request with a 500 at run time. A real GET of the
/// list runs the handler (it reads `agentic_schedules` through the state's
/// database), so a 200 proves both the mount and the layer.
#[tokio::test]
async fn the_workspace_tree_mounts_the_automations_surface() {
    let decls = surface_seams().workspace.decls;
    for path in [
        "/procedures",
        "/procedures/{path_b64}",
        "/automations",
        "/automations/{path_b64}",
        "/agentic-schedules",
        "/agentic-schedules/{id}",
        "/agentic-schedules/{id}/run-now",
        "/agentic-schedules/{id}/backfill",
    ] {
        assert!(
            decls.iter().any(|d| d.method == "*"
                && d.path == path
                && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
            "the workspace seam must carry `* {path}` as FleetOk"
        );
    }
    if db_unavailable() {
        return;
    }
    // A real (empty) workspace, so the workspace middleware lets the request
    // through (see `the_workspace_tree_hands_the_anomaly_inbox_its_agentic_state`).
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n")
        .expect("write config.yml");
    std::env::set_current_dir(dir.path()).expect("enter the workspace");
    let (router, _external, _preagg) = api_router(
        ServeMode::Local,
        false,
        None,
        dir.path().to_path_buf(),
        tokio_util::sync::CancellationToken::new(),
        false,
        surface_seams(),
    )
    .await
    .expect("router built");
    let ws = oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
    assert_eq!(
        status_with(
            &router,
            &Method::TRACE,
            &format!("/{ws}/automations-nothing-mounts-this"),
            None,
            ""
        )
        .await,
        StatusCode::NOT_FOUND,
        "control: an unmounted path must be 404, or a 405 below proves nothing"
    );
    for path in automation_routes() {
        let got = status_with(&router, &Method::TRACE, &format!("/{ws}{path}"), None, "").await;
        assert_eq!(
            got,
            StatusCode::METHOD_NOT_ALLOWED,
            "/{{ws}}{path} must be mounted, got {got}"
        );
    }
    let (status, body) = get_with_body(&router, &format!("/{ws}/agentic-schedules"), None).await;
    assert!(
        !body.contains(MISSING_EXTENSION),
        "GET /{{ws}}/agentic-schedules reached its handler without the AgenticState \
         extension ({status}: {body}) — the seam layer was dropped"
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /{{ws}}/agentic-schedules must reach its handler and answer from the \
         empty workspace, got {status}: {body}"
    );
    // The listing reads the empty workspace's automations: none, but answered.
    let (status, body) = get_with_body(&router, &format!("/{ws}/automations"), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /{{ws}}/automations must reach its handler, got {status}: {body}"
    );
}

/// `oxy-api-workspace-config`'s routes under `/{workspace_id}`, probed with
/// TRACE as above.
fn workspace_config_routes() -> Vec<&'static str> {
    vec![
        "/databases",
        "/databases/inspect",
        "/databases/inspect-schemas",
        "/databases/inspect-schema-tables",
        "/databases/warehouse/schema",
        "/databases/sync",
        "/secrets",
        "/secrets/bulk",
        "/secrets/7c9e6679-7425-40de-944b-e07fc1f90ae7",
        "/secrets/7c9e6679-7425-40de-944b-e07fc1f90ae7/value",
    ]
}

/// The workspace seam carries the workspace's databases and secrets into the
/// `/{workspace_id}` tree, with the nine FleetOk declarations `oxy-app`'s
/// `route_fleet` used to write (and `/databases/sync`, added 2026-11-21); and a real GET of the databases list, through
/// the composition `serve` uses, reaches its handler and answers from the
/// empty workspace's config, not the router's 404 or the workspace
/// middleware's 503.
#[tokio::test]
async fn the_workspace_tree_mounts_the_workspace_config_surface() {
    let decls = surface_seams().workspace.decls;
    for path in [
        "/databases",
        "/databases/inspect",
        "/databases/inspect-schemas",
        "/databases/inspect-schema-tables",
        "/databases/{database_name}/schema",
        "/databases/sync",
        "/secrets",
        "/secrets/bulk",
        "/secrets/{id}",
        "/secrets/{id}/value",
    ] {
        assert!(
            decls.iter().any(|d| d.method == "*"
                && d.path == path
                && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
            "the workspace seam must carry `* {path}` as FleetOk"
        );
    }
    if db_unavailable() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n")
        .expect("write config.yml");
    std::env::set_current_dir(dir.path()).expect("enter the workspace");
    let (router, _external, _preagg) = api_router(
        ServeMode::Local,
        false,
        None,
        dir.path().to_path_buf(),
        tokio_util::sync::CancellationToken::new(),
        false,
        surface_seams(),
    )
    .await
    .expect("router built");
    let ws = oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
    assert_eq!(
        status_with(
            &router,
            &Method::TRACE,
            &format!("/{ws}/workspace-config-nothing-mounts-this"),
            None,
            ""
        )
        .await,
        StatusCode::NOT_FOUND,
        "control: an unmounted path must be 404, or a 405 below proves nothing"
    );
    for path in workspace_config_routes() {
        let got = status_with(&router, &Method::TRACE, &format!("/{ws}{path}"), None, "").await;
        assert_eq!(
            got,
            StatusCode::METHOD_NOT_ALLOWED,
            "/{{ws}}{path} must be mounted, got {got}"
        );
    }
    let (status, body) = get_with_body(&router, &format!("/{ws}/databases"), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /{{ws}}/databases must reach its handler, got {status}: {body}"
    );
    assert_eq!(body, "[]", "the empty workspace configures no database");
}

/// The external seam carries the world-model app, and the LLM proxy, into the
/// curated `/external/api/{workspace_id}` tree, behind its API-key-only auth.
/// Auth runs before routing there, so the check needs a real key: a legacy key
/// for the local guest user, minted the way the key endpoint mints one.
#[tokio::test]
async fn the_external_tree_mounts_the_world_model_app() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    if db_unavailable() {
        return;
    }
    // A real (empty) workspace, so the anomaly-inbox check below reaches its
    // handler instead of the workspace middleware's 503 (see
    // `the_workspace_tree_hands_the_anomaly_inbox_its_agentic_state`).
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n")
        .expect("write config.yml");
    std::env::set_current_dir(dir.path()).expect("enter the workspace");
    let (router, external, _preagg) = api_router(
        ServeMode::Local,
        false,
        None,
        std::path::PathBuf::new(),
        tokio_util::sync::CancellationToken::new(),
        false,
        surface_seams(),
    )
    .await
    .expect("router built");
    let ws = oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
    // The first guest request creates the guest user the token will belong to.
    status(&router, &Method::GET, "/live").await;
    let db = sea_orm::Database::connect(std::env::var("OXY_DATABASE_URL").unwrap())
        .await
        .expect("connect");
    let guest = entity::users::Entity::find()
        .filter(entity::users::Column::Email.eq(oxy_auth::user::LOCAL_GUEST_EMAIL))
        .one(&db)
        .await
        .expect("query the guest")
        .expect("the guest user exists after a guest request");
    let key = oxy_auth::token::personal::create(
        &db,
        oxy_auth::token::personal::NewToken {
            user_id: guest.id,
            name: "served-router".into(),
            all_access: true,
            platform: true,
            partner: true,
            grants: Vec::new(),
            expires_at: None,
            source: oxy_auth::token::credential::source::UI,
        },
    )
    .await
    .expect("mint a token")
    .secret;

    let unmounted = format!("/{ws}/world-model/nothing-mounts-this");
    assert_eq!(
        status_with(&external, &Method::POST, &unmounted, Some(&key), "{}").await,
        StatusCode::NOT_FOUND,
        "control: with the key, an unmounted path must reach the 404 fallback"
    );
    assert_eq!(
        status_with(&external, &Method::POST, &unmounted, None, "{}").await,
        StatusCode::UNAUTHORIZED,
        "control: without the key nothing gets past auth, mounted or not"
    );
    for (method, path, body) in world_model_routes() {
        let got = status_with(
            &external,
            &method,
            &format!("/{ws}{path}"),
            Some(&key),
            body,
        )
        .await;
        assert_ne!(
            got,
            StatusCode::NOT_FOUND,
            "external {method} /{{ws}}{path} must be served, got {got}"
        );
    }
    // The camera list and the weather, on this tree as on the workspace one.
    for path in world_model_video_routes() {
        let got = status_with(
            &external,
            &Method::TRACE,
            &format!("/{ws}{path}"),
            Some(&key),
            "",
        )
        .await;
        assert_eq!(
            got,
            StatusCode::METHOD_NOT_ALLOWED,
            "external /{{ws}}{path} must be mounted, got {got}"
        );
    }
    // ...and the external seam hands the inbox the AgenticState extension too:
    // a real GET runs the list handler, which extracts it.
    let (status, body) = get_with_body(
        &external,
        &format!("/{ws}/semantic/anomalies?limit=1"),
        Some(&key),
    )
    .await;
    assert!(
        !body.contains(MISSING_EXTENSION),
        "external GET /{{ws}}/semantic/anomalies reached its handler without the \
         AgenticState extension ({status}: {body}) — the seam layer was dropped"
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "external GET /{{ws}}/semantic/anomalies must reach its handler and answer \
         from the empty workspace, got {status}: {body}"
    );
    // The anomaly inbox, `oxy-api-metric-monitoring`'s on the external seam:
    // the SDK's `client.anomalies.*` reaches it only here. TRACE, as in
    // `the_workspace_tree_mounts_metric_monitoring`.
    for path in anomaly_inbox_routes() {
        let got = status_with(
            &external,
            &Method::TRACE,
            &format!("/{ws}{path}"),
            Some(&key),
            "",
        )
        .await;
        assert_eq!(
            got,
            StatusCode::METHOD_NOT_ALLOWED,
            "external /{{ws}}{path} must be mounted, got {got}"
        );
    }
    let got = status_with(
        &external,
        &Method::POST,
        &format!("/{ws}/world-model/llm/messages"),
        Some(&key),
        r#"{"messages":[]}"#,
    )
    .await;
    assert_ne!(
        got,
        StatusCode::NOT_FOUND,
        "external LLM proxy must be served, got {got}"
    );
}

/// `oxy-api-identity`'s api-seam routes (cloud only, like the global tree they
/// left): the caller's tokens, the create dialog's options, and an org's API
/// access under the org middleware ([`ORG`] makes it answer 400, never 404).
/// Not `/auth/token`: its handler answers a session 404 by design.
fn identity_token_routes() -> Vec<(Method, String)> {
    vec![
        (Method::GET, "/user/tokens".to_string()),
        (Method::GET, "/user/token-options".to_string()),
        (Method::POST, "/auth/cli/authorize".to_string()),
        (Method::GET, format!("/orgs/{ORG}/service-accounts")),
        (Method::GET, format!("/orgs/{ORG}/tokens")),
        (Method::GET, format!("/orgs/{ORG}/token-policy")),
    ]
}

#[tokio::test]
async fn cloud_mode_mounts_the_identity_token_routes() {
    if db_unavailable() {
        return;
    }
    assert_cloud_mounts(identity_token_routes()).await;
}

#[tokio::test]
async fn local_mode_drops_the_identity_token_routes() {
    if db_unavailable() {
        return;
    }
    assert_local_drops(identity_token_routes()).await;
}

/// `oxy-api-identity`'s public seam, served in BOTH modes, outside the auth
/// stack: each answers its extractor's refusal of an empty body, never 404.
#[tokio::test]
async fn both_modes_mount_the_identity_handshakes() {
    if db_unavailable() {
        return;
    }
    for mode in [ServeMode::Cloud, ServeMode::Local] {
        let router = served(mode).await;
        assert_eq!(
            status(&router, &Method::POST, "/auth/nothing-mounts-this").await,
            StatusCode::NOT_FOUND,
            "control ({mode:?}): an unmounted path must be 404"
        );
        for path in [
            "/auth/cli/exchange",
            "/auth/cli/device",
            "/auth/cli/device/poll",
            "/auth/browser-ticket/redeem",
            "/auth/oidc/exchange",
            "/auth/tokens/revoke-leaked",
        ] {
            let got = status(&router, &Method::POST, path).await;
            assert_ne!(
                got,
                StatusCode::NOT_FOUND,
                "{mode:?}: POST {path}, got {got}"
            );
        }
    }
}

/// Signing in, `oxy-api-identity`'s since 2026-11-15 (was `oxy-app`'s
/// `build_public_routes`): served in BOTH modes, outside the auth stack. Probed
/// with TRACE, which none of them takes, so a mounted path answers 405 and an
/// unmounted one 404 — `/auth/dev-login` 404s from its handler when no
/// allow-list resolves, so a handler's answer would prove nothing there.
#[tokio::test]
async fn both_modes_mount_the_sign_in_routes() {
    if db_unavailable() {
        return;
    }
    for mode in [ServeMode::Cloud, ServeMode::Local] {
        let router = served(mode).await;
        assert_eq!(
            status(&router, &Method::TRACE, "/auth/nothing-mounts-this").await,
            StatusCode::NOT_FOUND,
            "control ({mode:?}): an unmounted path must be 404"
        );
        for path in [
            "/auth/config",
            "/auth/session",
            "/auth/oauth/state",
            "/auth/google",
            "/auth/github",
            "/auth/okta",
            "/auth/magic-link/request",
            "/auth/magic-link/verify",
            "/auth/return-to/validate",
            "/auth/dev-login",
            "/auth/mobile/exchange",
            "/auth/mobile/refresh",
            "/user",
        ] {
            assert_eq!(
                status(&router, &Method::TRACE, path).await,
                StatusCode::METHOD_NOT_ALLOWED,
                "{mode:?}: {path} must stay mounted"
            );
        }
    }
}

/// `/logout` and the mobile app's sign-in approval ride the api seam, cloud
/// only, as `build_global_routes` had them.
fn identity_session_routes() -> Vec<(Method, String)> {
    vec![
        (Method::GET, "/logout".to_string()),
        (Method::POST, "/auth/mobile/authorize".to_string()),
    ]
}

#[tokio::test]
async fn cloud_mode_mounts_logout_and_the_mobile_approval() {
    if db_unavailable() {
        return;
    }
    assert_cloud_mounts(identity_session_routes()).await;
}

#[tokio::test]
async fn local_mode_drops_logout_and_the_mobile_approval() {
    if db_unavailable() {
        return;
    }
    assert_local_drops(identity_session_routes()).await;
}

/// The data plane's `/query` (`oxy-api-custom-apps`' `data_plane`, on the
/// public seam) refuses an anonymous caller with 401 before it reads the body,
/// with a well-formed body or a malformed one: the handler takes raw bytes so
/// the gate runs first, and no extractor error leaks the route's body shape to
/// a caller who is not signed in. Auth is forced on, so a missing cookie cannot
/// fall through to local mode's guest user. Was `oxy-app`'s
/// `platform::projects_query`, which could not see the route once it left.
#[tokio::test]
async fn the_data_plane_query_refuses_an_anonymous_caller_before_its_body() {
    if db_unavailable() {
        return;
    }
    oxy_auth::built_in::set_auth_configured(true);
    let router = served(ServeMode::Local).await;
    for body in [r#"{"sql":"SELECT 1"}"#, "{}"] {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/projects/00000000-0000-0000-0000-000000000000/query")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let got = router
            .clone()
            .oneshot(request)
            .await
            .expect("oneshot")
            .status();
        assert_eq!(got, StatusCode::UNAUTHORIZED, "body {body}");
    }
}

/// The public seam carries the world model's customer-app twins beside the
/// public tree, in both modes, outside the auth stack (a bundle passes the
/// customer-app gate inline). PUT, which none of them takes, answers 405 from a
/// mounted path and 404 from an unmounted one, before any gate runs.
#[tokio::test]
async fn both_modes_mount_the_world_model_customer_app_twins() {
    if db_unavailable() {
        return;
    }
    let project = "00000000-0000-0000-0000-000000000001";
    for mode in [ServeMode::Cloud, ServeMode::Local] {
        let router = served(mode).await;
        assert_eq!(
            status(
                &router,
                &Method::PUT,
                &format!("/projects/{project}/semantic/nothing-mounts-this")
            )
            .await,
            StatusCode::NOT_FOUND,
            "control ({mode:?}): an unmounted path must be 404"
        );
        for path in [
            "semantic/world-model",
            "semantic/world-model/instances",
            "semantic/world-model/measure-breakdown",
            "semantic/cohort",
        ] {
            let got = status(
                &router,
                &Method::PUT,
                &format!("/projects/{project}/{path}"),
            )
            .await;
            assert_eq!(
                got,
                StatusCode::METHOD_NOT_ALLOWED,
                "{mode:?}: /projects/{{id}}/{path} must be mounted, got {got}"
            );
        }
    }
}

/// The workspace seam carries the token inventory into the `/{workspace_id}`
/// tree. PUT, which the path does not take, answers 405 from a mounted path and
/// 404 from an unmounted one, whatever the handler would say. Local mode for the
/// reason `the_workspace_tree_mounts_the_world_model_app` gives.
///
/// `/api-keys` was mounted beside it until legacy API keys were removed on
/// 2026-11-16; it is now one of the unmounted paths this asserts 404 for, which
/// is the point rather than an omission.
#[tokio::test]
async fn the_workspace_tree_mounts_the_token_inventory() {
    if db_unavailable() {
        return;
    }
    let router = served(ServeMode::Local).await;
    let ws = oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
    assert_eq!(
        status(
            &router,
            &Method::PUT,
            &format!("/{ws}/api-nothing-mounts-this")
        )
        .await,
        StatusCode::NOT_FOUND,
        "control: an unmounted path must be 404"
    );
    let got = status(&router, &Method::PUT, &format!("/{ws}/api-tokens")).await;
    assert_eq!(
        got,
        StatusCode::METHOD_NOT_ALLOWED,
        "PUT /{{ws}}/api-tokens must reach a mounted route, got {got}"
    );

    // And the removed one is gone from the tree, not merely refused by its
    // handler: 404, the same as a path nobody ever mounted.
    let got = status(&router, &Method::PUT, &format!("/{ws}/api-keys")).await;
    assert_eq!(
        got,
        StatusCode::NOT_FOUND,
        "PUT /{{ws}}/api-keys must be unmounted, got {got}"
    );
}

/// The admin seam carries the two token sections into the staff console, behind
/// its guard: the guest is refused, which a path nobody mounts is not.
#[tokio::test]
async fn cloud_mode_mounts_the_token_staff_sections_behind_the_guard() {
    if db_unavailable() {
        return;
    }
    let router = served(ServeMode::Cloud).await;
    let control = status(&router, &Method::GET, "/admin/nothing-mounts-this").await;
    for path in ["/admin/sandbox-agent-tokens", "/admin/standing-tokens"] {
        let got = status(&router, &Method::GET, path).await;
        assert_ne!(
            got, control,
            "GET {path} answered like an unmounted path: {got}"
        );
        assert!(
            got == StatusCode::FORBIDDEN || got == StatusCode::UNAUTHORIZED,
            "GET {path} must refuse a non-staff caller, got {got}"
        );
    }
}

/// The composition runs the token sweep on the global driver's pass, and gives
/// the console both token sections.
#[test]
fn the_composition_carries_the_identity_surface() {
    let seams = surface_seams();
    let ticks: Vec<&str> = seams.global_ticks.iter().map(|t| t.name).collect();
    assert!(ticks.contains(&"token_sweep"), "{ticks:?}");
    let caps: Vec<_> = seams.admin.iter().map(|s| s.capability).collect();
    for cap in [
        oxy_app::surface::Action::PlatformOperate,
        oxy_app::surface::Action::PlatformGrants,
    ] {
        assert!(caps.contains(&cap), "no section behind {cap:?}");
    }
}

/// `oxy-api-operating-graph`'s api-seam routes (cloud only, like the global
/// tree they left): work items at the root, outside the org middleware, and
/// the registry under one org ([`ORG`] makes it answer 400, never 404).
fn operating_graph_routes() -> Vec<(Method, String)> {
    vec![
        (Method::GET, "/work".to_string()),
        (Method::GET, format!("/orgs/{ORG}/locations")),
        (Method::GET, format!("/orgs/{ORG}/roles")),
        (Method::GET, format!("/orgs/{ORG}/assignments")),
    ]
}

#[tokio::test]
async fn cloud_mode_mounts_the_operating_graph() {
    if db_unavailable() {
        return;
    }
    assert_cloud_mounts(operating_graph_routes()).await;
}

#[tokio::test]
async fn local_mode_drops_the_operating_graph() {
    if db_unavailable() {
        return;
    }
    assert_local_drops(operating_graph_routes()).await;
}

/// A POST with `body` and `headers`, answered by the composed router: status
/// and body text.
async fn post_with(
    router: &Router,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// The GitHub App's webhook receiver, `oxy-api-github`'s since 2026-11-15 (was
/// `oxy-app`'s `build_public_routes`), rides the public seam in BOTH modes,
/// outside the auth stack. An unsigned or wrongly signed delivery gets the
/// handler's own refusal — 401 `invalid signature` — never the router's 404.
///
/// The handler answers 404 itself when `GITHUB_WEBHOOK_SECRET` is unset (an
/// unconfigured deployment has no such route), which would read exactly like
/// an unmounted path, so the secret is set first.
#[tokio::test]
async fn both_modes_mount_the_github_webhook() {
    if db_unavailable() {
        return;
    }
    // SAFETY: nextest runs each test in its own process (this module is in
    // `serial-db`), and nothing in it has read the variable yet.
    unsafe { std::env::set_var("GITHUB_WEBHOOK_SECRET", "whsec_served_router_test") };
    for mode in [ServeMode::Cloud, ServeMode::Local] {
        let router = served(mode).await;
        assert_eq!(
            post_with(&router, "/webhooks/nothing-mounts-this", &[], "{}")
                .await
                .0,
            StatusCode::NOT_FOUND,
            "control ({mode:?}): an unmounted path must be 404"
        );
        for signature in [None, Some("sha256=00"), Some("not-a-signature")] {
            let mut headers = vec![("x-github-event", "push")];
            if let Some(signature) = signature {
                headers.push(("x-hub-signature-256", signature));
            }
            let (status, body) = post_with(&router, "/webhooks/github", &headers, "{}").await;
            assert_eq!(
                (status, body.as_str()),
                (StatusCode::UNAUTHORIZED, "invalid signature"),
                "{mode:?}: POST /webhooks/github signed {signature:?} must reach the handler"
            );
        }
    }
}

/// Toast's order webhook, `oxy-api-world-model`'s since 2026-11-15 (was
/// `oxy-app`'s `build_public_routes`), rides the public seam in BOTH modes,
/// outside the auth stack, and answers from its own handler.
///
/// The handler's own "no such workspace" is a 404 too, so a bare status would
/// prove nothing. A `project_id` that is not a UUID is refused by the handler's
/// `Query` extractor (400, naming the field) before anything is read, and an
/// unknown workspace gets the handler's 404 WITH its body, where the router's
/// 404 has none.
#[tokio::test]
async fn both_modes_mount_the_toast_webhook() {
    if db_unavailable() {
        return;
    }
    for mode in [ServeMode::Cloud, ServeMode::Local] {
        let router = served(mode).await;
        assert_eq!(
            post_with(&router, "/webhooks/toast/nothing-mounts-this", &[], "{}").await,
            (StatusCode::NOT_FOUND, String::new()),
            "control ({mode:?}): an unmounted path must be the router's bodiless 404"
        );
        for query in ["", "?project_id=not-a-uuid"] {
            let (status, body) = post_with(
                &router,
                &format!("/webhooks/toast/orders{query}"),
                &[],
                "{}",
            )
            .await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "{mode:?} {query:?}: {body}"
            );
            assert!(body.contains("project_id"), "{mode:?} {query:?}: {body}");
        }
        let (status, body) = post_with(
            &router,
            "/webhooks/toast/orders?project_id=00000000-0000-0000-0000-00000000f00d",
            &[("Toast-Webhook-Signature", "bm90LWEtc2lnbmF0dXJl")],
            "{}",
        )
        .await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::NOT_FOUND, "workspace not found"),
            "{mode:?}: an unknown workspace must get the handler's own answer"
        );
    }
}

/// The router `serve` builds in local mode over a throwaway workspace directory
/// (`config.yml` with nothing in it), from the seams boot hands it: the shape
/// `the_workspace_tree_mounts_the_workspace_config_surface` uses, so the
/// workspace middleware lets a request on the nil workspace reach its handler.
async fn served_over_an_empty_workspace() -> (Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n")
        .expect("write config.yml");
    std::env::set_current_dir(dir.path()).expect("enter the workspace");
    let (router, _external, _preagg) = api_router(
        ServeMode::Local,
        false,
        None,
        dir.path().to_path_buf(),
        tokio_util::sync::CancellationToken::new(),
        false,
        surface_seams(),
    )
    .await
    .expect("router built");
    (router, dir)
}

/// TRACE, which none of the workspace routes takes, answers 405 from a mounted
/// path and 404 from an unmounted one. Asserts the control first.
async fn assert_workspace_paths_mounted(router: &Router, paths: &[&str]) {
    let ws = oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
    assert_eq!(
        status_with(
            router,
            &Method::TRACE,
            &format!("/{ws}/wave-a-nothing-mounts-this"),
            None,
            ""
        )
        .await,
        StatusCode::NOT_FOUND,
        "control: an unmounted path must be 404, or a 405 below proves nothing"
    );
    for path in paths {
        let got = status_with(router, &Method::TRACE, &format!("/{ws}{path}"), None, "").await;
        assert_eq!(
            got,
            StatusCode::METHOD_NOT_ALLOWED,
            "/{{ws}}{path} must be mounted, got {got}"
        );
    }
}

/// `oxy-api-tenancy`'s three leaves from `oxy-app` (2026-11-16). The org
/// subdomain's status and the logo ride the workspace seam in both modes, with
/// the two FleetOk declarations `route_fleet` wrote; the partner-publish
/// consent switch rides the api seam under `/orgs/{org_id}`, cloud only, as
/// `build_org_routes` had it, declared FleetOk too.
#[tokio::test]
async fn the_tenancy_leaves_are_mounted_where_oxy_app_had_them() {
    let seams = surface_seams();
    for (decls, path) in [
        (&seams.workspace.decls, "/org-subdomain"),
        (&seams.workspace.decls, "/logo"),
        (&seams.api.decls, "/orgs/{org_id}/partner-publish-consent"),
    ] {
        assert!(
            decls.iter().any(|d| d.method == "*"
                && d.path == path
                && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
            "the seam must carry `* {path}` as FleetOk"
        );
    }
    if db_unavailable() {
        return;
    }
    let consent = || {
        vec![
            (Method::GET, format!("/orgs/{ORG}/partner-publish-consent")),
            (Method::PUT, format!("/orgs/{ORG}/partner-publish-consent")),
        ]
    };
    assert_cloud_mounts(consent()).await;
    assert_local_drops(consent()).await;
    let (router, _dir) = served_over_an_empty_workspace().await;
    assert_workspace_paths_mounted(&router, &["/org-subdomain", "/logo"]).await;
}

/// `oxy-api-workspace-config`'s Looker listing (2026-11-16, out of
/// `build_integration_routes`) is merged into `/{workspace_id}/integrations`
/// beside the OAuth connects (the same crate's since 2026-11-21, out of the
/// same builder, until then `oxy-app`'s nest under that prefix), so every
/// route under the prefix must answer. A real GET reaches the handler and
/// answers from the empty workspace's config: no Looker integration, `[]`.
/// The credentials check beside it (`/integrations/looker/{name}/test`,
/// 2026-11-21) rides the same seam.
#[tokio::test]
async fn the_workspace_tree_mounts_the_looker_listing_beside_the_oauth_connects() {
    let decls = surface_seams().workspace.decls;
    for path in [
        "/integrations/looker",
        "/integrations/looker/{name}/test",
        "/integrations/quickbooks/authorize",
        "/integrations/oauth/{provider}/authorize",
    ] {
        assert!(
            decls.iter().any(|d| d.method == "*"
                && d.path == path
                && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
            "the workspace seam must carry `* {path}` as FleetOk"
        );
    }
    if db_unavailable() {
        return;
    }
    let (router, _dir) = served_over_an_empty_workspace().await;
    assert_workspace_paths_mounted(
        &router,
        &[
            "/integrations/looker",
            "/integrations/looker/my_looker/test",
            // The OAuth connects, `oxy-app`'s half of the prefix until
            // 2026-11-21 (`build_integration_routes`), now the same crate's.
            "/integrations/quickbooks/authorize",
            "/integrations/oauth/google/authorize",
        ],
    )
    .await;
    let ws = oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
    let (status, body) = get_with_body(&router, &format!("/{ws}/integrations/looker"), None).await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::OK, "[]"),
        "GET /{{ws}}/integrations/looker must reach its handler"
    );
}

/// `oxy-api-previews`' staging branch compile (2026-11-16) rides the WORKSPACE
/// seam, inside the `/{workspace_id}` tree where `build_workspace_routes` had
/// it (not the previews seam beside it), with the two FleetOk declarations
/// `route_fleet` wrote. A real status read reaches its handler: nothing has
/// compiled that commit, so it is `pending`.
#[tokio::test]
async fn the_workspace_tree_mounts_the_staging_compile() {
    let decls = surface_seams().workspace.decls;
    for path in ["/compile/staging", "/compile/staging/status"] {
        assert!(
            decls.iter().any(|d| d.method == "*"
                && d.path == path
                && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
            "the workspace seam must carry `* {path}` as FleetOk"
        );
    }
    if db_unavailable() {
        return;
    }
    let (router, _dir) = served_over_an_empty_workspace().await;
    assert_workspace_paths_mounted(&router, &["/compile/staging", "/compile/staging/status"]).await;
    let ws = oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let (status, body) = get_with_body(
        &router,
        &format!("/{ws}/compile/staging/status?git_sha={sha}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "status read: {body}");
    let json: serde_json::Value = serde_json::from_str(&body).expect("a JSON body");
    assert_eq!(json["status"], "pending", "{body}");
}

/// GET `path` and parse the body as JSON.
async fn get_json(router: &Router, path: &str) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .expect("oneshot");
    let status = response.status();
    // The whole route table is ~300 KB of prose, over `get_with_body`'s 1 MB
    // cap once JSON-escaped; read up to 16 MB.
    let bytes = axum::body::to_bytes(response.into_body(), 16 << 20)
        .await
        .expect("body");
    let body = String::from_utf8_lossy(&bytes).into_owned();
    let json = serde_json::from_str(&body).unwrap_or_else(|e| {
        panic!("GET {path} answered {status} with a non-JSON body ({e}): {body}")
    });
    (status, json)
}

/// The health probes, `oxy-server`'s since 2026-11-16 (`core_routes`; were
/// `oxy-app`'s `build_public_routes`), ride the public seam in BOTH modes,
/// declared `*` FleetOk as `route_fleet` declared them, and answer with the
/// bodies they always had.
#[tokio::test]
async fn both_modes_mount_the_health_probes() {
    let decls = surface_seams().public.decls;
    for path in ["/health", "/ready", "/live", "/version"] {
        assert!(
            decls.iter().any(|d| d.method == "*"
                && d.path == path
                && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
            "the public seam must carry `* {path}` as FleetOk"
        );
    }
    if db_unavailable() {
        return;
    }
    for mode in [ServeMode::Cloud, ServeMode::Local] {
        let router = served(mode).await;
        assert_eq!(
            status(&router, &Method::GET, "/health-nothing-mounts-this").await,
            StatusCode::NOT_FOUND,
            "control ({mode:?}): an unmounted path must be 404"
        );
        assert_eq!(
            status(&router, &Method::GET, "/live").await,
            StatusCode::OK,
            "{mode:?} /live"
        );
        // The database is up here, so readiness and health are too.
        assert_eq!(
            status(&router, &Method::GET, "/ready").await,
            StatusCode::OK,
            "{mode:?} /ready"
        );
        let (code, health) = get_json(&router, "/health").await;
        assert_eq!(code, StatusCode::OK, "{mode:?} /health: {health}");
        assert_eq!(health["status"], "healthy", "{health}");
        assert_eq!(health["service"], "oxy", "{health}");
        assert_eq!(health["database"]["connected"], true, "{health}");
        assert!(health["timestamp"].is_u64(), "{health}");
        assert!(
            health.as_object().unwrap().contains_key("image_tag"),
            "{health}"
        );
        assert!(
            health["build_info"]["git_commit_short"].is_string(),
            "{health}"
        );
        let (code, version) = get_json(&router, "/version").await;
        assert_eq!(code, StatusCode::OK, "{mode:?} /version: {version}");
        assert_eq!(version["service"], "oxy", "{version}");
        assert!(version.get("version").is_none(), "no semver: {version}");
        assert!(
            version.as_object().unwrap().contains_key("image_tag"),
            "{version}"
        );
        assert_eq!(
            version["build_info"]["git_commit_short"], health["build_info"]["git_commit_short"],
            "both probes name the same build"
        );
        assert!(
            version["build_info"]["sentry_release"].is_string(),
            "{version}"
        );
    }
}

/// Route discovery, `oxy-server`'s since 2026-11-16 (`core_routes`; was
/// `oxy-app`'s `build_catalog_routes`), rides `SurfaceSeams::catalog` at the
/// protected root in BOTH modes, declared `*` FleetOk, and serves the generated
/// table. (No auth is configured here, so the caller is the guest user; the
/// next test forces auth on.)
#[tokio::test]
async fn both_modes_serve_the_route_catalog() {
    let decls = surface_seams().catalog.decls;
    assert_eq!(
        decls
            .iter()
            .map(|d| (d.method, d.path, d.role))
            .collect::<Vec<_>>(),
        vec![("*", "/_catalog", oxy_shared::fleet_role::RouteRole::FleetOk)],
        "the catalog seam must carry exactly `* /_catalog` as FleetOk"
    );
    if db_unavailable() {
        return;
    }
    for mode in [ServeMode::Cloud, ServeMode::Local] {
        let router = served(mode).await;
        let (code, catalog) = get_json(&router, "/_catalog").await;
        assert_eq!(code, StatusCode::OK, "{mode:?}");
        let routes = catalog["routes"].as_array().expect("routes");
        assert!(routes.len() > 400, "{mode:?}: {} routes", routes.len());
        for path in ["/api/_catalog", "/api/health"] {
            assert!(
                routes.iter().any(|r| r["path"] == path),
                "{mode:?}: the served catalog lists {path}"
            );
        }
        let first = &routes[0];
        for field in [
            "method",
            "path",
            "surface",
            "credential",
            "path_parameters",
            "description",
            "note",
            "handler",
            "role",
        ] {
            assert!(
                first.get(field).is_some(),
                "{mode:?}: a route lacks `{field}`: {first}"
            );
        }
        assert!(
            !catalog["surfaces"].as_array().expect("surfaces").is_empty(),
            "{mode:?}"
        );
        assert_eq!(
            catalog["build"],
            oxy_app::host::build_identity::build_label(),
            "{mode:?}"
        );
        let (code, filtered) = get_json(&router, "/_catalog?filter=threads").await;
        assert_eq!(code, StatusCode::OK, "{mode:?}");
        let narrowed = filtered["routes"].as_array().expect("routes").len();
        assert!(
            narrowed > 0 && narrowed < routes.len(),
            "{mode:?}: ?filter= narrowed to {narrowed}"
        );
    }
}

/// The catalog is a reconnaissance gift, so it sits BEHIND the auth stack, as
/// it did in `oxy-app`; the probes sit in front of it. Auth is forced on, so a
/// missing credential cannot fall through to the guest user: cloud mode
/// refuses the catalog. Local mode's protected tree runs the guest-only stack
/// (`apply_local_middleware`), which admits the guest whatever is configured,
/// and the catalog is merged inside it exactly as `build_catalog_routes` was,
/// so there it answers. The probes answer in both.
#[tokio::test]
async fn the_route_catalog_refuses_an_anonymous_caller_and_the_probes_do_not() {
    if db_unavailable() {
        return;
    }
    oxy_auth::built_in::set_auth_configured(true);
    for (mode, catalog) in [
        (ServeMode::Cloud, StatusCode::UNAUTHORIZED),
        (ServeMode::Local, StatusCode::OK),
    ] {
        let router = served(mode).await;
        assert_eq!(
            status(&router, &Method::GET, "/_catalog").await,
            catalog,
            "{mode:?}: the route table is behind that mode's auth stack"
        );
        assert_eq!(
            status(&router, &Method::GET, "/live").await,
            StatusCode::OK,
            "{mode:?}: the probes are outside the auth stack"
        );
    }
}

/// `oxy-api-threads`' workspace routes under `/{workspace_id}`, probed with
/// TRACE as above.
fn thread_workspace_routes() -> Vec<&'static str> {
    vec![
        "/threads",
        "/threads/bulk-delete",
        "/threads/65c121b2-0000-0000-0000-000000000000",
        "/threads/65c121b2-0000-0000-0000-000000000000/stop",
        "/agents",
        "/artifacts/65c121b2-0000-0000-0000-000000000000",
        "/charts/chart-1.json",
        "/logs",
        "/results/files/65c121b2-0000-0000-0000-000000000000.parquet",
    ]
}

/// The conversation surface (Wave B, 2026-11-16) rides the WORKSPACE seam,
/// inside the `/{workspace_id}` tree where `build_workspace_routes` had it,
/// with its nine FleetOk declarations (`route_fleet` wrote ten; the tenth,
/// `/builder-availability`, was deleted with the builder agent). Real reads, through
/// the composition `serve` uses, reach their handlers: the thread list (the
/// guest's threads in the empty workspace: none, but answered) and the agent
/// listing `oxy-app`'s local-mode router test used to probe.
#[tokio::test]
async fn the_workspace_tree_mounts_the_conversation_surface() {
    let decls = surface_seams().workspace.decls;
    for path in [
        "/threads",
        "/threads/bulk-delete",
        "/threads/{id}",
        "/threads/{id}/stop",
        "/agents",
        "/artifacts/{id}",
        "/charts/{file_path}",
        "/logs",
        "/results/files/{file_id}",
    ] {
        assert!(
            decls.iter().any(|d| d.method == "*"
                && d.path == path
                && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
            "the workspace seam must carry `* {path}` as FleetOk"
        );
    }
    if db_unavailable() {
        return;
    }
    let (router, _dir) = served_over_an_empty_workspace().await;
    assert_workspace_paths_mounted(&router, &thread_workspace_routes()).await;
    let ws = oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
    let (status, body) = get_json(&router, &format!("/{ws}/threads")).await;
    assert_eq!(status, StatusCode::OK, "GET /{{ws}}/threads: {body}");
    assert!(
        body["threads"].is_array(),
        "GET /{{ws}}/threads must answer the paginated list: {body}"
    );
    let (status, body) = get_with_body(&router, &format!("/{ws}/agents"), None).await;
    assert_eq!(status, StatusCode::OK, "GET /{{ws}}/agents: {body}");
}

/// `oxy-api-threads`' org chat (Wave B, 2026-11-16): the api seam, at the root
/// where `build_global_routes` had it, so cloud only.
fn chat_routes() -> Vec<(Method, String)> {
    let id = "65c121b2-0000-0000-0000-000000000000";
    vec![
        (Method::TRACE, "/chat/channels".to_string()),
        (Method::TRACE, format!("/chat/channels/{id}/join")),
        (Method::TRACE, format!("/chat/channels/{id}/messages")),
        (Method::TRACE, format!("/chat/channels/{id}/read")),
        (Method::TRACE, format!("/chat/channels/{id}/stream")),
    ]
}

/// Chat rides the api seam with the five FleetOk declarations `route_fleet`
/// wrote (the SSE stream included), is mounted in cloud mode and dropped in
/// local mode as the global tree was, and a real read reaches its handler:
/// the guest belongs to no channel, so the list is empty but answered.
#[tokio::test]
async fn cloud_mode_mounts_org_chat_and_local_mode_drops_it() {
    let decls = surface_seams().api.decls;
    for path in [
        "/chat/channels",
        "/chat/channels/{id}/join",
        "/chat/channels/{id}/messages",
        "/chat/channels/{id}/read",
        "/chat/channels/{id}/stream",
    ] {
        assert!(
            decls.iter().any(|d| d.method == "*"
                && d.path == path
                && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
            "the api seam must carry `* {path}` as FleetOk"
        );
    }
    if db_unavailable() {
        return;
    }
    assert_cloud_mounts(chat_routes()).await;
    let router = served(ServeMode::Cloud).await;
    for (method, path) in chat_routes() {
        assert_eq!(
            status(&router, &method, &path).await,
            StatusCode::METHOD_NOT_ALLOWED,
            "{path} must be mounted (TRACE answers 405 from a mounted path)"
        );
    }
    let (status, body) = get_json(&router, "/chat/channels").await;
    assert_eq!(status, StatusCode::OK, "GET /chat/channels: {body}");
    assert!(
        body.is_array(),
        "GET /chat/channels must answer a list: {body}"
    );
    assert_local_drops(chat_routes()).await;
}

/// The composition starts chat's delivery listener through
/// `SurfaceSeams::background`, first, and `serve`'s boot check holds it there.
#[test]
fn the_composition_starts_the_chat_delivery_listener() {
    let seams = surface_seams();
    let names: Vec<&str> = seams.background.iter().map(|b| b.name).collect();
    assert_eq!(
        names.first(),
        Some(&"chat_delivery_listener"),
        "chat's listener must start first, as `new_agentic_state` started it: {names:?}"
    );
    for name in oxy_app::server::router::CHAT_BACKGROUND_JOBS {
        assert!(names.contains(&name), "no job `{name}`: {names:?}");
    }
    assert_eq!(seams.require_custom_apps(), Ok(()));
}

/// The listener `api_router` starts through the seam really attaches. A NOTIFY
/// never wakes the posting process directly (`announce` deliberately skips the
/// local bus), so a wake on this process's subscriber can only have come back
/// through the `LISTEN` connection the `chat_delivery_listener` job opened. The
/// announce repeats until the listener has attached (it connects in the
/// background), within a deadline.
#[tokio::test]
async fn the_served_router_starts_the_chat_delivery_listener() {
    if db_unavailable() {
        return;
    }
    let _router = served(ServeMode::Cloud).await;
    let channel = uuid::Uuid::new_v4();
    let mut rx = oxy_api_threads::chat::delivery::subscribe(channel);
    let db = oxy::database::client::establish_connection()
        .await
        .expect("database");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        oxy_api_threads::chat::delivery::announce(&db, channel).await;
        match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(())) => break,
            Ok(Err(e)) => panic!("the chat bus closed: {e}"),
            Err(_) => assert!(
                std::time::Instant::now() < deadline,
                "no wake within 10s: the chat delivery listener never attached, so \
                 a message posted on another replica would reach nobody here"
            ),
        }
    }
}

/// `oxy-api-workspaces`' workspace routes under `/{workspace_id}`, probed with
/// TRACE as above.
fn workspaces_workspace_routes() -> Vec<&'static str> {
    vec![
        "/meta",
        "/members",
        "/members/65c121b2-0000-0000-0000-000000000000",
        "/oxy-access",
        "/app-integrations",
    ]
}

/// The workspaces surface (Wave C, 2026-11-16) rides the WORKSPACE seam,
/// inside the `/{workspace_id}` tree where `build_workspace_routes` had it,
/// in both modes, with the five FleetOk declarations `route_fleet` wrote. Real
/// reads, through the composition `serve` uses, reach their handlers: the
/// workspace's metadata, its members (the guest owner) and the app
/// integrations its `config.yml` declares (none).
#[tokio::test]
async fn the_workspace_tree_mounts_the_workspaces_surface() {
    let decls = surface_seams().workspace.decls;
    for path in [
        "/meta",
        "/members",
        "/members/{user_id}",
        "/oxy-access",
        "/app-integrations",
    ] {
        assert!(
            decls.iter().any(|d| d.method == "*"
                && d.path == path
                && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
            "the workspace seam must carry `* {path}` as FleetOk"
        );
    }
    if db_unavailable() {
        return;
    }
    let (router, _dir) = served_over_an_empty_workspace().await;
    assert_workspace_paths_mounted(&router, &workspaces_workspace_routes()).await;
    let ws = oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
    let (status, body) = get_json(&router, &format!("/{ws}/meta")).await;
    assert_eq!(status, StatusCode::OK, "GET /{{ws}}/meta: {body}");
    assert_eq!(
        body["id"].as_str(),
        Some(ws.to_string().as_str()),
        "GET /{{ws}}/meta must answer this workspace's metadata: {body}"
    );
    let (status, body) = get_json(&router, &format!("/{ws}/members")).await;
    assert_eq!(status, StatusCode::OK, "GET /{{ws}}/members: {body}");
    assert!(
        body.is_array(),
        "GET /{{ws}}/members must answer a list: {body}"
    );
    let (status, body) = get_json(&router, &format!("/{ws}/app-integrations")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /{{ws}}/app-integrations: {body}"
    );
    assert_eq!(
        body,
        serde_json::json!([]),
        "the empty workspace's config.yml declares no app integrations"
    );
}

/// `oxy-api-workspaces`' org routes (Wave C, 2026-11-16): the api seam, under
/// `/orgs/{org_id}` where `build_org_routes` had them, so cloud only. Probed
/// with the methods they take: `org_middleware` answers an id that is not one
/// before any method routing, so a mounted path answers that refusal and an
/// unmounted one 404s.
fn workspaces_org_routes() -> Vec<(Method, String)> {
    vec![
        (Method::GET, format!("/orgs/{ORG}/workspaces")),
        (
            Method::DELETE,
            format!("/orgs/{ORG}/workspaces/65c121b2-0000-0000-0000-000000000000"),
        ),
        (
            Method::PATCH,
            format!("/orgs/{ORG}/workspaces/65c121b2-0000-0000-0000-000000000000/rename"),
        ),
    ]
}

/// The org's workspace list, delete and rename ride the api seam with the
/// three FleetOk declarations `route_fleet` wrote, are mounted in cloud mode
/// and dropped in local mode as the global tree was.
#[tokio::test]
async fn cloud_mode_mounts_the_org_workspace_routes_and_local_mode_drops_them() {
    let decls = surface_seams().api.decls;
    for path in [
        "/orgs/{org_id}/workspaces",
        "/orgs/{org_id}/workspaces/{id}",
        "/orgs/{org_id}/workspaces/{id}/rename",
    ] {
        assert!(
            decls.iter().any(|d| d.method == "*"
                && d.path == path
                && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
            "the api seam must carry `* {path}` as FleetOk"
        );
    }
    if db_unavailable() {
        return;
    }
    assert_cloud_mounts(workspaces_org_routes()).await;
    assert_local_drops(workspaces_org_routes()).await;
}

/// A real read of the org's workspace list through the composed cloud router:
/// `org_middleware` resolves the guest's own org (seeded here, the guest its
/// owner), the subscription guard lets it through, and the handler answers the
/// one workspace the org holds.
#[tokio::test]
async fn the_served_cloud_router_lists_an_orgs_workspaces() {
    if db_unavailable() {
        return;
    }
    use sea_orm::{ActiveModelTrait, ActiveValue};
    let router = served(ServeMode::Cloud).await;
    let db = oxy::database::client::establish_connection()
        .await
        .expect("database");
    let (status, me) = get_json(&router, "/user").await;
    assert_eq!(status, StatusCode::OK, "GET /user: {me}");
    let user_id: uuid::Uuid = me["id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("GET /user carries no id: {me}"));
    let org_id = uuid::Uuid::new_v4();
    entity::organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set("Wave C".into()),
        slug: ActiveValue::Set(format!("wave-c-{}", org_id.simple())),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed the org");
    entity::org_members::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        org_id: ActiveValue::Set(org_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(entity::org_members::OrgRole::Owner),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed the guest's membership");
    let workspace_id = uuid::Uuid::new_v4();
    let now = chrono::Utc::now();
    entity::workspaces::ActiveModel {
        id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set("listed".into()),
        org_id: ActiveValue::Set(Some(org_id)),
        status: ActiveValue::Set(entity::workspaces::WorkspaceStatus::Ready),
        created_at: ActiveValue::Set(now.into()),
        updated_at: ActiveValue::Set(now.into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed the workspace");
    let (status, body) = get_json(&router, &format!("/orgs/{org_id}/workspaces")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /orgs/{{org}}/workspaces: {body}"
    );
    let ids: Vec<&str> = body
        .as_array()
        .unwrap_or_else(|| panic!("the list must be an array: {body}"))
        .iter()
        .filter_map(|w| w["id"].as_str())
        .collect();
    assert_eq!(ids, [workspace_id.to_string()], "{body}");
}

/// `oxy-api-semantic`'s workspace routes under `/{workspace_id}` (Wave D2,
/// 2026-11-16), probed with TRACE as above. Concrete paths for the two
/// parameterised reads and the SQL file execute.
fn semantic_workspace_routes() -> Vec<&'static str> {
    vec![
        "/sql/cXVlcmllcy9hLnNxbA==",
        "/sql/query",
        "/semantic/objects",
        "/context-graph/sources",
        "/revision",
        "/revision/files",
        "/revision/files/content",
        "/semantic/topic/c2FsZXMudG9waWMueW1s",
        "/semantic/view/b3JkZXJzLnZpZXcueW1s",
        "/semantic/preagg-status",
        "/semantic/preagg-rebuild",
        "/semantic/compile",
        "/semantic",
    ]
}

/// The semantic surface (Wave D2, 2026-11-16) rides the WORKSPACE seam, inside
/// the `/{workspace_id}` tree where `build_workspace_routes` had it, with the
/// thirteen `*` FleetOk declarations `route_fleet` wrote. Every route is
/// mounted, and a real read reaches its handler through the composition
/// `serve` uses: the empty workspace serves no views and no topics.
#[tokio::test]
async fn the_workspace_tree_mounts_the_semantic_surface() {
    let decls = surface_seams().workspace.decls;
    for path in [
        "/sql/{pathb64}",
        "/sql/query",
        "/semantic/objects",
        "/context-graph/sources",
        "/revision",
        "/revision/files",
        "/revision/files/content",
        "/semantic/topic/{file_path_b64}",
        "/semantic/view/{file_path_b64}",
        "/semantic/preagg-status",
        "/semantic/preagg-rebuild",
        "/semantic/compile",
        "/semantic",
    ] {
        assert_eq!(
            decls.iter().filter(|d| d.path == path).count(),
            1,
            "the workspace seam must carry `{path}` exactly once"
        );
        assert!(
            decls.iter().any(|d| d.method == "*"
                && d.path == path
                && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
            "the workspace seam must carry `* {path}` as FleetOk"
        );
    }
    if db_unavailable() {
        return;
    }
    let (router, _dir) = served_over_an_empty_workspace().await;
    assert_workspace_paths_mounted(&router, &semantic_workspace_routes()).await;
    let ws = oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
    let (status, body) = get_json(&router, &format!("/{ws}/semantic/objects")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /{{ws}}/semantic/objects: {body}"
    );
    assert_eq!(
        body,
        serde_json::json!({ "views": [], "topics": [] }),
        "the empty workspace serves no views and no topics"
    );
}

/// The semantic surface's three routes on the curated external tree (Wave D2):
/// `/sql/query`, `/semantic` and `/semantic/compile`, behind the API-key-only
/// auth, as `build_external_workspace_routes` mounted them. Auth runs before
/// routing there, so the probe carries a real token (minted as
/// `the_external_tree_mounts_the_world_model_app` mints one); TRACE, which none
/// of them takes, answers 405 from a mounted path and the external fallback's
/// 404 from an unmounted one.
#[tokio::test]
async fn the_external_tree_mounts_the_semantic_surface() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    if db_unavailable() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n")
        .expect("write config.yml");
    std::env::set_current_dir(dir.path()).expect("enter the workspace");
    let (router, external, _preagg) = api_router(
        ServeMode::Local,
        false,
        None,
        std::path::PathBuf::new(),
        tokio_util::sync::CancellationToken::new(),
        false,
        surface_seams(),
    )
    .await
    .expect("router built");
    let ws = oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID;
    // The first guest request creates the guest user the token will belong to.
    status(&router, &Method::GET, "/live").await;
    let db = sea_orm::Database::connect(std::env::var("OXY_DATABASE_URL").unwrap())
        .await
        .expect("connect");
    let guest = entity::users::Entity::find()
        .filter(entity::users::Column::Email.eq(oxy_auth::user::LOCAL_GUEST_EMAIL))
        .one(&db)
        .await
        .expect("query the guest")
        .expect("the guest user exists after a guest request");
    let key = oxy_auth::token::personal::create(
        &db,
        oxy_auth::token::personal::NewToken {
            user_id: guest.id,
            name: "served-router-semantic".into(),
            all_access: true,
            platform: true,
            partner: true,
            grants: Vec::new(),
            expires_at: None,
            source: oxy_auth::token::credential::source::UI,
        },
    )
    .await
    .expect("mint a token")
    .secret;

    let unmounted = format!("/{ws}/semantic/nothing-mounts-this");
    assert_eq!(
        status_with(&external, &Method::TRACE, &unmounted, Some(&key), "").await,
        StatusCode::NOT_FOUND,
        "control: with the key, an unmounted path must reach the 404 fallback"
    );
    for path in ["/sql/query", "/semantic", "/semantic/compile"] {
        let got = status_with(
            &external,
            &Method::TRACE,
            &format!("/{ws}{path}"),
            Some(&key),
            "",
        )
        .await;
        assert_eq!(
            got,
            StatusCode::METHOD_NOT_ALLOWED,
            "external /{{ws}}{path} must be mounted, got {got}"
        );
    }
    // The rest of the surface is the IDE's and never was on this tree.
    for path in ["/semantic/objects", "/revision", "/semantic/preagg-status"] {
        let got = status_with(
            &external,
            &Method::TRACE,
            &format!("/{ws}{path}"),
            Some(&key),
            "",
        )
        .await;
        assert_eq!(
            got,
            StatusCode::NOT_FOUND,
            "external /{{ws}}{path} must stay off the curated tree, got {got}"
        );
    }
}

/// The three staff sections the admin split's first PR took out of `oxy-app`'s
/// `admin::router`, each now on `SurfaceSeams::admin`: an org's subdomain
/// (`oxy-api-tenancy`, `PlatformOrgs`), Admin → Metrics' LLM cost
/// (`oxy-api-observability`, `PlatformOperate`) and the Explorer
/// (`oxy-api-threads`, `PlatformExplorer`). `{org}` is filled per test.
const ADMIN_SPLIT_ROUTES: &[(&str, &str)] = &[
    ("GET", "/admin/orgs/{org}/subdomain"),
    ("PUT", "/admin/orgs/{org}/subdomain"),
    ("GET", "/admin/metrics/llm-usage"),
    ("GET", "/admin/metrics/orgs/{org}/llm-usage"),
    ("GET", "/admin/explorer/threads"),
    ("GET", "/admin/explorer/runs"),
];

/// Sets `OXY_OWNER` for one test and puts back what was there. The guards read
/// it per request, and nextest runs each test in its own process, so it never
/// reaches another test.
struct OwnerEnv(Option<String>);

impl OwnerEnv {
    fn set(email: &str) -> Self {
        let prev = std::env::var("OXY_OWNER").ok();
        unsafe { std::env::set_var("OXY_OWNER", email) };
        Self(prev)
    }
}

impl Drop for OwnerEnv {
    fn drop(&mut self) {
        match &self.0 {
            Some(v) => unsafe { std::env::set_var("OXY_OWNER", v) },
            None => unsafe { std::env::remove_var("OXY_OWNER") },
        }
    }
}

/// The served router mounts the three sections inside `/api/admin`, behind the
/// same layers `admin::router` gave them in `oxy-app`, answering what `main`
/// answered before the move (read off the served router at 3e347a524):
///
/// * the guest, who is not staff: 403 on every route, TRACE included — the
///   staff door refuses before any section is consulted;
/// * the guest as the Global Owner: TRACE 405 (mounted), and a real read —
///   both LLM-usage reads and both Explorer searches 200, an org that does not
///   exist 404 on its subdomain;
/// * the same owner while acting as a tenant (a live assume session): 403
///   everywhere, from `block_admin_while_acting`;
///
/// with a path nobody mounts answering 404 throughout, as the control.
#[tokio::test]
async fn the_admin_seam_mounts_the_first_split_sections_behind_the_staff_layers() {
    if db_unavailable() {
        return;
    }
    use oxy_app::surface::roles::{RouteRole, classify};
    use sea_orm::{ActiveModelTrait, ActiveValue};

    let router = served(ServeMode::Cloud).await;
    let org = uuid::Uuid::new_v4().to_string();
    let routes: Vec<(Method, String)> = ADMIN_SPLIT_ROUTES
        .iter()
        .map(|(m, p)| {
            (
                Method::from_bytes(m.as_bytes()).unwrap(),
                p.replace("{org}", &org),
            )
        })
        .collect();
    let control = "/admin/nothing-mounts-this";

    // Pod placement: each declared FleetOk on the served table.
    for (method, path) in &routes {
        assert_eq!(
            classify(method.as_str(), &format!("/api{path}")),
            RouteRole::FleetOk,
            "{method} {path} is Postgres-only and must stay FleetOk"
        );
    }

    // Not staff: the door refuses, exactly as on `main`.
    assert_eq!(
        status(&router, &Method::GET, control).await,
        StatusCode::NOT_FOUND
    );
    for (method, path) in &routes {
        for m in [method, &Method::TRACE] {
            let got = status(&router, m, path).await;
            assert_eq!(
                got,
                StatusCode::FORBIDDEN,
                "non-staff {m} {path}: got {got}"
            );
        }
    }

    // Staff: mounted, and a real read answers.
    let _owner = OwnerEnv::set(oxy_auth::user::LOCAL_GUEST_EMAIL);
    assert_eq!(
        status(&router, &Method::GET, control).await,
        StatusCode::NOT_FOUND
    );
    for (_, path) in &routes {
        let got = status(&router, &Method::TRACE, path).await;
        assert_eq!(
            got,
            StatusCode::METHOD_NOT_ALLOWED,
            "{path} must be mounted for staff, got {got}"
        );
    }
    for path in [
        "/admin/metrics/llm-usage".to_string(),
        format!("/admin/metrics/orgs/{org}/llm-usage"),
        "/admin/explorer/threads".to_string(),
        "/admin/explorer/runs".to_string(),
    ] {
        let (got, body) = get_json(&router, &path).await;
        assert_eq!(got, StatusCode::OK, "staff GET {path}: {body}");
    }
    assert_eq!(
        status(
            &router,
            &Method::GET,
            &format!("/admin/orgs/{org}/subdomain")
        )
        .await,
        StatusCode::NOT_FOUND,
        "an org that does not exist has no subdomain to read"
    );

    // Acting as a tenant closes the console, these sections included.
    let db = oxy::database::client::establish_connection()
        .await
        .expect("database");
    let (got, me) = get_json(&router, "/user").await;
    assert_eq!(got, StatusCode::OK, "GET /user: {me}");
    let user_id: uuid::Uuid = me["id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("GET /user carries no id: {me}"));
    let acting_org = uuid::Uuid::new_v4();
    entity::organizations::ActiveModel {
        id: ActiveValue::Set(acting_org),
        name: ActiveValue::Set("Admin split".into()),
        slug: ActiveValue::Set(format!("admin-split-{}", acting_org.simple())),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed the org acted as");
    let now = chrono::Utc::now();
    let session = entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        actor_user_id: ActiveValue::Set(user_id),
        actor_email: ActiveValue::Set(oxy_auth::user::LOCAL_GUEST_EMAIL.into()),
        org_id: ActiveValue::Set(acting_org),
        reason: ActiveValue::Set("served-router admin split".into()),
        started_at: ActiveValue::Set(now.into()),
        // Short, so a panic below leaves the guest acting for two minutes at most.
        expires_at: ActiveValue::Set((now + chrono::Duration::minutes(2)).into()),
        ended_at: ActiveValue::Set(None),
        token_id: ActiveValue::Set(None),
    }
    .insert(&db)
    .await
    .expect("seed a live assume session");
    let mut refused = Vec::new();
    for (method, path) in &routes {
        refused.push((
            method.clone(),
            path.clone(),
            status(&router, method, path).await,
        ));
    }
    let control_while_acting = status(&router, &Method::GET, control).await;
    entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(session.id),
        ended_at: ActiveValue::Set(Some(chrono::Utc::now().into())),
        ..Default::default()
    }
    .update(&db)
    .await
    .expect("end the session");
    assert_eq!(control_while_acting, StatusCode::NOT_FOUND);
    for (method, path, got) in refused {
        assert_eq!(
            got,
            StatusCode::FORBIDDEN,
            "acting {method} {path}: got {got}"
        );
    }
}

/// The composition hands the admin seam each split section behind the
/// capability `admin::router` gave it in `oxy-app`.
#[test]
fn the_composition_carries_the_first_split_sections() {
    use oxy_app::surface::Action;
    let seams = surface_seams();
    let section_of = |path: &str| {
        seams
            .admin
            .iter()
            .find(|s| s.decls.iter().any(|d| d.path == path))
            .map(|s| s.capability)
    };
    assert_eq!(
        section_of("/explorer/threads"),
        Some(Action::PlatformExplorer)
    );
    assert_eq!(section_of("/explorer/runs"), Some(Action::PlatformExplorer));
    assert_eq!(
        section_of("/metrics/llm-usage"),
        Some(Action::PlatformOperate)
    );
    assert_eq!(
        section_of("/metrics/orgs/{org_id}/llm-usage"),
        Some(Action::PlatformOperate)
    );
    // Tenancy's section declares nothing of its own (the console's
    // `/{*rest}` FleetOk wildcard places it), so find it by capability.
    assert!(
        seams
            .admin
            .iter()
            .any(|s| s.capability == Action::PlatformOrgs),
        "no section behind PlatformOrgs"
    );
}

/// The two staff sections the admin split's second PR took out of `oxy-app`'s
/// `admin::router`, each now on `SurfaceSeams::admin`: the partner registry
/// (`oxy-api-tenancy`, its own section, `PlatformPartners`) and the CI publish
/// tokens (`oxy-api-custom-apps`, `PlatformApps`). As `(method, path, what the
/// Global Owner gets)`: a read answers 200, an empty body 422, an id that does
/// not exist 404, revoking a partnership nobody holds 204 (an audited no-op)
/// and a session mint 200 — read off the served router on `main` before the
/// move.
const ADMIN_SPLIT_PR2_ROUTES: &[(&str, &str, u16)] = &[
    ("GET", "/admin/partners", 200),
    ("POST", "/admin/partners/grant", 422),
    ("GET", "/admin/partners/{x}", 404),
    ("PUT", "/admin/partners/{x}/capabilities", 422),
    ("POST", "/admin/partners/{x}/orgs", 422),
    ("DELETE", "/admin/partners/{x}/orgs/{y}", 404),
    ("PUT", "/admin/partners/{x}/people/{y}", 404),
    ("DELETE", "/admin/partners/{x}/people/{y}", 404),
    ("DELETE", "/admin/partners/{x}", 204),
    ("GET", "/admin/app-publish-tokens", 200),
    ("POST", "/admin/app-publish-tokens", 200),
    ("POST", "/admin/app-publish-tokens/{x}/revoke", 404),
];

/// One request with a `{}` JSON body for a write, so a mounted write answers
/// its extractor's 422 rather than a missing content type's 415.
async fn admin_split_call(router: &Router, method: &Method, path: &str) -> StatusCode {
    let mut req = Request::builder().method(method).uri(path);
    let body = if matches!(*method, Method::POST | Method::PUT) {
        req = req.header("content-type", "application/json");
        Body::from("{}")
    } else {
        Body::empty()
    };
    router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .expect("oneshot")
        .status()
}

/// The served router mounts the second split's two sections inside
/// `/api/admin`, behind the same layers `admin::router` gave them in
/// `oxy-app`, answering what `main` answered before the move:
///
/// * the guest, who is not staff: 403 on every route, TRACE included;
/// * the guest as the Global Owner: TRACE 405 (mounted), and each route's
///   handler speaking ([`ADMIN_SPLIT_PR2_ROUTES`]);
/// * the same owner while acting as a tenant: 403 everywhere;
/// * the guest as a partner (an admin of an org holding a grant, no staff
///   standing): 403 everywhere — the registry is staff provisioning, not the
///   partner console, which the same caller does reach (the control);
///
/// with a path nobody mounts answering 404 throughout.
#[tokio::test]
async fn the_admin_seam_mounts_the_second_split_sections_behind_the_staff_layers() {
    if db_unavailable() {
        return;
    }
    use oxy_app::surface::roles::{RouteRole, classify};
    use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait};

    let router = served(ServeMode::Cloud).await;
    let (x, y) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    let routes: Vec<(Method, String, u16)> = ADMIN_SPLIT_PR2_ROUTES
        .iter()
        .map(|(m, p, owner)| {
            (
                Method::from_bytes(m.as_bytes()).unwrap(),
                p.replace("{x}", &x.to_string())
                    .replace("{y}", &y.to_string()),
                *owner,
            )
        })
        .collect();
    let control = "/admin/nothing-mounts-this";
    let refused_everywhere = |who: &'static str| {
        let router = router.clone();
        let routes = routes.clone();
        async move {
            assert_eq!(
                status(&router, &Method::GET, control).await,
                StatusCode::NOT_FOUND,
                "{who}: control"
            );
            for (method, path, _) in &routes {
                for m in [method, &Method::TRACE] {
                    let got = admin_split_call(&router, m, path).await;
                    assert_eq!(got, StatusCode::FORBIDDEN, "{who} {m} {path}: got {got}");
                }
            }
        }
    };

    for (method, path, _) in &routes {
        assert_eq!(
            classify(method.as_str(), &format!("/api{path}")),
            RouteRole::FleetOk,
            "{method} {path} is Postgres-only and must stay FleetOk"
        );
    }

    refused_everywhere("non-staff").await;

    let owner = OwnerEnv::set(oxy_auth::user::LOCAL_GUEST_EMAIL);
    assert_eq!(
        status(&router, &Method::GET, control).await,
        StatusCode::NOT_FOUND
    );
    for (method, path, expected) in &routes {
        let got = admin_split_call(&router, &Method::TRACE, path).await;
        assert_eq!(
            got,
            StatusCode::METHOD_NOT_ALLOWED,
            "{path} must be mounted for staff, got {got}"
        );
        let got = admin_split_call(&router, method, path).await;
        assert_eq!(got.as_u16(), *expected, "owner {method} {path}: got {got}");
    }

    let db = oxy::database::client::establish_connection()
        .await
        .expect("database");
    let (got, me) = get_json(&router, "/user").await;
    assert_eq!(got, StatusCode::OK, "GET /user: {me}");
    let user_id: uuid::Uuid = me["id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("GET /user carries no id: {me}"));
    let now = chrono::Utc::now();
    let org = |name: &'static str| {
        let id = uuid::Uuid::new_v4();
        entity::organizations::ActiveModel {
            id: ActiveValue::Set(id),
            name: ActiveValue::Set(name.into()),
            slug: ActiveValue::Set(format!("admin-split-2-{}", id.simple())),
            ..Default::default()
        }
    };

    // Acting as a tenant closes the console, these sections included.
    let acting = org("Admin split 2")
        .insert(&db)
        .await
        .expect("seed the org");
    let session = entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        actor_user_id: ActiveValue::Set(user_id),
        actor_email: ActiveValue::Set(oxy_auth::user::LOCAL_GUEST_EMAIL.into()),
        org_id: ActiveValue::Set(acting.id),
        reason: ActiveValue::Set("served-router admin split 2".into()),
        started_at: ActiveValue::Set(now.into()),
        // Short, so a panic below leaves the guest acting for two minutes at most.
        expires_at: ActiveValue::Set((now + chrono::Duration::minutes(2)).into()),
        ended_at: ActiveValue::Set(None),
        token_id: ActiveValue::Set(None),
    }
    .insert(&db)
    .await
    .expect("seed a live assume session");
    let mut acting_answers = Vec::new();
    for (method, path, _) in &routes {
        for m in [method, &Method::TRACE] {
            acting_answers.push((
                m.clone(),
                path.clone(),
                admin_split_call(&router, m, path).await,
            ));
        }
    }
    let acting_control = status(&router, &Method::GET, control).await;
    entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(session.id),
        ended_at: ActiveValue::Set(Some(chrono::Utc::now().into())),
        ..Default::default()
    }
    .update(&db)
    .await
    .expect("end the session");
    assert_eq!(acting_control, StatusCode::NOT_FOUND);
    for (method, path, got) in acting_answers {
        assert_eq!(
            got,
            StatusCode::FORBIDDEN,
            "acting {method} {path}: got {got}"
        );
    }

    // A partner, not staff: the registry refuses, the partner console answers.
    drop(owner);
    let partner = org("Admin split 2 partner")
        .insert(&db)
        .await
        .expect("seed the partner org");
    entity::partner_grants::ActiveModel {
        org_id: ActiveValue::Set(partner.id),
        status: ActiveValue::Set("active".into()),
        created_by: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now.into()),
    }
    .insert(&db)
    .await
    .expect("seed the grant");
    let member = entity::org_members::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        org_id: ActiveValue::Set(partner.id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(entity::org_members::OrgRole::Admin),
        created_at: ActiveValue::Set(now.into()),
        updated_at: ActiveValue::Set(now.into()),
    }
    .insert(&db)
    .await
    .expect("seed the membership");
    entity::partner_role_bindings::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        org_member_id: ActiveValue::Set(member.id),
        created_at: ActiveValue::Set(now.into()),
    }
    .insert(&db)
    .await
    .expect("seed partner access");
    let (got, mine) = get_json(&router, "/partners").await;
    assert_eq!(got, StatusCode::OK, "GET /partners: {mine}");
    assert!(
        mine.as_array().is_some_and(|all| all
            .iter()
            .any(|p| p["partner_id"] == partner.id.to_string())),
        "the guest must read as a partner of the seeded org, or the refusals below \
         prove nothing beyond the non-staff ones: {mine}"
    );
    refused_everywhere("partner").await;

    // Leave the guest a partner of nothing for the tests after this one: the
    // grant, the membership and the binding cascade with the org.
    entity::organizations::Entity::delete_by_id(partner.id)
        .exec(&db)
        .await
        .expect("remove the partner org");
}

/// The composition hands the admin seam the second split's sections behind
/// the capabilities `admin::router` gave them in `oxy-app`. Neither declares
/// anything (the console's `/{*rest}` FleetOk wildcard places them), so each
/// is found by capability: exactly one section each.
#[test]
fn the_composition_carries_the_second_split_sections() {
    use oxy_app::surface::Action;
    let seams = surface_seams();
    for capability in [Action::PlatformPartners, Action::PlatformApps] {
        let sections: Vec<_> = seams
            .admin
            .iter()
            .filter(|s| s.capability == capability)
            .collect();
        assert_eq!(sections.len(), 1, "sections behind {capability:?}");
        assert!(
            sections[0].decls.is_empty(),
            "{capability:?} declares nothing"
        );
    }
}

/// The two staff sections the admin split's fourth PR took out of `oxy-app`'s
/// `admin::router`, each now on `SurfaceSeams::admin` from `oxy-api-tenancy`:
/// an org's OLTP database (`PlatformOltp`) and a workspace's Airhouse tenant
/// (`PlatformAirhouse`). As `(method, path, what the Global Owner gets)`, read
/// off the served router on `main` before the move. `None` is a write this test
/// never sends as the owner — provisioning and dropping a database reach the
/// provider wherever one is configured — so the owner's TRACE 405 is what shows
/// it mounted, and the bounded operator below drives it into the fence.
const ADMIN_SPLIT_PR4_ROUTES: &[(&str, &str, Option<u16>)] = &[
    ("GET", "/admin/oltp", Some(200)),
    ("GET", "/admin/orgs/{o}/oltp", Some(200)),
    ("POST", "/admin/orgs/{o}/oltp/test-connection", Some(200)),
    ("POST", "/admin/orgs/{o}/oltp/provision", None),
    ("POST", "/admin/orgs/{o}/oltp/credentials", Some(422)),
    ("POST", "/admin/orgs/{o}/oltp/visibility", Some(422)),
    ("POST", "/admin/orgs/{o}/oltp/deprovision-writer", Some(422)),
    ("DELETE", "/admin/orgs/{o}/oltp", None),
    ("GET", "/admin/orgs/{o}/oltp/branches/staging", Some(200)),
    (
        "POST",
        "/admin/orgs/{o}/oltp/branches/staging/reset",
        Some(404),
    ),
    // Joined the OLTP section after the split (2026-11-21), when `oxy oltp
    // audit | rotate` became routes. The owner's random org has no database,
    // so the audit is a 404; a rotation reaches the provisioner, which needs
    // a provider wherever one is configured, so it is never sent as the owner.
    ("POST", "/admin/orgs/{o}/oltp/audit", Some(404)),
    ("POST", "/admin/orgs/{o}/oltp/roles/app:probe/rotate", None),
    ("GET", "/admin/airhouse", Some(200)),
    (
        "POST",
        "/admin/workspaces/{w}/airhouse/provision",
        Some(404),
    ),
    // Joined the Airhouse section after the split (2026-11-21), when
    // `oxy admin airhouse rotate-sa` became a route. Same shim, same answers:
    // the owner's random workspace is the shim's 404 before any rotation.
    (
        "POST",
        "/admin/workspaces/{w}/airhouse/rotate-sa",
        Some(404),
    ),
];

/// What a `global_admin` bounded to another org gets on each route, for an org
/// (and a workspace in it) that exists but sits outside its grant — read off
/// `main` before the move. The fleet lists FILTER (200); every org- or
/// workspace-keyed route answers the fence's 404, never 403, so the console
/// does not confirm the tenant exists. The three writes whose body names a
/// writer or role answer the JSON extractor's 422, which runs before the
/// handler's fence — as on `main`.
const ADMIN_SPLIT_PR4_OUT_OF_SCOPE: &[u16] = &[
    200, 404, 404, 404, 422, 422, 422, 404, 404, 404, 404, 404, 200, 404, 404,
];

fn admin_split_pr4_routes(o: &str, w: &str) -> Vec<(Method, String)> {
    ADMIN_SPLIT_PR4_ROUTES
        .iter()
        .map(|(m, p, _)| {
            (
                Method::from_bytes(m.as_bytes()).unwrap(),
                p.replace("{o}", o).replace("{w}", w),
            )
        })
        .collect()
}

/// The served router mounts the fourth split's two sections inside
/// `/api/admin`, behind the same layers `admin::router` gave them in
/// `oxy-app`, answering what `main` answered before the move:
///
/// * the guest, who is not staff: 403 on every route, TRACE included;
/// * the guest as the Global Owner: TRACE 405 (mounted), and each route's
///   handler speaking ([`ADMIN_SPLIT_PR4_ROUTES`]);
/// * the same owner while acting as a tenant: 403 everywhere;
/// * the guest as a partner: 403 everywhere, with the partner console's 200
///   as the control;
/// * the guest as an App Operator bounded to one org: 403 everywhere — it
///   holds neither capability, so the section's layer refuses it;
/// * the guest as a Global Admin bounded to one org, asking about another:
///   the per-org fence's answers ([`ADMIN_SPLIT_PR4_OUT_OF_SCOPE`]);
///
/// with a path nobody mounts answering 404 throughout.
#[tokio::test]
async fn the_admin_seam_mounts_the_fourth_split_sections_behind_the_staff_layers() {
    if db_unavailable() {
        return;
    }
    use oxy_app::surface::roles::{RouteRole, classify};
    use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait};

    let router = served(ServeMode::Cloud).await;
    let (x, y) = (
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
    );
    let routes = admin_split_pr4_routes(&x, &y);
    let control = "/admin/nothing-mounts-this";
    let refused_everywhere = |who: &'static str, routes: Vec<(Method, String)>| {
        let router = router.clone();
        async move {
            assert_eq!(
                status(&router, &Method::GET, control).await,
                StatusCode::NOT_FOUND,
                "{who}: control"
            );
            for (method, path) in &routes {
                for m in [method, &Method::TRACE] {
                    let got = admin_split_call(&router, m, path).await;
                    assert_eq!(got, StatusCode::FORBIDDEN, "{who} {m} {path}: got {got}");
                }
            }
        }
    };

    for (method, path) in &routes {
        assert_eq!(
            classify(method.as_str(), &format!("/api{path}")),
            RouteRole::FleetOk,
            "{method} {path} is Postgres plus a provider API and must stay FleetOk"
        );
    }

    refused_everywhere("non-staff", routes.clone()).await;

    let owner = OwnerEnv::set(oxy_auth::user::LOCAL_GUEST_EMAIL);
    for ((method, path), (_, _, expected)) in routes.iter().zip(ADMIN_SPLIT_PR4_ROUTES) {
        let got = admin_split_call(&router, &Method::TRACE, path).await;
        assert_eq!(
            got,
            StatusCode::METHOD_NOT_ALLOWED,
            "{path} must be mounted for staff, got {got}"
        );
        if let Some(expected) = expected {
            let got = admin_split_call(&router, method, path).await;
            assert_eq!(got.as_u16(), *expected, "owner {method} {path}: got {got}");
        }
    }

    let db = oxy::database::client::establish_connection()
        .await
        .expect("database");
    let (got, me) = get_json(&router, "/user").await;
    assert_eq!(got, StatusCode::OK, "GET /user: {me}");
    let user_id: uuid::Uuid = me["id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("GET /user carries no id: {me}"));
    let now = chrono::Utc::now();
    let org = |name: &'static str| {
        let id = uuid::Uuid::new_v4();
        entity::organizations::ActiveModel {
            id: ActiveValue::Set(id),
            name: ActiveValue::Set(name.into()),
            slug: ActiveValue::Set(format!("admin-split-4-{}", id.simple())),
            ..Default::default()
        }
    };

    // Acting as a tenant closes the console, these sections included.
    let acting = org("Admin split 4")
        .insert(&db)
        .await
        .expect("seed the org");
    let session = entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        actor_user_id: ActiveValue::Set(user_id),
        actor_email: ActiveValue::Set(oxy_auth::user::LOCAL_GUEST_EMAIL.into()),
        org_id: ActiveValue::Set(acting.id),
        reason: ActiveValue::Set("served-router admin split 4".into()),
        started_at: ActiveValue::Set(now.into()),
        // Short, so a panic below leaves the guest acting for two minutes at most.
        expires_at: ActiveValue::Set((now + chrono::Duration::minutes(2)).into()),
        ended_at: ActiveValue::Set(None),
        token_id: ActiveValue::Set(None),
    }
    .insert(&db)
    .await
    .expect("seed a live assume session");
    let mut acting_answers = Vec::new();
    for (method, path) in &routes {
        for m in [method, &Method::TRACE] {
            acting_answers.push((
                m.clone(),
                path.clone(),
                admin_split_call(&router, m, path).await,
            ));
        }
    }
    let acting_control = status(&router, &Method::GET, control).await;
    entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(session.id),
        ended_at: ActiveValue::Set(Some(chrono::Utc::now().into())),
        ..Default::default()
    }
    .update(&db)
    .await
    .expect("end the session");
    assert_eq!(acting_control, StatusCode::NOT_FOUND);
    for (method, path, got) in acting_answers {
        assert_eq!(
            got,
            StatusCode::FORBIDDEN,
            "acting {method} {path}: got {got}"
        );
    }

    // A partner, not staff: the sections refuse, the partner console answers.
    drop(owner);
    let partner = org("Admin split 4 partner")
        .insert(&db)
        .await
        .expect("seed the partner org");
    entity::partner_grants::ActiveModel {
        org_id: ActiveValue::Set(partner.id),
        status: ActiveValue::Set("active".into()),
        created_by: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now.into()),
    }
    .insert(&db)
    .await
    .expect("seed the grant");
    let member = entity::org_members::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        org_id: ActiveValue::Set(partner.id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(entity::org_members::OrgRole::Admin),
        created_at: ActiveValue::Set(now.into()),
        updated_at: ActiveValue::Set(now.into()),
    }
    .insert(&db)
    .await
    .expect("seed the membership");
    entity::partner_role_bindings::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        org_member_id: ActiveValue::Set(member.id),
        created_at: ActiveValue::Set(now.into()),
    }
    .insert(&db)
    .await
    .expect("seed partner access");
    let (got, mine) = get_json(&router, "/partners").await;
    assert_eq!(got, StatusCode::OK, "GET /partners: {mine}");
    assert!(
        mine.as_array().is_some_and(|all| all
            .iter()
            .any(|p| p["partner_id"] == partner.id.to_string())),
        "the guest must read as a partner of the seeded org, or the refusals below \
         prove nothing beyond the non-staff ones: {mine}"
    );
    refused_everywhere("partner", routes.clone()).await;
    entity::organizations::Entity::delete_by_id(partner.id)
        .exec(&db)
        .await
        .expect("remove the partner org");

    // Staff bounded to org A, asking about org B (which exists, with a
    // workspace): the App Operator is refused by the section's capability, the
    // Global Admin by the per-org fence.
    let in_scope = org("Admin split 4 in scope")
        .insert(&db)
        .await
        .expect("org A");
    let out_of_scope = org("Admin split 4 out of scope")
        .insert(&db)
        .await
        .expect("org B");
    let ws = entity::workspaces::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        name: ActiveValue::Set("admin split 4".into()),
        org_id: ActiveValue::Set(Some(out_of_scope.id)),
        created_at: ActiveValue::Set(now.into()),
        updated_at: ActiveValue::Set(now.into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("a workspace in org B");
    let elsewhere = admin_split_pr4_routes(&out_of_scope.id.to_string(), &ws.id.to_string());
    for role in ["app_operator", "global_admin"] {
        let grant = entity::app_admins::ActiveModel {
            id: ActiveValue::Set(uuid::Uuid::new_v4()),
            email: ActiveValue::Set(oxy_auth::user::LOCAL_GUEST_EMAIL.into()),
            granted_by: ActiveValue::Set(None),
            created_at: ActiveValue::Set(now.into()),
            role: ActiveValue::Set(role.into()),
            scope_all: ActiveValue::Set(false),
            updated_at: ActiveValue::Set(now.into()),
        }
        .insert(&db)
        .await
        .expect("seed the bounded grant");
        entity::app_admin_scope_orgs::ActiveModel {
            id: ActiveValue::Set(uuid::Uuid::new_v4()),
            app_admin_id: ActiveValue::Set(grant.id),
            org_id: ActiveValue::Set(in_scope.id),
            created_at: ActiveValue::Set(now.into()),
            created_by: ActiveValue::Set(None),
        }
        .insert(&db)
        .await
        .expect("bound it to org A");
        // The grant read is cached per process, and the guest's "no grant"
        // from the non-staff pass above is in it.
        oxy_server_authz::globals::invalidate_admin_cache();
        let mut answers = Vec::new();
        for (method, path) in &elsewhere {
            answers.push((
                method.clone(),
                path.clone(),
                admin_split_call(&router, method, path).await,
            ));
        }
        entity::app_admins::Entity::delete_by_id(grant.id)
            .exec(&db)
            .await
            .expect("remove the grant");
        oxy_server_authz::globals::invalidate_admin_cache();
        for ((method, path, got), main) in answers.iter().zip(ADMIN_SPLIT_PR4_OUT_OF_SCOPE) {
            let expected = if role == "app_operator" { 403 } else { *main };
            assert_eq!(got.as_u16(), expected, "{role} {method} {path}: got {got}");
        }
    }
    entity::workspaces::Entity::delete_by_id(ws.id)
        .exec(&db)
        .await
        .expect("remove the seeded workspace");
    for id in [in_scope.id, out_of_scope.id] {
        entity::organizations::Entity::delete_by_id(id)
            .exec(&db)
            .await
            .expect("remove the seeded org");
    }
}

/// The composition hands the admin seam the fourth split's sections behind
/// the capabilities `admin::router` gave them in `oxy-app`. Neither declares
/// anything (the console's `/{*rest}` FleetOk wildcard places them), so each
/// is found by capability: exactly one section each.
#[test]
fn the_composition_carries_the_fourth_split_sections() {
    use oxy_app::surface::Action;
    let seams = surface_seams();
    for capability in [Action::PlatformOltp, Action::PlatformAirhouse] {
        let sections: Vec<_> = seams
            .admin
            .iter()
            .filter(|s| s.capability == capability)
            .collect();
        assert_eq!(sections.len(), 1, "sections behind {capability:?}");
        assert!(
            sections[0].decls.is_empty(),
            "{capability:?} declares nothing"
        );
    }
}

/// The fifth admin split section: the user directory (`oxy-api-identity`,
/// `PlatformUsers`). As `(method, path, body, what the Global Owner gets for
/// ids that do not exist)`, read off the served router on `main` (6b0cd95ef)
/// before the move: the listings answer 200, everything naming a person or an
/// invitation that does not exist 404. `{u}`, `{o}` and `{i}` are a user, an
/// org and an invitation.
const ADMIN_SPLIT_PR5_ROUTES: &[(&str, &str, Option<&str>, u16)] = &[
    ("GET", "/admin/users", None, 200),
    ("GET", "/admin/users/{u}", None, 404),
    (
        "PATCH",
        "/admin/users/{u}/status",
        Some(r#"{"status":"active"}"#),
        404,
    ),
    ("GET", "/admin/users/{u}/org-memberships", None, 200),
    (
        "POST",
        "/admin/users/{u}/org-memberships",
        Some(r#"{"org_id":"{o}","role":"member"}"#),
        404,
    ),
    (
        "PATCH",
        "/admin/users/{u}/org-memberships/{o}",
        Some(r#"{"role":"member"}"#),
        404,
    ),
    ("DELETE", "/admin/users/{u}/org-memberships/{o}", None, 404),
    ("DELETE", "/admin/users/{u}/invitations/{i}", None, 404),
];

/// What a Global Admin bounded to org A gets for a person who is a member of
/// org B only, an invitation from B, and B itself — on `main`, the same order
/// as [`ADMIN_SPLIT_PR5_ROUTES`]. The directory lists (200, narrowed to A);
/// every read of the person and every write is the per-person or per-org
/// fence's 404, never 403: the grant holds `PlatformUsers`, so the section's
/// capability layer admits it and the handler is what refuses.
const ADMIN_SPLIT_PR5_OUT_OF_SCOPE: &[u16] = &[200, 404, 404, 404, 404, 404, 404, 404];

/// [`ADMIN_SPLIT_PR5_ROUTES`] with the ids filled in: `(method, path, body)`.
fn admin_split_pr5_routes(u: &str, o: &str, i: &str) -> Vec<(Method, String, Option<String>)> {
    ADMIN_SPLIT_PR5_ROUTES
        .iter()
        .map(|(m, p, body, _)| {
            (
                Method::from_bytes(m.as_bytes()).unwrap(),
                p.replace("{u}", u).replace("{o}", o).replace("{i}", i),
                body.map(|b| b.replace("{o}", o)),
            )
        })
        .collect()
}

/// One request, with its JSON body when it has one.
async fn admin_split_send(
    router: &Router,
    method: &Method,
    path: &str,
    body: Option<&str>,
) -> StatusCode {
    let mut req = Request::builder().method(method).uri(path);
    let body = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .expect("oneshot")
        .status()
}

/// A user row, for the bounded callers below to ask about.
async fn admin_split_pr5_user(db: &sea_orm::DatabaseConnection, tag: &str) -> uuid::Uuid {
    use sea_orm::{ActiveModelTrait, ActiveValue};
    let id = uuid::Uuid::new_v4();
    entity::users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(format!("{tag}-{id}@admin-split-5.test"))),
        name: ActiveValue::Set(format!("admin split 5 {tag}")),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        magic_link_token: ActiveValue::Set(None),
        magic_link_token_expires_at: ActiveValue::Set(None),
        status: ActiveValue::Set(entity::users::UserStatus::Active),
        created_at: ActiveValue::NotSet,
        last_login_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("seed a user");
    id
}

/// The served router mounts the fifth split's section inside `/api/admin`,
/// behind the same layers `admin::router` gave it in `oxy-app`, answering what
/// `main` answered before the move:
///
/// * the guest, who is not staff: 403 on every route, TRACE included;
/// * the guest as the Global Owner: TRACE 405 (mounted), and each route's
///   handler speaking ([`ADMIN_SPLIT_PR5_ROUTES`]);
/// * the same owner while acting as a tenant: 403 everywhere;
/// * the guest as a partner: 403 everywhere, with the partner console's 200
///   as the control;
/// * the guest as an App Operator bounded to one org: 403 everywhere — it
///   does not hold `PlatformUsers`, so the section's layer refuses it;
/// * the guest as a Global Admin bounded to org A, asking about a person in
///   org B: the per-person and per-org fences' 404s
///   ([`ADMIN_SPLIT_PR5_OUT_OF_SCOPE`]), and nothing the writes named moved;
///   asking about a person in A, the reads answer 200 (the control);
///
/// with a path nobody mounts answering 404 throughout.
#[tokio::test]
async fn the_admin_seam_mounts_the_fifth_split_section_behind_the_staff_layers() {
    if db_unavailable() {
        return;
    }
    use oxy_app::surface::roles::{RouteRole, classify};
    use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};

    let router = served(ServeMode::Cloud).await;
    let (x, y, z) = (
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
    );
    let routes = admin_split_pr5_routes(&x, &y, &z);
    let control = "/admin/nothing-mounts-this";
    let refused_everywhere = |who: &'static str, routes: Vec<(Method, String, Option<String>)>| {
        let router = router.clone();
        async move {
            assert_eq!(
                status(&router, &Method::GET, control).await,
                StatusCode::NOT_FOUND,
                "{who}: control"
            );
            for (method, path, body) in &routes {
                let got = admin_split_send(&router, method, path, body.as_deref()).await;
                assert_eq!(
                    got,
                    StatusCode::FORBIDDEN,
                    "{who} {method} {path}: got {got}"
                );
                let got = admin_split_send(&router, &Method::TRACE, path, None).await;
                assert_eq!(got, StatusCode::FORBIDDEN, "{who} TRACE {path}: got {got}");
            }
        }
    };

    for (method, path, _) in &routes {
        assert_eq!(
            classify(method.as_str(), &format!("/api{path}")),
            RouteRole::FleetOk,
            "{method} {path} is Postgres only and must stay FleetOk"
        );
    }

    refused_everywhere("non-staff", routes.clone()).await;

    let owner = OwnerEnv::set(oxy_auth::user::LOCAL_GUEST_EMAIL);
    for ((method, path, body), (_, _, _, expected)) in routes.iter().zip(ADMIN_SPLIT_PR5_ROUTES) {
        let got = admin_split_send(&router, &Method::TRACE, path, None).await;
        assert_eq!(
            got,
            StatusCode::METHOD_NOT_ALLOWED,
            "{path} must be mounted for staff, got {got}"
        );
        let got = admin_split_send(&router, method, path, body.as_deref()).await;
        assert_eq!(got.as_u16(), *expected, "owner {method} {path}: got {got}");
    }

    let db = oxy::database::client::establish_connection()
        .await
        .expect("database");
    let (got, me) = get_json(&router, "/user").await;
    assert_eq!(got, StatusCode::OK, "GET /user: {me}");
    let user_id: uuid::Uuid = me["id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("GET /user carries no id: {me}"));
    let now = chrono::Utc::now();
    let org = |name: &'static str| {
        let id = uuid::Uuid::new_v4();
        entity::organizations::ActiveModel {
            id: ActiveValue::Set(id),
            name: ActiveValue::Set(name.into()),
            slug: ActiveValue::Set(format!("admin-split-5-{}", id.simple())),
            ..Default::default()
        }
    };
    let membership = |user: uuid::Uuid, org: uuid::Uuid, role| entity::org_members::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(user),
        role: ActiveValue::Set(role),
        created_at: ActiveValue::Set(now.into()),
        updated_at: ActiveValue::Set(now.into()),
    };

    // Acting as a tenant closes the console, this section included.
    let acting = org("Admin split 5")
        .insert(&db)
        .await
        .expect("seed the org");
    let session = entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        actor_user_id: ActiveValue::Set(user_id),
        actor_email: ActiveValue::Set(oxy_auth::user::LOCAL_GUEST_EMAIL.into()),
        org_id: ActiveValue::Set(acting.id),
        reason: ActiveValue::Set("served-router admin split 5".into()),
        started_at: ActiveValue::Set(now.into()),
        // Short, so a panic below leaves the guest acting for two minutes at most.
        expires_at: ActiveValue::Set((now + chrono::Duration::minutes(2)).into()),
        ended_at: ActiveValue::Set(None),
        token_id: ActiveValue::Set(None),
    }
    .insert(&db)
    .await
    .expect("seed a live assume session");
    let mut acting_answers = Vec::new();
    for (method, path, body) in &routes {
        acting_answers.push((
            method.clone(),
            path.clone(),
            admin_split_send(&router, method, path, body.as_deref()).await,
        ));
        acting_answers.push((
            Method::TRACE,
            path.clone(),
            admin_split_send(&router, &Method::TRACE, path, None).await,
        ));
    }
    let acting_control = status(&router, &Method::GET, control).await;
    entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(session.id),
        ended_at: ActiveValue::Set(Some(chrono::Utc::now().into())),
        ..Default::default()
    }
    .update(&db)
    .await
    .expect("end the session");
    assert_eq!(acting_control, StatusCode::NOT_FOUND);
    for (method, path, got) in acting_answers {
        assert_eq!(
            got,
            StatusCode::FORBIDDEN,
            "acting {method} {path}: got {got}"
        );
    }

    // A partner, not staff: the section refuses, the partner console answers.
    drop(owner);
    let partner = org("Admin split 5 partner")
        .insert(&db)
        .await
        .expect("seed the partner org");
    entity::partner_grants::ActiveModel {
        org_id: ActiveValue::Set(partner.id),
        status: ActiveValue::Set("active".into()),
        created_by: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now.into()),
    }
    .insert(&db)
    .await
    .expect("seed the grant");
    let member = membership(user_id, partner.id, entity::org_members::OrgRole::Admin)
        .insert(&db)
        .await
        .expect("seed the membership");
    entity::partner_role_bindings::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        org_member_id: ActiveValue::Set(member.id),
        created_at: ActiveValue::Set(now.into()),
    }
    .insert(&db)
    .await
    .expect("seed partner access");
    let (got, mine) = get_json(&router, "/partners").await;
    assert_eq!(got, StatusCode::OK, "GET /partners: {mine}");
    assert!(
        mine.as_array().is_some_and(|all| all
            .iter()
            .any(|p| p["partner_id"] == partner.id.to_string())),
        "the guest must read as a partner of the seeded org, or the refusals below \
         prove nothing beyond the non-staff ones: {mine}"
    );
    refused_everywhere("partner", routes.clone()).await;
    entity::organizations::Entity::delete_by_id(partner.id)
        .exec(&db)
        .await
        .expect("remove the partner org");

    // Staff bounded to org A. A person in org B only, with a pending
    // invitation from B; and a person in A only, the in-scope control.
    let in_scope = org("Admin split 5 in scope")
        .insert(&db)
        .await
        .expect("org A");
    let out_of_scope = org("Admin split 5 out of scope")
        .insert(&db)
        .await
        .expect("org B");
    let person_b = admin_split_pr5_user(&db, "in-b").await;
    membership(
        person_b,
        out_of_scope.id,
        entity::org_members::OrgRole::Member,
    )
    .insert(&db)
    .await
    .expect("a member of org B");
    let person_a = admin_split_pr5_user(&db, "in-a").await;
    membership(person_a, in_scope.id, entity::org_members::OrgRole::Member)
        .insert(&db)
        .await
        .expect("a member of org A");
    let invitation_b = entity::org_invitations::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        org_id: ActiveValue::Set(out_of_scope.id),
        email: ActiveValue::Set(format!("invitee-{person_b}@admin-split-5.test")),
        role: ActiveValue::Set(entity::org_members::OrgRole::Member),
        invited_by: ActiveValue::Set(user_id),
        token: ActiveValue::Set(format!("admin-split-5-{}", uuid::Uuid::new_v4())),
        status: ActiveValue::Set(entity::org_invitations::InviteStatus::Pending),
        expires_at: ActiveValue::Set((now + chrono::Duration::days(7)).into()),
        created_at: ActiveValue::NotSet,
    }
    .insert(&db)
    .await
    .expect("an invitation from org B");
    let elsewhere = admin_split_pr5_routes(
        &person_b.to_string(),
        &out_of_scope.id.to_string(),
        &invitation_b.id.to_string(),
    );
    let at_home = admin_split_pr5_routes(
        &person_a.to_string(),
        &in_scope.id.to_string(),
        &uuid::Uuid::new_v4().to_string(),
    );
    for role in ["app_operator", "global_admin"] {
        let grant = entity::app_admins::ActiveModel {
            id: ActiveValue::Set(uuid::Uuid::new_v4()),
            email: ActiveValue::Set(oxy_auth::user::LOCAL_GUEST_EMAIL.into()),
            granted_by: ActiveValue::Set(None),
            created_at: ActiveValue::Set(now.into()),
            role: ActiveValue::Set(role.into()),
            scope_all: ActiveValue::Set(false),
            updated_at: ActiveValue::Set(now.into()),
        }
        .insert(&db)
        .await
        .expect("seed the bounded grant");
        entity::app_admin_scope_orgs::ActiveModel {
            id: ActiveValue::Set(uuid::Uuid::new_v4()),
            app_admin_id: ActiveValue::Set(grant.id),
            org_id: ActiveValue::Set(in_scope.id),
            created_at: ActiveValue::Set(now.into()),
            created_by: ActiveValue::Set(None),
        }
        .insert(&db)
        .await
        .expect("bound it to org A");
        // The grant read is cached per process, and the guest's "no grant"
        // from the non-staff pass above is in it.
        oxy_server_authz::globals::invalidate_admin_cache();
        let mut answers = Vec::new();
        for (method, path, body) in &elsewhere {
            answers.push((
                method.clone(),
                path.clone(),
                admin_split_send(&router, method, path, body.as_deref()).await,
            ));
        }
        // The reads about a person in A: the control that the 404s above are
        // the fence, not a section that answers nothing.
        let mut home_answers = Vec::new();
        for (method, path, _) in at_home.iter().filter(|(m, _, _)| *m == Method::GET) {
            home_answers.push((path.clone(), status(&router, method, path).await));
        }
        entity::app_admins::Entity::delete_by_id(grant.id)
            .exec(&db)
            .await
            .expect("remove the grant");
        oxy_server_authz::globals::invalidate_admin_cache();
        for ((method, path, got), main) in answers.iter().zip(ADMIN_SPLIT_PR5_OUT_OF_SCOPE) {
            let expected = if role == "app_operator" { 403 } else { *main };
            assert_eq!(got.as_u16(), expected, "{role} {method} {path}: got {got}");
        }
        for (path, got) in home_answers {
            let expected = if role == "app_operator" { 403 } else { 200 };
            assert_eq!(
                got.as_u16(),
                expected,
                "{role} in scope GET {path}: got {got}"
            );
        }
    }

    // Nothing the out-of-scope writes named moved.
    let still_member = entity::org_members::Entity::find()
        .filter(entity::org_members::Column::UserId.eq(person_b))
        .filter(entity::org_members::Column::OrgId.eq(out_of_scope.id))
        .one(&db)
        .await
        .expect("read the membership");
    assert!(still_member.is_some(), "org B's membership was removed");
    let still_invited = entity::org_invitations::Entity::find_by_id(invitation_b.id)
        .one(&db)
        .await
        .expect("read the invitation");
    assert!(still_invited.is_some(), "org B's invitation was revoked");

    for id in [in_scope.id, out_of_scope.id] {
        entity::organizations::Entity::delete_by_id(id)
            .exec(&db)
            .await
            .expect("remove the seeded org");
    }
    for id in [person_a, person_b] {
        entity::users::Entity::delete_by_id(id)
            .exec(&db)
            .await
            .expect("remove the seeded user");
    }
}

/// The composition hands the admin seam the fifth split's section behind
/// `PlatformUsers`, the capability `admin::router` gave it in `oxy-app`. It
/// declares nothing (the console's `/{*rest}` FleetOk wildcard places it).
#[test]
fn the_composition_carries_the_fifth_split_section() {
    use oxy_app::surface::Action;
    let seams = surface_seams();
    let sections: Vec<_> = seams
        .admin
        .iter()
        .filter(|s| s.capability == Action::PlatformUsers)
        .collect();
    assert_eq!(sections.len(), 1, "sections behind PlatformUsers");
    assert!(
        sections[0].decls.is_empty(),
        "PlatformUsers declares nothing"
    );
}

/// The staff console's two sibling nests (admin split, PR 6), `/admin/internal-jobs`
/// (in `oxy-app`) and `/admin/compiles` (`oxy-api-previews`), both folded through
/// `SurfaceSeams::admin_nests`' shape. As `(method, path, body, answers, trace)`,
/// read off the served router on `main` (853935eff) before the seam existed.
/// `answers` and `trace` (the same path asked with TRACE) are per caller, in
/// [`ADMIN_NEST_CALLERS`] order; `0` is a request this test does not send: a
/// write the in-scope reads skip, the `{}`-body variant's TRACE, and three
/// writes whose effect is global on the shared test database (the Global
/// Owner's reaper and retention runs, and a backfill that would queue a compile
/// for every uncompiled workspace it reaches). Their refusals are still sent.
/// `{w}`, `{r}` and `{t}` are a workspace, a revision and a task id.
///
/// What it pins, beyond each handler speaking for the Global Owner:
///
/// * the staff door, the refusal while acting and `PlatformOperate` answer on
///   every mounted path, a wrong verb included (TRACE 403 for a refused
///   caller, 405 for an admitted one; `POST /admin/compiles/nothing-here` is
///   the `/{revision_id}` route asked with a verb it does not take);
/// * an App Operator bounded to org A is refused by the capability layer
///   (the door admits any standing);
/// * a Global Admin bounded to org A gets the handlers' own fences on org B's
///   workspace and revision (404s, and "not found" per batch item), and org
///   A's rows on the reads;
/// * a path neither tree mounts, and each bare or slashed prefix that is not a
///   route, is the outer router's 404 for every caller — the guards never see
///   it, as on `main`.
const ADMIN_NEST_ROUTES: &[(&str, &str, Option<&str>, [u16; 8], [u16; 8])] = &[
    (
        "GET",
        "/admin/internal-jobs/queue-stats",
        None,
        [403, 200, 403, 403, 403, 200, 403, 200],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "GET",
        "/admin/internal-jobs/recent-failures",
        None,
        [403, 200, 403, 403, 403, 200, 403, 200],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "GET",
        "/admin/internal-jobs/failure-groups",
        None,
        [403, 200, 403, 403, 403, 200, 403, 200],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "GET",
        "/admin/internal-jobs/dead-letter",
        None,
        [403, 200, 403, 403, 403, 200, 403, 200],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "POST",
        "/admin/internal-jobs/dead-letter/{t}/reenqueue",
        None,
        [403, 404, 403, 403, 403, 404, 0, 0],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "DELETE",
        "/admin/internal-jobs/dead-letter/{t}",
        None,
        [403, 404, 403, 403, 403, 404, 0, 0],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "GET",
        "/admin/internal-jobs/workers",
        None,
        [403, 200, 403, 403, 403, 200, 403, 200],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "GET",
        "/admin/internal-jobs/scheduled",
        None,
        [403, 200, 403, 403, 403, 200, 403, 200],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "POST",
        "/admin/internal-jobs/run-reaper",
        None,
        [403, 0, 403, 403, 403, 404, 0, 0],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "POST",
        "/admin/internal-jobs/run-retention",
        None,
        [403, 0, 403, 403, 403, 404, 0, 0],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "GET",
        "/admin/internal-jobs/nothing-here",
        None,
        [404, 404, 404, 404, 404, 404, 404, 404],
        [404, 404, 404, 404, 404, 404, 404, 404],
    ),
    (
        "POST",
        "/admin/internal-jobs/nothing-here",
        None,
        [404, 404, 404, 404, 404, 404, 0, 0],
        [404, 404, 404, 404, 404, 404, 404, 404],
    ),
    (
        "GET",
        "/admin/internal-jobs",
        None,
        [404, 404, 404, 404, 404, 404, 404, 404],
        [404, 404, 404, 404, 404, 404, 404, 404],
    ),
    (
        "GET",
        "/admin/internal-jobs/",
        None,
        [404, 404, 404, 404, 404, 404, 404, 404],
        [404, 404, 404, 404, 404, 404, 404, 404],
    ),
    (
        "GET",
        "/admin/internal-jobsx",
        None,
        [404, 404, 404, 404, 404, 404, 404, 404],
        [404, 404, 404, 404, 404, 404, 404, 404],
    ),
    (
        "GET",
        "/admin/compiles",
        None,
        [403, 200, 403, 403, 403, 200, 403, 200],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "GET",
        "/admin/compiles/",
        None,
        [404, 404, 404, 404, 404, 404, 404, 404],
        [404, 404, 404, 404, 404, 404, 404, 404],
    ),
    (
        "GET",
        "/admin/compiles/workspaces",
        None,
        [403, 200, 403, 403, 403, 200, 403, 200],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "GET",
        "/admin/compiles?workspace_id={w}",
        None,
        [403, 200, 403, 403, 403, 200, 403, 200],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "GET",
        "/admin/compiles/{r}",
        None,
        [403, 404, 403, 403, 403, 404, 403, 200],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "POST",
        "/admin/compiles/run",
        Some(r#"{"workspace_id":"{w}"}"#),
        [403, 404, 403, 403, 403, 404, 0, 0],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "POST",
        "/admin/compiles/run",
        Some("{}"),
        [403, 422, 403, 403, 403, 422, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
    ),
    (
        "POST",
        "/admin/compiles/backfill",
        None,
        [403, 0, 403, 403, 403, 0, 0, 0],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "POST",
        "/admin/compiles/batch/run",
        Some(r#"{"workspace_ids":["{w}"]}"#),
        [403, 200, 403, 403, 403, 200, 0, 0],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "POST",
        "/admin/compiles/batch/run",
        Some("{}"),
        [403, 422, 403, 403, 403, 422, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
    ),
    (
        "POST",
        "/admin/compiles/batch/promote",
        Some(r#"{"revision_ids":["{r}"]}"#),
        [403, 200, 403, 403, 403, 200, 0, 0],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "POST",
        "/admin/compiles/batch/promote",
        Some("{}"),
        [403, 422, 403, 403, 403, 422, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
    ),
    (
        "POST",
        "/admin/compiles/{r}/promote",
        None,
        [403, 404, 403, 403, 403, 404, 0, 0],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "GET",
        "/admin/compiles/nothing-here/deeper",
        None,
        [404, 404, 404, 404, 404, 404, 404, 404],
        [404, 404, 404, 404, 404, 404, 404, 404],
    ),
    (
        "POST",
        "/admin/compiles/nothing-here",
        None,
        [403, 405, 403, 403, 403, 405, 0, 0],
        [403, 405, 403, 403, 403, 405, 403, 405],
    ),
    (
        "GET",
        "/admin/compilesx",
        None,
        [404, 404, 404, 404, 404, 404, 404, 404],
        [404, 404, 404, 404, 404, 404, 404, 404],
    ),
];

/// Who [`ADMIN_NEST_ROUTES`]' columns are, in order.
const ADMIN_NEST_CALLERS: [&str; 8] = [
    "non-staff",
    "owner",
    "owner acting",
    "partner",
    "app_operator@A about B",
    "global_admin@A about B",
    "app_operator@A about A",
    "global_admin@A about A",
];

/// Send every [`ADMIN_NEST_ROUTES`] row as caller `col`, with ids `(w, r, t)`,
/// and assert `main`'s answer.
async fn admin_nest_sweep(router: &Router, col: usize, ids: (&str, &str, &str)) {
    let (w, r, t) = ids;
    let who = ADMIN_NEST_CALLERS[col];
    for (method, path, body, answers, trace) in ADMIN_NEST_ROUTES {
        let path = path.replace("{w}", w).replace("{r}", r).replace("{t}", t);
        let body = body.map(|b| b.replace("{w}", w).replace("{r}", r));
        let method = Method::from_bytes(method.as_bytes()).unwrap();
        if answers[col] != 0 {
            let got = admin_split_send(router, &method, &path, body.as_deref()).await;
            assert_eq!(
                got.as_u16(),
                answers[col],
                "{who} {method} {path} {body:?}: got {got}"
            );
        }
        if trace[col] != 0 {
            let got = admin_split_send(router, &Method::TRACE, &path, None).await;
            assert_eq!(got.as_u16(), trace[col], "{who} TRACE {path}: got {got}");
        }
    }
}

/// The served router nests `/admin/internal-jobs` and `/admin/compiles` beside
/// the console behind the three layers the seam applies, answering exactly what
/// `main` answered before it (see [`ADMIN_NEST_ROUTES`]) for: the guest, who is
/// not staff; the guest as the Global Owner; the same owner while acting as a
/// tenant; the guest as a partner (the partner console's 200 as the control);
/// and the guest as an App Operator and then a Global Admin, each bounded to
/// org A, asking about org B and (reads only) about org A.
#[tokio::test]
async fn the_admin_nests_answer_what_main_did_behind_the_staff_layers() {
    if db_unavailable() {
        return;
    }
    use oxy_app::surface::roles::{RouteRole, classify};
    use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait};

    let router = served(ServeMode::Cloud).await;
    for prefix in ["/admin/internal-jobs", "/admin/compiles"] {
        for path in [prefix.to_string(), format!("{prefix}/anything")] {
            assert_eq!(
                classify("POST", &format!("/api{path}")),
                RouteRole::FleetOk,
                "{path} is Postgres only and must stay FleetOk"
            );
        }
    }
    let (x, y, z) = (
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
    );
    admin_nest_sweep(&router, 0, (&x, &y, &z)).await;

    let owner = OwnerEnv::set(oxy_auth::user::LOCAL_GUEST_EMAIL);
    admin_nest_sweep(&router, 1, (&x, &y, &z)).await;

    let db = oxy::database::client::establish_connection()
        .await
        .expect("database");
    let (got, me) = get_json(&router, "/user").await;
    assert_eq!(got, StatusCode::OK, "GET /user: {me}");
    let user_id: uuid::Uuid = me["id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("GET /user carries no id: {me}"));
    let now = chrono::Utc::now();
    let org = |name: &'static str| {
        let id = uuid::Uuid::new_v4();
        entity::organizations::ActiveModel {
            id: ActiveValue::Set(id),
            name: ActiveValue::Set(name.into()),
            slug: ActiveValue::Set(format!("admin-split-6-{}", id.simple())),
            ..Default::default()
        }
    };

    // Acting as a tenant closes both trees.
    let acting = org("Admin split 6")
        .insert(&db)
        .await
        .expect("seed the org");
    let session = entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        actor_user_id: ActiveValue::Set(user_id),
        actor_email: ActiveValue::Set(oxy_auth::user::LOCAL_GUEST_EMAIL.into()),
        org_id: ActiveValue::Set(acting.id),
        reason: ActiveValue::Set("served-router admin split 6".into()),
        started_at: ActiveValue::Set(now.into()),
        // Short, so a panic below leaves the guest acting for two minutes at most.
        expires_at: ActiveValue::Set((now + chrono::Duration::minutes(2)).into()),
        ended_at: ActiveValue::Set(None),
        token_id: ActiveValue::Set(None),
    }
    .insert(&db)
    .await
    .expect("seed a live assume session");
    // Collected before asserting, so a failure still ends the session.
    let acting_run = {
        let router = router.clone();
        let (x, y, z) = (x.clone(), y.clone(), z.clone());
        tokio::spawn(async move { admin_nest_sweep(&router, 2, (&x, &y, &z)).await })
    }
    .await;
    entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(session.id),
        ended_at: ActiveValue::Set(Some(chrono::Utc::now().into())),
        ..Default::default()
    }
    .update(&db)
    .await
    .expect("end the session");
    entity::organizations::Entity::delete_by_id(acting.id)
        .exec(&db)
        .await
        .expect("remove the acting org");
    if let Err(e) = acting_run {
        std::panic::resume_unwind(e.into_panic());
    }

    // A partner, not staff.
    drop(owner);
    let partner = org("Admin split 6 partner")
        .insert(&db)
        .await
        .expect("seed the partner org");
    entity::partner_grants::ActiveModel {
        org_id: ActiveValue::Set(partner.id),
        status: ActiveValue::Set("active".into()),
        created_by: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now.into()),
    }
    .insert(&db)
    .await
    .expect("seed the grant");
    let member = entity::org_members::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        org_id: ActiveValue::Set(partner.id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(entity::org_members::OrgRole::Admin),
        created_at: ActiveValue::Set(now.into()),
        updated_at: ActiveValue::Set(now.into()),
    }
    .insert(&db)
    .await
    .expect("seed the membership");
    entity::partner_role_bindings::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        org_member_id: ActiveValue::Set(member.id),
        created_at: ActiveValue::Set(now.into()),
    }
    .insert(&db)
    .await
    .expect("seed partner access");
    let (got, mine) = get_json(&router, "/partners").await;
    assert_eq!(got, StatusCode::OK, "GET /partners: {mine}");
    assert!(
        mine.as_array().is_some_and(|all| all
            .iter()
            .any(|p| p["partner_id"] == partner.id.to_string())),
        "the guest must read as a partner of the seeded org: {mine}"
    );
    admin_nest_sweep(&router, 3, (&x, &y, &z)).await;
    entity::organizations::Entity::delete_by_id(partner.id)
        .exec(&db)
        .await
        .expect("remove the partner org");

    // Staff bounded to org A; a workspace and a ready main revision in each org.
    let in_scope = org("Admin split 6 in scope")
        .insert(&db)
        .await
        .expect("org A");
    let out_of_scope = org("Admin split 6 out of scope")
        .insert(&db)
        .await
        .expect("org B");
    let workspace = |org: uuid::Uuid| {
        let id = uuid::Uuid::new_v4();
        entity::workspaces::ActiveModel {
            id: ActiveValue::Set(id),
            name: ActiveValue::Set(format!("admin-split-6-{}", id.simple())),
            org_id: ActiveValue::Set(Some(org)),
            path: ActiveValue::Set(Some(format!("/nonexistent/{id}"))),
            status: ActiveValue::Set(entity::workspaces::WorkspaceStatus::Ready),
            ..Default::default()
        }
    };
    let revision = |workspace: uuid::Uuid| entity::revisions::ActiveModel {
        revision_id: ActiveValue::Set(uuid::Uuid::new_v4()),
        workspace_id: ActiveValue::Set(workspace),
        git_sha: ActiveValue::Set("0123abc".into()),
        branch: ActiveValue::Set(Some("main".into())),
        schema_version: ActiveValue::Set(1),
        status: ActiveValue::Set("ready".into()),
        kind: ActiveValue::Set("main".into()),
        owner_user_id: ActiveValue::Set(None),
        compiler_version: ActiveValue::Set("test".into()),
        started_at: ActiveValue::Set(now.fixed_offset()),
        finished_at: ActiveValue::Set(Some(now.fixed_offset())),
        file_count_seen: ActiveValue::Set(0),
        file_count_compiled: ActiveValue::Set(0),
        file_count_failed: ActiveValue::Set(0),
        error_summary: ActiveValue::Set(None),
    };
    let ws_a = workspace(in_scope.id)
        .insert(&db)
        .await
        .expect("A's workspace")
        .id;
    let ws_b = workspace(out_of_scope.id)
        .insert(&db)
        .await
        .expect("B's workspace")
        .id;
    let rev_a = revision(ws_a)
        .insert(&db)
        .await
        .expect("A's revision")
        .revision_id;
    let rev_b = revision(ws_b)
        .insert(&db)
        .await
        .expect("B's revision")
        .revision_id;
    let (a, b) = (
        (ws_a.to_string(), rev_a.to_string()),
        (ws_b.to_string(), rev_b.to_string()),
    );
    for (role, out_col, in_col) in [("app_operator", 4, 6), ("global_admin", 5, 7)] {
        let grant = entity::app_admins::ActiveModel {
            id: ActiveValue::Set(uuid::Uuid::new_v4()),
            email: ActiveValue::Set(oxy_auth::user::LOCAL_GUEST_EMAIL.into()),
            granted_by: ActiveValue::Set(None),
            created_at: ActiveValue::Set(now.into()),
            role: ActiveValue::Set(role.into()),
            scope_all: ActiveValue::Set(false),
            updated_at: ActiveValue::Set(now.into()),
        }
        .insert(&db)
        .await
        .expect("seed the bounded grant");
        entity::app_admin_scope_orgs::ActiveModel {
            id: ActiveValue::Set(uuid::Uuid::new_v4()),
            app_admin_id: ActiveValue::Set(grant.id),
            org_id: ActiveValue::Set(in_scope.id),
            created_at: ActiveValue::Set(now.into()),
            created_by: ActiveValue::Set(None),
        }
        .insert(&db)
        .await
        .expect("bound it to org A");
        // The grant read is cached per process, and the guest's earlier
        // standing is in it.
        oxy_server_authz::globals::invalidate_admin_cache();
        let t = uuid::Uuid::new_v4().to_string();
        let run = {
            let router = router.clone();
            let (a, b, t) = (a.clone(), b.clone(), t.clone());
            tokio::spawn(async move {
                admin_nest_sweep(&router, out_col, (&b.0, &b.1, &t)).await;
                admin_nest_sweep(&router, in_col, (&a.0, &a.1, &t)).await;
            })
        }
        .await;
        entity::app_admins::Entity::delete_by_id(grant.id)
            .exec(&db)
            .await
            .expect("remove the grant");
        oxy_server_authz::globals::invalidate_admin_cache();
        if let Err(e) = run {
            std::panic::resume_unwind(e.into_panic());
        }
    }

    // Nothing the out-of-scope writes named moved: B still serves no revision,
    // and no compile was queued for B's workspace.
    let b_now = entity::workspaces::Entity::find_by_id(ws_b)
        .one(&db)
        .await
        .expect("read B's workspace")
        .expect("B's workspace");
    assert_eq!(
        b_now.current_revision_id, None,
        "B's workspace was repointed"
    );
    use sea_orm::ConnectionTrait;
    let queued: i64 = db
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT COUNT(*)::bigint AS n FROM agentic_runs WHERE workspace_id = $1",
            [ws_b.into()],
        ))
        .await
        .expect("count B's runs")
        .expect("a count row")
        .try_get("", "n")
        .expect("the count");
    assert_eq!(queued, 0, "a compile was queued for B's workspace");

    for id in [rev_a, rev_b] {
        entity::revisions::Entity::delete_by_id(id)
            .exec(&db)
            .await
            .expect("remove the seeded revision");
    }
    for id in [ws_a, ws_b] {
        entity::workspaces::Entity::delete_by_id(id)
            .exec(&db)
            .await
            .expect("remove the seeded workspace");
    }
    for id in [in_scope.id, out_of_scope.id] {
        entity::organizations::Entity::delete_by_id(id)
            .exec(&db)
            .await
            .expect("remove the seeded org");
    }
}

/// The composition hands `SurfaceSeams::admin_nests` exactly the task
/// fleet's console (`oxy-api-admin`, admin split PR 7) under
/// `/internal-jobs` and then the compile console (`oxy-api-previews`, PR 6)
/// under `/compiles`, each behind `PlatformOperate` and FleetOk — the
/// capability and placement `oxy-app` gave its `nest_all` before the moves.
/// In that order: `global.rs` folds the nests as handed in, so the order is
/// the route-role table's (internal jobs, then compiles, as on `main`).
#[test]
fn the_composition_carries_the_compile_console_nest() {
    use oxy_app::surface::Action;
    use oxy_app::surface::roles::RouteRole;
    let seams = surface_seams();
    let nests: Vec<_> = seams
        .admin_nests
        .iter()
        .map(|n| (n.prefix, n.capability, n.role))
        .collect();
    assert_eq!(
        nests,
        vec![
            (
                "/internal-jobs",
                Action::PlatformOperate,
                RouteRole::FleetOk
            ),
            ("/compiles", Action::PlatformOperate, RouteRole::FleetOk),
        ]
    );
}

/// The platform audit log's routes (`oxy-api-admin`'s section since the admin
/// split's audit move, 2026-11-20), as `(method, path, owner, bounded global
/// admin asking about org B, the same asking about org A)`, read off the
/// served router on `main` (dfd8ec6edc) before the move. `{o}` is an org id.
///
/// Every other caller this test sends — not staff, the owner while acting,
/// an App Operator bounded to org A (no `PlatformAudit`) — is 403 on each of
/// these, the wrong verbs included.
const ADMIN_AUDIT_ROUTES: &[(&str, &str, u16, u16, u16)] = &[
    ("GET", "/admin/audit", 200, 200, 200),
    ("GET", "/admin/audit?org_id={o}", 200, 200, 200),
    ("GET", "/admin/audit?org_id=not-a-uuid", 400, 400, 400),
    ("GET", "/admin/audit/verify/{o}", 200, 404, 200),
    ("GET", "/admin/audit/anchor/{o}", 200, 404, 200),
    ("GET", "/admin/audit/verify/not-a-uuid", 400, 400, 400),
    ("POST", "/admin/audit", 405, 405, 405),
    ("DELETE", "/admin/audit/verify/{o}", 405, 405, 405),
    ("TRACE", "/admin/audit", 405, 405, 405),
    ("TRACE", "/admin/audit/anchor/{o}", 405, 405, 405),
];

/// Paths the section does not mount: the outer router's 404 for every caller.
const ADMIN_AUDIT_UNMOUNTED: &[&str] = &[
    "/admin/audit/",
    "/admin/audit/verify",
    "/admin/audit/verify/{o}/deeper",
    "/admin/auditx",
];

async fn admin_audit_sweep(router: &Router, who: &str, org: &str, col: Option<usize>) {
    for (m, p, owner, ga_out, ga_in) in ADMIN_AUDIT_ROUTES {
        let path = p.replace("{o}", org);
        let method = Method::from_bytes(m.as_bytes()).unwrap();
        let want = match col {
            None => 403,
            Some(0) => *owner,
            Some(1) => *ga_out,
            Some(_) => *ga_in,
        };
        let got = status(router, &method, &path).await;
        assert_eq!(got.as_u16(), want, "{who} {m} {p}: got {got}");
    }
    for p in ADMIN_AUDIT_UNMOUNTED {
        let path = p.replace("{o}", org);
        let got = status(router, &Method::GET, &path).await;
        assert_eq!(got, StatusCode::NOT_FOUND, "{who} GET {p}");
    }
}

/// The served router mounts the audit log inside `/api/admin` behind the
/// layers `admin::router` gave it in `oxy-app` — the staff door, the refusal
/// while acting, and `PlatformAudit` — answering what `main` answered before
/// the move ([`ADMIN_AUDIT_ROUTES`]). The bounded App Operator is the
/// capability layer's witness: the door admits any standing, so without
/// `PlatformAudit` on the section it would reach the handlers. The bounded
/// Global Admin's 404s on org B are the handlers' own fences.
#[tokio::test]
async fn the_admin_seam_mounts_the_audit_section_behind_the_staff_layers() {
    if db_unavailable() {
        return;
    }
    use oxy_app::surface::roles::{RouteRole, classify};
    use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};

    let router = served(ServeMode::Cloud).await;
    let nobody = uuid::Uuid::new_v4().to_string();
    for (m, p, ..) in ADMIN_AUDIT_ROUTES {
        assert_eq!(
            classify(m, &format!("/api{}", p.replace("{o}", &nobody))),
            RouteRole::FleetOk,
            "{m} {p} is Postgres (and the anchor's S3) only and must stay FleetOk"
        );
    }

    // A run that panicked mid-way (a retry, say) leaves its bounded grant
    // behind, which would make the guest staff below; these tests are serial
    // (`serial-db`), so clear the guest's grants before the non-staff pass.
    let db = oxy::database::client::establish_connection()
        .await
        .expect("database");
    entity::app_admins::Entity::delete_many()
        .filter(entity::app_admins::Column::Email.eq(oxy_auth::user::LOCAL_GUEST_EMAIL))
        .exec(&db)
        .await
        .expect("clear a stale guest grant");
    oxy_server_authz::globals::invalidate_admin_cache();

    admin_audit_sweep(&router, "non-staff", &nobody, None).await;
    let owner = OwnerEnv::set(oxy_auth::user::LOCAL_GUEST_EMAIL);
    admin_audit_sweep(&router, "owner", &nobody, Some(0)).await;

    let (got, me) = get_json(&router, "/user").await;
    assert_eq!(got, StatusCode::OK, "GET /user: {me}");
    let user_id: uuid::Uuid = me["id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("GET /user carries no id: {me}"));
    let now = chrono::Utc::now();
    let org = |name: &'static str| {
        let id = uuid::Uuid::new_v4();
        entity::organizations::ActiveModel {
            id: ActiveValue::Set(id),
            name: ActiveValue::Set(name.into()),
            slug: ActiveValue::Set(format!("admin-audit-{}", id.simple())),
            ..Default::default()
        }
    };

    // Acting as a tenant closes the console, the audit log included.
    let acting = org("Admin audit acting")
        .insert(&db)
        .await
        .expect("seed the org");
    let session = entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        actor_user_id: ActiveValue::Set(user_id),
        actor_email: ActiveValue::Set(oxy_auth::user::LOCAL_GUEST_EMAIL.into()),
        org_id: ActiveValue::Set(acting.id),
        reason: ActiveValue::Set("served-router admin audit".into()),
        started_at: ActiveValue::Set(now.into()),
        // Short, so a panic below leaves the guest acting for two minutes at most.
        expires_at: ActiveValue::Set((now + chrono::Duration::minutes(2)).into()),
        ended_at: ActiveValue::Set(None),
        token_id: ActiveValue::Set(None),
    }
    .insert(&db)
    .await
    .expect("seed a live assume session");
    let mut acting_answers = Vec::new();
    for (m, p, ..) in ADMIN_AUDIT_ROUTES {
        let method = Method::from_bytes(m.as_bytes()).unwrap();
        let path = p.replace("{o}", &nobody);
        acting_answers.push((*m, *p, status(&router, &method, &path).await));
    }
    entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(session.id),
        ended_at: ActiveValue::Set(Some(chrono::Utc::now().into())),
        ..Default::default()
    }
    .update(&db)
    .await
    .expect("end the session");
    for (m, p, got) in acting_answers {
        assert_eq!(got, StatusCode::FORBIDDEN, "acting {m} {p}: got {got}");
    }
    drop(owner);

    // Staff bounded to org A, asking about org B and then about org A.
    let in_scope = org("Admin audit in scope")
        .insert(&db)
        .await
        .expect("org A");
    let out_of_scope = org("Admin audit out of scope")
        .insert(&db)
        .await
        .expect("org B");
    for role in ["app_operator", "global_admin"] {
        let grant = entity::app_admins::ActiveModel {
            id: ActiveValue::Set(uuid::Uuid::new_v4()),
            email: ActiveValue::Set(oxy_auth::user::LOCAL_GUEST_EMAIL.into()),
            granted_by: ActiveValue::Set(None),
            created_at: ActiveValue::Set(now.into()),
            role: ActiveValue::Set(role.into()),
            scope_all: ActiveValue::Set(false),
            updated_at: ActiveValue::Set(now.into()),
        }
        .insert(&db)
        .await
        .expect("seed the bounded grant");
        entity::app_admin_scope_orgs::ActiveModel {
            id: ActiveValue::Set(uuid::Uuid::new_v4()),
            app_admin_id: ActiveValue::Set(grant.id),
            org_id: ActiveValue::Set(in_scope.id),
            created_at: ActiveValue::Set(now.into()),
            created_by: ActiveValue::Set(None),
        }
        .insert(&db)
        .await
        .expect("bound it to org A");
        // The grant read is cached per process, and the guest's "no grant"
        // from the non-staff pass above is in it.
        oxy_server_authz::globals::invalidate_admin_cache();
        let (out_col, in_col) = if role == "app_operator" {
            (None, None)
        } else {
            (Some(1), Some(2))
        };
        let b = out_of_scope.id.to_string();
        let a = in_scope.id.to_string();
        admin_audit_sweep(&router, &format!("{role}@A asking B"), &b, out_col).await;
        admin_audit_sweep(&router, &format!("{role}@A asking A"), &a, in_col).await;
        entity::app_admins::Entity::delete_by_id(grant.id)
            .exec(&db)
            .await
            .expect("remove the grant");
        oxy_server_authz::globals::invalidate_admin_cache();
    }
    for id in [acting.id, in_scope.id, out_of_scope.id] {
        entity::organizations::Entity::delete_by_id(id)
            .exec(&db)
            .await
            .expect("remove the seeded org");
    }
}

/// The composition hands the admin seam the audit log behind `PlatformAudit`,
/// the capability `admin::router` gave it in `oxy-app`. It declares nothing
/// (the console's `/{*rest}` FleetOk wildcard places it).
#[test]
fn the_composition_carries_the_audit_section() {
    use oxy_app::surface::Action;
    let seams = surface_seams();
    let sections: Vec<_> = seams
        .admin
        .iter()
        .filter(|s| s.capability == Action::PlatformAudit)
        .collect();
    assert_eq!(sections.len(), 1, "sections behind PlatformAudit");
    assert!(
        sections[0].decls.is_empty(),
        "PlatformAudit declares nothing"
    );
}

/// The grant table's routes (`oxy-api-admin`'s section since the admin split's
/// app-admins move, 2026-11-20), as `(method, path, body, owner, Global Admin
/// bounded to org A)`, read off the served router on `main` (2b45e1761d)
/// before the move. `{a}` / `{b}` are org A and org B, `{t}` a fresh target
/// email, `{row}` the id of a grant bounded to org A and `{peer}` one bounded
/// to A and B, both seeded by the owner first. Run in order: the owner's
/// writes are undone before the bounded pass.
///
/// Every other caller this test sends — not staff, the owner while acting, an
/// App Operator bounded to org A (no `PlatformGrants`) — is 403 on each of
/// these, the wrong verbs included.
const ADMIN_APP_ADMINS_ROUTES: &[(&str, &str, Option<&str>, u16, u16)] = &[
    ("GET", "/admin/app-admins", None, 200, 200),
    (
        "POST",
        "/admin/app-admins",
        Some(r#"{"email":"{t}","role":"app_operator","scope_org_ids":["{a}"]}"#),
        200,
        200,
    ),
    (
        "POST",
        "/admin/app-admins",
        Some(r#"{"email":"{t}","role":"app_operator","scope_org_ids":["{b}"]}"#),
        200,
        403,
    ),
    (
        "POST",
        "/admin/app-admins",
        Some(r#"{"email":"{t}","role":"global_admin","scope_org_ids":["{a}"]}"#),
        200,
        403,
    ),
    (
        "POST",
        "/admin/app-admins",
        Some(r#"{"email":"no-at-sign"}"#),
        400,
        400,
    ),
    (
        "POST",
        "/admin/app-admins",
        Some(r#"{"email":"{t}","role":"bogus"}"#),
        400,
        400,
    ),
    ("DELETE", "/admin/app-admins/{peer}", None, 204, 403),
    ("DELETE", "/admin/app-admins/{row}", None, 204, 204),
    (
        "DELETE",
        "/admin/app-admins/00000000-0000-0000-0000-00000000dead",
        None,
        404,
        404,
    ),
    ("DELETE", "/admin/app-admins/not-a-uuid", None, 400, 400),
    ("PUT", "/admin/app-admins", None, 405, 405),
    ("DELETE", "/admin/app-admins", None, 405, 405),
    ("GET", "/admin/app-admins/{row}", None, 405, 405),
    ("TRACE", "/admin/app-admins", None, 405, 405),
    ("TRACE", "/admin/app-admins/{row}", None, 405, 405),
];

/// Paths the section does not mount: the outer router's 404 for every caller.
const ADMIN_APP_ADMINS_UNMOUNTED: &[&str] = &[
    "/admin/app-admins/",
    "/admin/app-admins/{row}/deeper",
    "/admin/app-adminsx",
];

/// What the bounded test world needs to fill a row of
/// [`ADMIN_APP_ADMINS_ROUTES`].
struct GrantWorld {
    db: sea_orm::DatabaseConnection,
    a: uuid::Uuid,
    b: uuid::Uuid,
    target: String,
    row: String,
    peer: String,
}

impl GrantWorld {
    fn fill(&self, s: &str) -> String {
        s.replace("{a}", &self.a.to_string())
            .replace("{b}", &self.b.to_string())
            .replace("{t}", &self.target)
            .replace("{row}", &self.row)
            .replace("{peer}", &self.peer)
    }

    /// (Re)create the two target grants as the owner would, straight into
    /// the table, and point `{row}` / `{peer}` at them.
    async fn seed(&mut self, tag: &str) {
        use sea_orm::{ActiveModelTrait, ActiveValue};
        self.clear().await;
        let now = chrono::Utc::now();
        let mut ids = Vec::new();
        for (who, scope) in [("row", vec![self.a]), ("peer", vec![self.a, self.b])] {
            let grant = entity::app_admins::ActiveModel {
                id: ActiveValue::Set(uuid::Uuid::new_v4()),
                email: ActiveValue::Set(format!("{who}-{tag}-{}", self.target)),
                granted_by: ActiveValue::Set(None),
                created_at: ActiveValue::Set(now.into()),
                role: ActiveValue::Set("app_operator".into()),
                scope_all: ActiveValue::Set(false),
                updated_at: ActiveValue::Set(now.into()),
            }
            .insert(&self.db)
            .await
            .expect("seed a target grant");
            for org_id in scope {
                entity::app_admin_scope_orgs::ActiveModel {
                    id: ActiveValue::Set(uuid::Uuid::new_v4()),
                    app_admin_id: ActiveValue::Set(grant.id),
                    org_id: ActiveValue::Set(org_id),
                    created_at: ActiveValue::Set(now.into()),
                    created_by: ActiveValue::Set(None),
                }
                .insert(&self.db)
                .await
                .expect("scope the target grant");
            }
            ids.push(grant.id.to_string());
        }
        self.row = ids[0].clone();
        self.peer = ids[1].clone();
    }

    /// Remove every grant this test made for a target (never the guest's).
    async fn clear(&self) {
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        entity::app_admins::Entity::delete_many()
            .filter(entity::app_admins::Column::Email.contains(&self.target))
            .exec(&self.db)
            .await
            .expect("clear the target grants");
    }
}

async fn admin_app_admins_call(
    router: &Router,
    method: &Method,
    path: &str,
    body: Option<&str>,
) -> StatusCode {
    let mut req = Request::builder().method(method).uri(path);
    let body = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .expect("oneshot")
        .status()
}

async fn admin_app_admins_sweep(
    router: &Router,
    who: &str,
    w: &mut GrantWorld,
    col: Option<usize>,
) {
    w.seed(who.split('@').next().unwrap_or(who)).await;
    for (m, p, body, owner, ga) in ADMIN_APP_ADMINS_ROUTES {
        let method = Method::from_bytes(m.as_bytes()).unwrap();
        let want = match col {
            None => 403,
            Some(0) => *owner,
            Some(_) => *ga,
        };
        let path = w.fill(p);
        let body = body.map(|b| w.fill(b));
        let got = admin_app_admins_call(router, &method, &path, body.as_deref()).await;
        assert_eq!(got.as_u16(), want, "{who} {m} {p}: got {got}");
    }
    for p in ADMIN_APP_ADMINS_UNMOUNTED {
        let got = status(router, &Method::GET, &w.fill(p)).await;
        assert_eq!(got, StatusCode::NOT_FOUND, "{who} GET {p}");
    }
}

/// `platform.grant.*` audit rows naming `target`, oldest first, as their action.
async fn grant_audit_actions(db: &sea_orm::DatabaseConnection, target: &str) -> Vec<String> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
    entity::audit_events::Entity::find()
        .filter(entity::audit_events::Column::Action.starts_with("platform.grant."))
        .filter(entity::audit_events::Column::TargetLabel.eq(target))
        .order_by_asc(entity::audit_events::Column::Seq)
        .all(db)
        .await
        .expect("read the grant audit rows")
        .into_iter()
        .map(|e| e.action)
        .collect()
}

/// The served router mounts the grant table inside `/api/admin` behind the
/// layers `admin::router` gave it in `oxy-app` — the staff door, the refusal
/// while acting, and `PlatformGrants` — answering what `main` answered before
/// the move ([`ADMIN_APP_ADMINS_ROUTES`]). The bounded App Operator is the
/// capability layer's witness: the door admits any standing, and its GET
/// would list every grant without `PlatformGrants` on the section (its writes
/// would still meet `may_delegate`). The bounded Global Admin's 403s are the
/// handlers' own row fence. Writes are audited, and each one clears the grant
/// cache itself: the guest's own grant, written and revoked through the
/// handler, takes effect on the very next request with no invalidation here.
#[tokio::test]
async fn the_admin_seam_mounts_the_app_admins_section_behind_the_staff_layers() {
    if db_unavailable() {
        return;
    }
    use oxy_app::surface::roles::{RouteRole, classify};
    use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};

    let router = served(ServeMode::Cloud).await;
    for (m, p, ..) in ADMIN_APP_ADMINS_ROUTES {
        let path = p.replace("{row}", &uuid::Uuid::new_v4().to_string());
        assert_eq!(
            classify(m, &format!("/api{path}")),
            RouteRole::FleetOk,
            "{m} {p} is Postgres only and must stay FleetOk"
        );
    }

    // A run that panicked mid-way leaves its grant behind, which would make
    // the guest staff below; these tests are serial (`serial-db`).
    let db = oxy::database::client::establish_connection()
        .await
        .expect("database");
    let guest = oxy_auth::user::LOCAL_GUEST_EMAIL;
    entity::app_admins::Entity::delete_many()
        .filter(entity::app_admins::Column::Email.eq(guest))
        .exec(&db)
        .await
        .expect("clear a stale guest grant");
    oxy_server_authz::globals::invalidate_admin_cache();

    let org = |name: &'static str| {
        let id = uuid::Uuid::new_v4();
        entity::organizations::ActiveModel {
            id: ActiveValue::Set(id),
            name: ActiveValue::Set(name.into()),
            slug: ActiveValue::Set(format!("admin-grants-{}", id.simple())),
            ..Default::default()
        }
    };
    let a = org("Admin grants in scope")
        .insert(&db)
        .await
        .expect("org A");
    let b = org("Admin grants out of scope")
        .insert(&db)
        .await
        .expect("org B");
    let mut w = GrantWorld {
        db: db.clone(),
        a: a.id,
        b: b.id,
        target: format!("grant-{}@served.test", uuid::Uuid::new_v4().simple()),
        row: String::new(),
        peer: String::new(),
    };

    admin_app_admins_sweep(&router, "non-staff", &mut w, None).await;
    assert!(
        grant_audit_actions(&db, &w.target).await.is_empty(),
        "a refused write was audited"
    );
    let owner = OwnerEnv::set(guest);
    admin_app_admins_sweep(&router, "owner", &mut w, Some(0)).await;
    // The owner's three admissible writes to `{t}`: a create, then two updates.
    assert_eq!(
        grant_audit_actions(&db, &w.target).await,
        vec![
            "platform.grant.created",
            "platform.grant.updated",
            "platform.grant.updated"
        ]
    );

    let (got, me) = get_json(&router, "/user").await;
    assert_eq!(got, StatusCode::OK, "GET /user: {me}");
    let user_id: uuid::Uuid = me["id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("GET /user carries no id: {me}"));

    // Acting as a tenant closes the console, the grant table included.
    let now = chrono::Utc::now();
    let acting = org("Admin grants acting")
        .insert(&db)
        .await
        .expect("seed the org");
    let session = entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        actor_user_id: ActiveValue::Set(user_id),
        actor_email: ActiveValue::Set(guest.into()),
        org_id: ActiveValue::Set(acting.id),
        reason: ActiveValue::Set("served-router admin grants".into()),
        started_at: ActiveValue::Set(now.into()),
        // Short, so a panic below leaves the guest acting for two minutes at most.
        expires_at: ActiveValue::Set((now + chrono::Duration::minutes(2)).into()),
        ended_at: ActiveValue::Set(None),
        token_id: ActiveValue::Set(None),
    }
    .insert(&db)
    .await
    .expect("seed a live assume session");
    w.seed("acting").await;
    let mut acting_answers = Vec::new();
    for (m, p, body, ..) in ADMIN_APP_ADMINS_ROUTES {
        let method = Method::from_bytes(m.as_bytes()).unwrap();
        let body = body.map(|b| w.fill(b));
        let got = admin_app_admins_call(&router, &method, &w.fill(p), body.as_deref()).await;
        acting_answers.push((*m, *p, got));
    }
    entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(session.id),
        ended_at: ActiveValue::Set(Some(chrono::Utc::now().into())),
        ..Default::default()
    }
    .update(&db)
    .await
    .expect("end the session");
    for (m, p, got) in acting_answers {
        assert_eq!(got, StatusCode::FORBIDDEN, "acting {m} {p}: got {got}");
    }

    // The handlers clear the grant cache themselves. "No grant" for the guest
    // is cached by the non-owner request first; nothing below invalidates.
    w.clear().await;
    drop(owner);
    assert_eq!(
        status(&router, &Method::GET, "/admin/app-admins").await,
        StatusCode::FORBIDDEN,
        "the guest holds no grant yet"
    );
    let owner = OwnerEnv::set(guest);
    let grant_guest = format!(r#"{{"email":"{guest}"}}"#);
    let got = admin_app_admins_call(
        &router,
        &Method::POST,
        "/admin/app-admins",
        Some(&grant_guest),
    )
    .await;
    assert_eq!(got, StatusCode::OK, "the owner grants the guest");
    drop(owner);
    assert_eq!(
        status(&router, &Method::GET, "/admin/app-admins").await,
        StatusCode::OK,
        "a grant written through the console is in force on the next request"
    );
    let guest_grant = entity::app_admins::Entity::find()
        .filter(entity::app_admins::Column::Email.eq(guest))
        .one(&db)
        .await
        .expect("read the guest's grant")
        .expect("the guest's grant was written");
    let owner = OwnerEnv::set(guest);
    let got = admin_app_admins_call(
        &router,
        &Method::DELETE,
        &format!("/admin/app-admins/{}", guest_grant.id),
        None,
    )
    .await;
    assert_eq!(got, StatusCode::NO_CONTENT, "the owner revokes it");
    drop(owner);
    assert_eq!(
        status(&router, &Method::GET, "/admin/app-admins").await,
        StatusCode::FORBIDDEN,
        "a grant revoked through the console is gone on the next request"
    );
    assert_eq!(
        grant_audit_actions(&db, guest)
            .await
            .last()
            .map(String::as_str),
        Some("platform.grant.revoked")
    );

    // Staff bounded to org A: an App Operator (no `PlatformGrants`), then a
    // Global Admin, whose writes the delegation bound narrows to org A.
    for role in ["app_operator", "global_admin"] {
        let grant = entity::app_admins::ActiveModel {
            id: ActiveValue::Set(uuid::Uuid::new_v4()),
            email: ActiveValue::Set(guest.into()),
            granted_by: ActiveValue::Set(None),
            created_at: ActiveValue::Set(now.into()),
            role: ActiveValue::Set(role.into()),
            scope_all: ActiveValue::Set(false),
            updated_at: ActiveValue::Set(now.into()),
        }
        .insert(&db)
        .await
        .expect("seed the bounded grant");
        entity::app_admin_scope_orgs::ActiveModel {
            id: ActiveValue::Set(uuid::Uuid::new_v4()),
            app_admin_id: ActiveValue::Set(grant.id),
            org_id: ActiveValue::Set(a.id),
            created_at: ActiveValue::Set(now.into()),
            created_by: ActiveValue::Set(None),
        }
        .insert(&db)
        .await
        .expect("bound it to org A");
        oxy_server_authz::globals::invalidate_admin_cache();
        let col = (role == "global_admin").then_some(1);
        admin_app_admins_sweep(&router, &format!("{role}@A"), &mut w, col).await;
        entity::app_admins::Entity::delete_by_id(grant.id)
            .exec(&db)
            .await
            .expect("remove the grant");
        oxy_server_authz::globals::invalidate_admin_cache();
    }
    w.clear().await;
    for id in [acting.id, a.id, b.id] {
        entity::organizations::Entity::delete_by_id(id)
            .exec(&db)
            .await
            .expect("remove the seeded org");
    }
}

/// The composition hands the admin seam the grant table behind
/// `PlatformGrants`, the capability `admin::router` gave it in `oxy-app`,
/// declaring nothing (the console's `/{*rest}` FleetOk wildcard places it).
/// Identified by what it mounts, not by its capability: identity's standing
/// tokens are behind `PlatformGrants` too.
#[tokio::test]
async fn the_composition_carries_the_app_admins_section() {
    let seams = surface_seams();
    let mut carrying = Vec::new();
    for section in seams.admin {
        let got = section
            .routes
            .clone()
            .with_state(oxy_app::surface::bare_app_state())
            .oneshot(
                Request::builder()
                    .method(Method::TRACE)
                    .uri("/app-admins")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot")
            .status();
        if got == StatusCode::METHOD_NOT_ALLOWED {
            carrying.push((section.capability, section.decls.len()));
        }
    }
    assert_eq!(
        carrying,
        vec![(oxy_app::surface::Action::PlatformGrants, 0)],
        "the sections mounting /app-admins"
    );
}

/// The weekly usage report's routes (`oxy-api-billing`'s two sections since
/// admin split PR 8b), as `(method, path, staff answer)`, read off the served
/// router on `main` (2b45e1761d) before the move. The staff answer is the
/// owner's, and a Global Admin's bounded to one org: both hold
/// `PlatformOperate` and `PlatformGrants`. No report is written, so the
/// report is `null` and a copy is `no_report`.
///
/// Every other caller this test sends — not staff, the owner while acting,
/// an App Operator (neither capability) — is 403 on each of these, the wrong
/// verbs included.
const USAGE_REPORT_ROUTES: &[(&str, &str, u16)] = &[
    ("GET", "/admin/usage-report", 200),
    ("GET", "/admin/usage-report/email-preference", 200),
    ("POST", "/admin/usage-report/send-to-me", 404),
    ("GET", "/admin/usage-report/recipients", 200),
    ("POST", "/admin/usage-report", 405),
    ("DELETE", "/admin/usage-report/email-preference", 405),
    ("GET", "/admin/usage-report/send-to-me", 405),
    ("POST", "/admin/usage-report/recipients", 405),
    (
        "GET",
        "/admin/usage-report/recipients/someone@example.com",
        405,
    ),
    ("TRACE", "/admin/usage-report", 405),
    ("TRACE", "/admin/usage-report/recipients", 405),
];

/// Paths neither section mounts: the outer router's 404 for every caller.
const USAGE_REPORT_UNMOUNTED: &[&str] = &[
    "/admin/usage-report/",
    "/admin/usage-report/nothing-here",
    "/admin/usage-report/recipients/a@b.c/deeper",
    "/admin/usage-reportx",
];

async fn usage_report_sweep(router: &Router, who: &str, staff: bool) {
    for (m, p, want) in USAGE_REPORT_ROUTES {
        let method = Method::from_bytes(m.as_bytes()).unwrap();
        let want = if staff { *want } else { 403 };
        let got = status(router, &method, p).await;
        assert_eq!(got.as_u16(), want, "{who} {m} {p}: got {got}");
    }
    for p in USAGE_REPORT_UNMOUNTED {
        let got = status(router, &Method::GET, p).await;
        assert_eq!(got, StatusCode::NOT_FOUND, "{who} GET {p}");
    }
}

/// The served router mounts the usage report inside `/api/admin` behind the
/// layers `admin::router` gave it in `oxy-app` — the staff door, the refusal
/// while acting, and `PlatformOperate` / `PlatformGrants` — answering what
/// `main` answered before the move ([`USAGE_REPORT_ROUTES`]). The App
/// Operator is the capability layers' witness: the door admits any standing,
/// so without a capability on each section it would reach the handlers.
/// Which section holds which capability is pinned in `oxy-api-billing`.
#[tokio::test]
async fn the_admin_seam_mounts_the_usage_report_behind_the_staff_layers() {
    if db_unavailable() {
        return;
    }
    use oxy_app::surface::roles::{RouteRole, classify};
    use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};

    let router = served(ServeMode::Cloud).await;
    for (m, p, _) in USAGE_REPORT_ROUTES {
        assert_eq!(
            classify(m, &format!("/api{p}")),
            RouteRole::FleetOk,
            "{m} {p} is Postgres only and must stay FleetOk"
        );
    }

    // A run that panicked mid-way leaves its grant behind, which would make
    // the guest staff below; these tests are serial (`serial-db`).
    let db = oxy::database::client::establish_connection()
        .await
        .expect("database");
    entity::app_admins::Entity::delete_many()
        .filter(entity::app_admins::Column::Email.eq(oxy_auth::user::LOCAL_GUEST_EMAIL))
        .exec(&db)
        .await
        .expect("clear a stale guest grant");
    oxy_server_authz::globals::invalidate_admin_cache();

    usage_report_sweep(&router, "non-staff", false).await;
    let owner = OwnerEnv::set(oxy_auth::user::LOCAL_GUEST_EMAIL);
    usage_report_sweep(&router, "owner", true).await;

    let (got, me) = get_json(&router, "/user").await;
    assert_eq!(got, StatusCode::OK, "GET /user: {me}");
    let user_id: uuid::Uuid = me["id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("GET /user carries no id: {me}"));
    let now = chrono::Utc::now();
    let org = |name: &'static str| {
        let id = uuid::Uuid::new_v4();
        entity::organizations::ActiveModel {
            id: ActiveValue::Set(id),
            name: ActiveValue::Set(name.into()),
            slug: ActiveValue::Set(format!("usage-report-{}", id.simple())),
            ..Default::default()
        }
    };

    // Acting as a tenant closes the console, the usage report included.
    let acting = org("Usage report acting")
        .insert(&db)
        .await
        .expect("seed the org");
    let session = entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        actor_user_id: ActiveValue::Set(user_id),
        actor_email: ActiveValue::Set(oxy_auth::user::LOCAL_GUEST_EMAIL.into()),
        org_id: ActiveValue::Set(acting.id),
        reason: ActiveValue::Set("served-router usage report".into()),
        started_at: ActiveValue::Set(now.into()),
        // Short, so a panic below leaves the guest acting for two minutes at most.
        expires_at: ActiveValue::Set((now + chrono::Duration::minutes(2)).into()),
        ended_at: ActiveValue::Set(None),
        token_id: ActiveValue::Set(None),
    }
    .insert(&db)
    .await
    .expect("seed a live assume session");
    let mut acting_answers = Vec::new();
    for (m, p, _) in USAGE_REPORT_ROUTES {
        let method = Method::from_bytes(m.as_bytes()).unwrap();
        acting_answers.push((*m, *p, status(&router, &method, p).await));
    }
    entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(session.id),
        ended_at: ActiveValue::Set(Some(chrono::Utc::now().into())),
        ..Default::default()
    }
    .update(&db)
    .await
    .expect("end the session");
    for (m, p, got) in acting_answers {
        assert_eq!(got, StatusCode::FORBIDDEN, "acting {m} {p}: got {got}");
    }
    drop(owner);

    // Staff bounded to one org: an App Operator holds neither capability, a
    // Global Admin both.
    let in_scope = org("Usage report scope")
        .insert(&db)
        .await
        .expect("the org");
    for role in ["app_operator", "global_admin"] {
        let grant = entity::app_admins::ActiveModel {
            id: ActiveValue::Set(uuid::Uuid::new_v4()),
            email: ActiveValue::Set(oxy_auth::user::LOCAL_GUEST_EMAIL.into()),
            granted_by: ActiveValue::Set(None),
            created_at: ActiveValue::Set(now.into()),
            role: ActiveValue::Set(role.into()),
            scope_all: ActiveValue::Set(false),
            updated_at: ActiveValue::Set(now.into()),
        }
        .insert(&db)
        .await
        .expect("seed the bounded grant");
        entity::app_admin_scope_orgs::ActiveModel {
            id: ActiveValue::Set(uuid::Uuid::new_v4()),
            app_admin_id: ActiveValue::Set(grant.id),
            org_id: ActiveValue::Set(in_scope.id),
            created_at: ActiveValue::Set(now.into()),
            created_by: ActiveValue::Set(None),
        }
        .insert(&db)
        .await
        .expect("bound it to the org");
        // The grant read is cached per process.
        oxy_server_authz::globals::invalidate_admin_cache();
        usage_report_sweep(&router, role, role == "global_admin").await;
        entity::app_admins::Entity::delete_by_id(grant.id)
            .exec(&db)
            .await
            .expect("remove the grant");
        oxy_server_authz::globals::invalidate_admin_cache();
    }
    for id in [acting.id, in_scope.id] {
        entity::organizations::Entity::delete_by_id(id)
            .exec(&db)
            .await
            .expect("remove the seeded org");
    }
}

/// The composition hands assume-role (`oxy-api-admin`'s since the admin split,
/// 2026-11-20) to the api seam, the root of the protected tree, where
/// `oxy-app`'s global tree merged it, and to no staff-console section or nest:
/// either would put the exit behind the staff door and the refusal while
/// acting. Undeclared, as it was: the FleetOk default places it.
#[tokio::test]
async fn the_composition_carries_assume_role_on_the_api_seam() {
    async fn trace(tree: &Router<oxy_app::server::router::AppState>, uri: &str) -> StatusCode {
        tree.clone()
            .with_state(oxy_app::surface::bare_app_state())
            .oneshot(
                Request::builder()
                    .method(Method::TRACE)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot")
            .status()
    }
    let seams = surface_seams();
    for uri in ["/assume", "/assume/current", "/assume/history"] {
        assert_eq!(
            trace(&seams.api.routes, uri).await,
            StatusCode::METHOD_NOT_ALLOWED,
            "the api seam mounts {uri}"
        );
        for section in &seams.admin {
            assert_eq!(
                trace(&section.routes, uri).await,
                StatusCode::NOT_FOUND,
                "{uri}"
            );
        }
        // A nest may hold a bare `/{id}` route (the compile console's
        // `/{revision_id}`), which answers any one-segment path, so a nest
        // mounts nothing for assume-role when the path answers exactly as an
        // unmounted sibling of it does.
        for nest in &seams.admin_nests {
            let control = format!("/nothing-mounts-this{}", &uri["/assume".len()..]);
            assert_eq!(
                trace(&nest.routes, uri).await,
                trace(&nest.routes, &control).await,
                "{uri} on the nest at /admin{}",
                nest.prefix
            );
        }
    }
    // No `/assumex` control here: the api seam carries other surfaces'
    // one-segment `/{param}` routes, which answer it. The assume router's own
    // exact paths are `oxy-api-admin`'s `every_assume_route_is_mounted`.
    assert!(
        !seams.api.decls.iter().any(|d| d.path.contains("assume")),
        "assume-role declares nothing; the FleetOk default places it"
    );
}

/// POST, GET or DELETE `path` with an optional JSON body; the status and the
/// body (JSON, or `Null` when empty).
async fn assume_call(
    router: &Router,
    method: Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder().method(method).uri(path);
    let body = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    let response = router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// The `admin.assume.*` audit actions written for `org`, oldest first.
async fn assume_audit_actions(db: &sea_orm::DatabaseConnection, org: uuid::Uuid) -> Vec<String> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
    entity::audit_events::Entity::find()
        .filter(entity::audit_events::Column::Action.starts_with("admin.assume."))
        .filter(entity::audit_events::Column::OrgId.eq(org))
        .order_by_asc(entity::audit_events::Column::Seq)
        .all(db)
        .await
        .expect("read the assume audit rows")
        .into_iter()
        .map(|e| e.action)
        .collect()
}

/// The served router mounts assume-role at `/api/assume*` OUTSIDE the staff
/// console, and acting closes the console, answering what `main` answered
/// before the move (read off the served router at 7cdd65f7ea):
///
/// * not staff: a blank reason is 400 (checked before any authority), a real
///   one 403 (`may_act_as`), the history 403 (`PlatformAudit`), and the
///   caller's live sessions an empty list;
/// * the Global Owner: a session of exactly 60 minutes, audited
///   `admin.assume.started` in the same transaction, listed by `/current`;
/// * while it is live, the console is 403 on three extracted sections
///   (`/admin/routing-manifest` and `/admin/feature-flags`, each the in-tree
///   probe until it moved to `oxy-api-admin` on 2026-11-21, and
///   `/admin/audit`) and a sibling nest (`/admin/internal-jobs/queue-stats`),
///   and `/api/assume` itself still answers: the exit is not behind the door
///   it locks. No in-tree console route is left to probe;
/// * ending it is 204, audited `admin.assume.ended`, and the console is back.
///
/// Dropping `oxy_api_admin::routes()` from the api seam turns every assume
/// call here 404; moving it behind the staff door turns the exit 403 while
/// acting; dropping the refusal while acting turns the console 200.
#[tokio::test]
async fn the_api_seam_mounts_assume_role_outside_the_staff_console() {
    if db_unavailable() {
        return;
    }
    use oxy_app::surface::roles::{RouteRole, classify};
    use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
    use serde_json::json;

    for (m, p) in [
        ("POST", "/api/assume"),
        ("DELETE", "/api/assume"),
        ("GET", "/api/assume/current"),
        ("GET", "/api/assume/history"),
    ] {
        assert_eq!(
            classify(m, p),
            RouteRole::FleetOk,
            "{m} {p} is Postgres only and must stay FleetOk"
        );
    }

    let router = served(ServeMode::Cloud).await;
    let db = oxy::database::client::establish_connection()
        .await
        .expect("database");
    let guest = oxy_auth::user::LOCAL_GUEST_EMAIL;
    // A run that panicked mid-way leaves a grant behind, which would make the
    // guest staff below; these tests are serial (`serial-db`).
    entity::app_admins::Entity::delete_many()
        .filter(entity::app_admins::Column::Email.eq(guest))
        .exec(&db)
        .await
        .expect("clear a stale guest grant");
    oxy_server_authz::globals::invalidate_admin_cache();
    // Likewise a live session the guest was left holding, which would make
    // `/assume/current` non-empty and the console refuse before acting.
    entity::admin_assume_sessions::Entity::update_many()
        .col_expr(
            entity::admin_assume_sessions::Column::EndedAt,
            sea_orm::sea_query::Expr::value(chrono::Utc::now().fixed_offset()),
        )
        .filter(entity::admin_assume_sessions::Column::ActorEmail.eq(guest))
        .filter(entity::admin_assume_sessions::Column::EndedAt.is_null())
        .exec(&db)
        .await
        .expect("end a stale guest session");

    let id = uuid::Uuid::new_v4();
    let org = entity::organizations::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set("Assume seam".into()),
        slug: ActiveValue::Set(format!("assume-seam-{}", id.simple())),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed the org");
    let start = |reason: &str| json!({ "org_id": org.id, "reason": reason });

    // Not staff.
    let (got, _) = assume_call(&router, Method::POST, "/assume", Some(start("  "))).await;
    assert_eq!(got, StatusCode::BAD_REQUEST, "a blank reason, not staff");
    let (got, _) = assume_call(&router, Method::POST, "/assume", Some(start("probe"))).await;
    assert_eq!(got, StatusCode::FORBIDDEN, "not staff may not act");
    let (got, _) = assume_call(&router, Method::GET, "/assume/history", None).await;
    assert_eq!(got, StatusCode::FORBIDDEN, "the history is an audit read");
    let (got, live) = assume_call(&router, Method::GET, "/assume/current", None).await;
    assert_eq!((got, live), (StatusCode::OK, json!([])));
    assert!(assume_audit_actions(&db, org.id).await.is_empty());

    // The Global Owner acts.
    let owner = OwnerEnv::set(guest);
    let (got, _) = assume_call(&router, Method::POST, "/assume", Some(start(""))).await;
    assert_eq!(got, StatusCode::BAD_REQUEST, "a blank reason, as the owner");
    let (got, session) = assume_call(
        &router,
        Method::POST,
        "/assume",
        Some(start(" seam probe ")),
    )
    .await;
    assert_eq!(got, StatusCode::OK, "{session}");
    assert_eq!(session["org_id"], json!(org.id));
    assert_eq!(session["reason"], "seam probe");
    let row = entity::admin_assume_sessions::Entity::find()
        .filter(entity::admin_assume_sessions::Column::OrgId.eq(org.id))
        .one(&db)
        .await
        .expect("read the session")
        .expect("the session was written");
    // `expires_at` is computed in the handler a moment before the insert stamps
    // `started_at`, so the span is a hair under the hour, never over it.
    let span = (row.expires_at - row.started_at).num_seconds();
    assert!(
        (3595..=3600).contains(&span),
        "a 60-minute session: {span}s"
    );
    assert_eq!(
        assume_audit_actions(&db, org.id).await,
        vec!["admin.assume.started"]
    );
    let (got, live) = assume_call(&router, Method::GET, "/assume/current", None).await;
    assert_eq!(got, StatusCode::OK);
    assert_eq!(live[0]["id"], json!(row.id), "{live}");

    // Acting closes the console: three sections and a nest.
    let mut acting = Vec::new();
    for p in [
        "/admin/routing-manifest",
        "/admin/feature-flags",
        "/admin/audit",
        "/admin/internal-jobs/queue-stats",
    ] {
        acting.push((p, status(&router, &Method::GET, p).await));
    }
    // The exit still answers (this is the end below).
    let (ended, _) = assume_call(&router, Method::DELETE, "/assume", None).await;
    let mut after = Vec::new();
    for p in [
        "/admin/routing-manifest",
        "/admin/feature-flags",
        "/admin/audit",
        "/admin/internal-jobs/queue-stats",
    ] {
        after.push((p, status(&router, &Method::GET, p).await));
    }
    drop(owner);
    let actions = assume_audit_actions(&db, org.id).await;
    entity::organizations::Entity::delete_by_id(org.id)
        .exec(&db)
        .await
        .expect("remove the seeded org");
    for (p, got) in acting {
        assert_eq!(got, StatusCode::FORBIDDEN, "acting GET {p}: got {got}");
    }
    assert_eq!(ended, StatusCode::NO_CONTENT, "the exit while acting");
    assert_eq!(actions, vec!["admin.assume.started", "admin.assume.ended"]);
    for (p, got) in after {
        assert_eq!(got, StatusCode::OK, "after ending GET {p}: got {got}");
    }
}

/// The Billing queue's routes (`oxy-api-billing`'s third section since admin
/// split PR 9, out of `oxy-app`'s `admin::billing`), as `(method, path, the
/// Global Owner's answer)`. Stripe is not configured, so every handler that
/// runs answers 503 from `billing_service`; a wrong verb is 405. Only routes
/// whose handler reads no body are sent their own verb, so a 503 is the
/// handler's and not an extractor's.
///
/// Every other caller this test sends — not staff, the owner while acting, a
/// Global Admin with every capability, an App Operator — is 403 on each of
/// these, the wrong verbs included: the strict owner guard `billing` carried
/// in-tree (`PlatformOwnerOnly` in `extracted_sections`).
const BILLING_QUEUE_ROUTES: &[(&str, &str, u16)] = &[
    ("GET", "/admin/orgs", 503),
    ("GET", "/admin/billing/prices", 503),
    (
        "GET",
        "/admin/orgs/00000000-0000-0000-0000-000000000000/billing/subscription",
        503,
    ),
    (
        "GET",
        "/admin/orgs/00000000-0000-0000-0000-000000000000/billing/checkout",
        503,
    ),
    (
        "POST",
        "/admin/orgs/00000000-0000-0000-0000-000000000000/billing/checkout/resend",
        503,
    ),
    (
        "POST",
        "/admin/orgs/00000000-0000-0000-0000-000000000000/billing/checkout/cancel",
        503,
    ),
    ("POST", "/admin/billing/prices", 405),
    (
        "GET",
        "/admin/orgs/00000000-0000-0000-0000-000000000000/billing/provision-subscription",
        405,
    ),
    (
        "GET",
        "/admin/orgs/00000000-0000-0000-0000-000000000000/billing/provision-checkout",
        405,
    ),
    (
        "GET",
        "/admin/orgs/00000000-0000-0000-0000-000000000000/billing/resync",
        405,
    ),
    ("TRACE", "/admin/billing/prices", 405),
];

/// Paths the Billing queue does not mount: the outer router's 404 for every
/// caller.
const BILLING_QUEUE_UNMOUNTED: &[&str] = &[
    "/admin/billing",
    "/admin/billing/prices/x",
    "/admin/orgs/00000000-0000-0000-0000-000000000000/billing/nothing-here",
];

async fn billing_queue_sweep(router: &Router, who: &str, owner: bool) {
    for (m, p, want) in BILLING_QUEUE_ROUTES {
        let method = Method::from_bytes(m.as_bytes()).unwrap();
        let want = if owner { *want } else { 403 };
        let got = status(router, &method, p).await;
        assert_eq!(got.as_u16(), want, "{who} {m} {p}: got {got}");
    }
    for p in BILLING_QUEUE_UNMOUNTED {
        let got = status(router, &Method::GET, p).await;
        assert_eq!(got, StatusCode::NOT_FOUND, "{who} GET {p}");
    }
}

/// The served router mounts the Billing queue inside `/api/admin` behind the
/// layers `admin::router` gave it in `oxy-app` — the staff door, the refusal
/// while acting, and the strict owner guard — answering as [`BILLING_QUEUE_ROUTES`]
/// says. The Global Admin is the owner guard's witness: it holds every
/// capability, so behind any capability guard it would reach the handlers.
#[tokio::test]
async fn the_admin_seam_mounts_the_billing_queue_behind_the_owner_guard() {
    if db_unavailable() {
        return;
    }
    use oxy_app::surface::roles::{RouteRole, classify};
    use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};

    let router = served(ServeMode::Cloud).await;
    for (m, p, _) in BILLING_QUEUE_ROUTES {
        assert_eq!(
            classify(m, &format!("/api{p}")),
            RouteRole::FleetOk,
            "{m} {p} is Postgres and Stripe only and must stay FleetOk"
        );
    }

    // A run that panicked mid-way leaves its grant behind, which would make
    // the guest staff below; these tests are serial (`serial-db`).
    let db = oxy::database::client::establish_connection()
        .await
        .expect("database");
    entity::app_admins::Entity::delete_many()
        .filter(entity::app_admins::Column::Email.eq(oxy_auth::user::LOCAL_GUEST_EMAIL))
        .exec(&db)
        .await
        .expect("clear a stale guest grant");
    oxy_server_authz::globals::invalidate_admin_cache();

    billing_queue_sweep(&router, "non-staff", false).await;
    assert_eq!(
        status(&router, &Method::DELETE, "/admin/orgs").await,
        StatusCode::FORBIDDEN,
        "non-staff DELETE /admin/orgs"
    );
    let owner = OwnerEnv::set(oxy_auth::user::LOCAL_GUEST_EMAIL);
    billing_queue_sweep(&router, "owner", true).await;
    // `/admin/orgs` is two sections' path: the queue's GET (owner-only) and
    // tenancy's POST (`PlatformOrgs`). A wrong verb takes the later-merged
    // router's 405 under its guard, which on `main` was tenancy's: the owner
    // and a Global Admin read 405, an App Operator 403 (below).
    assert_eq!(
        status(&router, &Method::DELETE, "/admin/orgs").await,
        StatusCode::METHOD_NOT_ALLOWED,
        "owner DELETE /admin/orgs"
    );

    let (got, me) = get_json(&router, "/user").await;
    assert_eq!(got, StatusCode::OK, "GET /user: {me}");
    let user_id: uuid::Uuid = me["id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("GET /user carries no id: {me}"));
    let now = chrono::Utc::now();

    // Acting as a tenant closes the console, the Billing queue included.
    let org_id = uuid::Uuid::new_v4();
    let acting = entity::organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set("Billing queue acting".into()),
        slug: ActiveValue::Set(format!("billing-queue-{}", org_id.simple())),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed the org");
    let session = entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        actor_user_id: ActiveValue::Set(user_id),
        actor_email: ActiveValue::Set(oxy_auth::user::LOCAL_GUEST_EMAIL.into()),
        org_id: ActiveValue::Set(acting.id),
        reason: ActiveValue::Set("served-router billing queue".into()),
        started_at: ActiveValue::Set(now.into()),
        // Short, so a panic below leaves the guest acting for two minutes at most.
        expires_at: ActiveValue::Set((now + chrono::Duration::minutes(2)).into()),
        ended_at: ActiveValue::Set(None),
        token_id: ActiveValue::Set(None),
    }
    .insert(&db)
    .await
    .expect("seed a live assume session");
    let mut acting_answers = Vec::new();
    for (m, p, _) in BILLING_QUEUE_ROUTES {
        let method = Method::from_bytes(m.as_bytes()).unwrap();
        acting_answers.push((*m, *p, status(&router, &method, p).await));
    }
    entity::admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(session.id),
        ended_at: ActiveValue::Set(Some(chrono::Utc::now().into())),
        ..Default::default()
    }
    .update(&db)
    .await
    .expect("end the session");
    for (m, p, got) in acting_answers {
        assert_eq!(got, StatusCode::FORBIDDEN, "acting {m} {p}: got {got}");
    }
    drop(owner);

    // Staff who are not the owner, unbounded: a Global Admin holds every
    // capability and an App Operator ships apps; the Billing queue is neither's.
    for role in ["app_operator", "global_admin"] {
        let grant = entity::app_admins::ActiveModel {
            id: ActiveValue::Set(uuid::Uuid::new_v4()),
            email: ActiveValue::Set(oxy_auth::user::LOCAL_GUEST_EMAIL.into()),
            granted_by: ActiveValue::Set(None),
            created_at: ActiveValue::Set(now.into()),
            role: ActiveValue::Set(role.into()),
            scope_all: ActiveValue::Set(true),
            updated_at: ActiveValue::Set(now.into()),
        }
        .insert(&db)
        .await
        .expect("seed the grant");
        // The grant read is cached per process.
        oxy_server_authz::globals::invalidate_admin_cache();
        billing_queue_sweep(&router, role, false).await;
        let want = if role == "global_admin" {
            StatusCode::METHOD_NOT_ALLOWED
        } else {
            StatusCode::FORBIDDEN
        };
        assert_eq!(
            status(&router, &Method::DELETE, "/admin/orgs").await,
            want,
            "{role} DELETE /admin/orgs: tenancy's `PlatformOrgs`, not the owner guard"
        );
        entity::app_admins::Entity::delete_by_id(grant.id)
            .exec(&db)
            .await
            .expect("remove the grant");
        oxy_server_authz::globals::invalidate_admin_cache();
    }
    entity::organizations::Entity::delete_by_id(acting.id)
        .exec(&db)
        .await
        .expect("remove the seeded org");
}

/// The tenant's billing routes (`oxy-api-billing`'s on the api seam since admin
/// split PR 9, out of `build_org_routes`), as the browser calls them.
fn tenant_billing_routes() -> Vec<(Method, String)> {
    vec![
        (Method::GET, format!("/orgs/{ORG}/billing")),
        (Method::GET, format!("/orgs/{ORG}/billing/status")),
        (Method::POST, format!("/orgs/{ORG}/billing/portal-session")),
        (Method::GET, format!("/orgs/{ORG}/billing/invoices")),
        (
            Method::GET,
            format!("/orgs/{ORG}/billing/checkout-sessions/cs_test"),
        ),
    ]
}

#[tokio::test]
async fn cloud_mode_mounts_the_tenant_billing_routes() {
    if db_unavailable() {
        return;
    }
    assert_cloud_mounts(tenant_billing_routes()).await;
    use oxy_app::surface::roles::{RouteRole, classify};
    for (m, p) in tenant_billing_routes() {
        let uri = format!(
            "/api{}",
            p.replace(ORG, "00000000-0000-0000-0000-000000000000")
        );
        assert_eq!(
            classify(m.as_str(), &uri),
            RouteRole::FleetOk,
            "{m} {uri}: the Stripe customer and subscription rows, FleetOk"
        );
    }
}

#[tokio::test]
async fn local_mode_drops_the_tenant_billing_routes() {
    if db_unavailable() {
        return;
    }
    assert_local_drops(tenant_billing_routes()).await;
}

/// Stripe's webhook (`oxy-api-billing`'s on the public seam since admin split
/// PR 9, out of `build_public_routes`), served in both modes as it was, and
/// declared FleetOk. TRACE, which it does not take, is 405 where it is mounted
/// and the outer 404 where it is not, with no credential and without reaching
/// the handler; a POST would not tell, since unconfigured Stripe answers 404.
#[tokio::test]
async fn both_modes_mount_the_stripe_webhook() {
    if db_unavailable() {
        return;
    }
    use oxy_app::surface::roles::{RouteRole, classify};
    for mode in [ServeMode::Cloud, ServeMode::Local] {
        let router = served(mode).await;
        assert_eq!(
            status(&router, &Method::TRACE, "/webhooks/stripe").await,
            StatusCode::METHOD_NOT_ALLOWED,
            "{mode:?} serves the Stripe webhook"
        );
        assert_eq!(
            status(&router, &Method::TRACE, "/webhooks/stripe-nothing").await,
            StatusCode::NOT_FOUND,
            "{mode:?} control"
        );
    }
    assert_eq!(classify("POST", "/api/webhooks/stripe"), RouteRole::FleetOk);
}

/// The OAuth callbacks (`oxy-api-workspace-config`'s on the public seam since
/// 2026-11-21, out of `build_public_routes`): QuickBooks' registered
/// `/quickbooks/oauth/callback` and the uniform `/oauth/{provider}/callback`,
/// served in both modes as they were, outside the auth stack, and declared
/// FleetOk. TRACE, which neither takes, is 405 where they are mounted and the
/// outer 404 where they are not, with no credential and without reaching the
/// handler. Moved here from `oxy-app`'s two router canaries
/// (`local_router_mounts_the_public_tree` and `local_mode_router`'s
/// `local_router_has_its_public_tree`), which probe the public seam now.
#[tokio::test]
async fn both_modes_mount_the_oauth_callbacks() {
    use oxy_app::surface::roles::{RouteRole, classify};
    let seams = surface_seams();
    for path in ["/quickbooks/oauth/callback", "/oauth/{provider}/callback"] {
        assert!(
            seams.public.decls.iter().any(|d| d.method == "*"
                && d.path == path
                && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
            "the public seam declares {path} FleetOk"
        );
    }
    if db_unavailable() {
        return;
    }
    for mode in [ServeMode::Cloud, ServeMode::Local] {
        let router = served(mode).await;
        for path in ["/quickbooks/oauth/callback", "/oauth/google/callback"] {
            assert_eq!(
                status(&router, &Method::TRACE, path).await,
                StatusCode::METHOD_NOT_ALLOWED,
                "{mode:?} serves {path}"
            );
        }
        assert_eq!(
            status(&router, &Method::TRACE, "/oauth/google/nothing-mounts-this").await,
            StatusCode::NOT_FOUND,
            "{mode:?} control"
        );
    }
    assert_eq!(
        classify("GET", "/api/quickbooks/oauth/callback"),
        RouteRole::FleetOk
    );
    assert_eq!(
        classify("GET", "/api/oauth/google/callback"),
        RouteRole::FleetOk
    );
}

/// The org member's flag read (`oxy-api-admin`'s on the api seam since
/// 2026-11-21, out of `build_org_routes`), as the shell calls it.
fn org_feature_flag_routes() -> Vec<(Method, String)> {
    vec![(Method::GET, format!("/orgs/{ORG}/feature-flags"))]
}

#[tokio::test]
async fn cloud_mode_mounts_the_org_feature_flag_read() {
    if db_unavailable() {
        return;
    }
    assert_cloud_mounts(org_feature_flag_routes()).await;
    use oxy_app::surface::roles::{RouteRole, classify};
    assert_eq!(
        classify(
            "GET",
            "/api/orgs/00000000-0000-0000-0000-000000000000/feature-flags"
        ),
        RouteRole::FleetOk,
        "the flag cache, loaded from Postgres on every replica: FleetOk"
    );
}

#[tokio::test]
async fn local_mode_drops_the_org_feature_flag_read() {
    if db_unavailable() {
        return;
    }
    assert_local_drops(org_feature_flag_routes()).await;
}

/// The composition hands the feature-flag routes to the seams `oxy-app`
/// mounted them from: the staff console's half to `admin` (a section behind
/// `PlatformOperate`, as `admin::router` merged it), and the org member's
/// read to `api` (behind `org_middleware`, whose `Path` answers an
/// unparseable org id 400 before any database), declared FleetOk as
/// `route_fleet` declared it. No database.
#[tokio::test]
async fn the_composition_carries_the_feature_flag_routes_on_two_seams() {
    async fn trace(tree: &Router<oxy_app::server::router::AppState>, uri: &str) -> StatusCode {
        tree.clone()
            .with_state(oxy_app::surface::bare_app_state())
            .oneshot(
                Request::builder()
                    .method(Method::TRACE)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot")
            .status()
    }
    let seams = surface_seams();
    let mut console = Vec::new();
    for section in &seams.admin {
        if trace(&section.routes, "/feature-flags").await == StatusCode::METHOD_NOT_ALLOWED {
            console.push(section.capability);
        }
    }
    assert_eq!(
        console,
        vec![oxy_app::surface::Action::PlatformOperate],
        "one section mounts the flag console, behind PlatformOperate"
    );
    assert_eq!(
        trace(&seams.api.routes, "/orgs/not-a-uuid/feature-flags").await,
        StatusCode::BAD_REQUEST,
        "the api seam mounts the org flag read behind org_middleware"
    );
    assert!(
        seams.api.decls.iter().any(|d| d.method == "*"
            && d.path == "/orgs/{org_id}/feature-flags"
            && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
        "the api seam declares /orgs/{{org_id}}/feature-flags FleetOk"
    );
}

/// The composition hands the route-role table (`/routing-manifest`,
/// `oxy-api-admin`'s since 2026-11-21) to the admin seam as exactly one
/// section, behind `PlatformOperate`, the `route_layer` `admin::router` gave
/// it in-tree. No database. (Only the sections are probed: a nest's tree is
/// relative to its own `/admin{prefix}` and the api seam's to the protected
/// root, so a `/{id}`-shaped route in either matches the bare path and the
/// probe would prove nothing; the served router's answer is pinned by the
/// assume-role test's `/admin/routing-manifest` probes.)
#[tokio::test]
async fn the_composition_carries_the_routing_manifest_as_one_section() {
    async fn trace(tree: &Router<oxy_app::server::router::AppState>, uri: &str) -> StatusCode {
        tree.clone()
            .with_state(oxy_app::surface::bare_app_state())
            .oneshot(
                Request::builder()
                    .method(Method::TRACE)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot")
            .status()
    }
    let seams = surface_seams();
    let mut console = Vec::new();
    for section in &seams.admin {
        if trace(&section.routes, "/routing-manifest").await == StatusCode::METHOD_NOT_ALLOWED {
            console.push((section.capability, section.decls.len()));
        }
    }
    assert_eq!(
        console,
        vec![(oxy_app::surface::Action::PlatformOperate, 0)],
        "one section mounts the route-role table, behind PlatformOperate, declaring nothing"
    );
}

/// The composition hands billing's three shares to the seams `oxy-app` mounted
/// them from: the Billing queue to `admin` (one section, naming
/// `PlatformOwnerOnly`), the tenant's routes to `api` (behind
/// `org_middleware`, whose `Path` answers an unparseable org id 400 before any
/// database), and the webhook to `public`, each with the declarations
/// `nest_all` and `route_fleet` made in-tree. No database.
#[tokio::test]
async fn the_composition_carries_billing_on_its_three_seams() {
    async fn trace(tree: &Router<oxy_app::server::router::AppState>, uri: &str) -> StatusCode {
        tree.clone()
            .with_state(oxy_app::surface::bare_app_state())
            .oneshot(
                Request::builder()
                    .method(Method::TRACE)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot")
            .status()
    }
    let seams = surface_seams();
    let owner_only: Vec<_> = seams
        .admin
        .iter()
        .filter(|s| s.capability == oxy_app::surface::Action::PlatformOwnerOnly)
        .collect();
    assert_eq!(
        owner_only.len(),
        1,
        "one owner-only section: the Billing queue"
    );
    assert_eq!(
        trace(&owner_only[0].routes, "/billing/prices").await,
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        trace(&seams.api.routes, "/orgs/not-a-uuid/billing/status").await,
        StatusCode::BAD_REQUEST,
        "the api seam mounts the tenant's billing behind org_middleware"
    );
    assert_eq!(
        trace(&seams.public.routes, "/webhooks/stripe").await,
        StatusCode::METHOD_NOT_ALLOWED,
        "the public seam mounts the webhook"
    );
    for path in ["/orgs/{org_id}/billing", "/orgs/{org_id}/billing/{*rest}"] {
        assert!(
            seams.api.decls.iter().any(|d| d.method == "*"
                && d.path == path
                && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
            "the api seam declares {path} FleetOk"
        );
    }
    assert!(
        seams.public.decls.iter().any(|d| d.method == "*"
            && d.path == "/webhooks/stripe"
            && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
        "the public seam declares the webhook FleetOk"
    );
}
