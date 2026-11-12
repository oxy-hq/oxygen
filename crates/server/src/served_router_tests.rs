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
    for (method, path, body) in world_model_routes() {
        let got = status_with(&router, &method, &format!("/{ws}{path}"), None, body).await;
        assert_ne!(
            got,
            StatusCode::NOT_FOUND,
            "{method} /{{ws}}{path} must be served, got {got}"
        );
    }
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
    // The first guest request creates the guest user the key will belong to.
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
    let key = oxy_auth::ApiKeyService::create_api_key(
        &db,
        oxy_auth::CreateApiKeyParams {
            user_id: guest.id,
            name: "served-router".into(),
            expires_at: None,
            project_id: ws,
        },
        &oxy_auth::ApiKeyConfig::default(),
    )
    .await
    .expect("mint a key")
    .key;

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
