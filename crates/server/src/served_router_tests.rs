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
        "/traces/clusters/map",
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
        "/secrets",
        "/secrets/bulk",
        "/secrets/7c9e6679-7425-40de-944b-e07fc1f90ae7",
        "/secrets/7c9e6679-7425-40de-944b-e07fc1f90ae7/value",
    ]
}

/// The workspace seam carries the workspace's databases and secrets into the
/// `/{workspace_id}` tree, with the nine FleetOk declarations `oxy-app`'s
/// `route_fleet` used to write; and a real GET of the databases list, through
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
/// beside the OAuth connects `oxy-app` still nests there, so both halves of
/// the prefix must answer. A real GET reaches the handler and answers from the
/// empty workspace's config: no Looker integration, `[]`.
#[tokio::test]
async fn the_workspace_tree_mounts_the_looker_listing_beside_the_oauth_connects() {
    let decls = surface_seams().workspace.decls;
    assert!(
        decls.iter().any(|d| d.method == "*"
            && d.path == "/integrations/looker"
            && d.role == oxy_shared::fleet_role::RouteRole::FleetOk),
        "the workspace seam must carry `* /integrations/looker` as FleetOk"
    );
    if db_unavailable() {
        return;
    }
    let (router, _dir) = served_over_an_empty_workspace().await;
    assert_workspace_paths_mounted(
        &router,
        &[
            "/integrations/looker",
            // `oxy-app`'s half of the same prefix, which must survive the merge.
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
        "/builder-availability",
        "/results/files/65c121b2-0000-0000-0000-000000000000.parquet",
    ]
}

/// The conversation surface (Wave B, 2026-11-16) rides the WORKSPACE seam,
/// inside the `/{workspace_id}` tree where `build_workspace_routes` had it,
/// with the ten FleetOk declarations `route_fleet` wrote. Real reads, through
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
        "/builder-availability",
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
