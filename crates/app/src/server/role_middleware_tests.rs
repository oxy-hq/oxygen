use super::*;
use axum::Router;
use axum::body::to_bytes;
use axum::http::Request as HttpRequest;
use axum::middleware;
use axum::routing::{get, post};
use tower::ServiceExt;

/// The live declarations plus one IdeOnly probe. No live route is IdeOnly any
/// more (the airway backfill drive was the last, 2026-11-14), and these tests
/// are about what the middleware does with one, so they bring their own.
fn install_roles() {
    crate::server::role_manifest::install_route_declarations_for_tests_with(vec![(
        "POST",
        "/{workspace_id}/ide-probe".to_string(),
        RouteRole::IdeOnly,
    )]);
}

fn nested_router() -> Router {
    let workspace_routes = Router::new()
        .route("/ide-probe", post(|| async { "should not reach" }))
        .route("/threads", get(|| async { "threads ok" }));
    let api_routes = Router::new().nest("/{workspace_id}", workspace_routes);
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .nest("/api", api_routes)
        .layer(middleware::from_fn(enforce_role))
}

#[tokio::test]
async fn ide_only_route_on_serve_replica_returns_421_through_nest() {
    install_roles();
    unsafe { std::env::set_var("OXY_ROLE", "serve") };
    crate::server::role_manifest::init_process_role_from_env();

    let resp = nested_router()
        .oneshot(
            HttpRequest::post("/api/some-uuid/ide-probe")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::MISDIRECTED_REQUEST);
    assert_eq!(resp.headers().get(HEADER_REQUIRED_ROLE).unwrap(), "ide");
    assert!(
        resp.headers()
            .get(HEADER_SERVED_BY)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("serve@")
    );

    unsafe { std::env::remove_var("OXY_ROLE") };
}

#[tokio::test]
async fn worker_with_upstream_does_not_forward_ide_route() {
    install_roles();
    unsafe {
        std::env::set_var("OXY_ROLE", "worker");
        std::env::set_var("OXY_IDE_UPSTREAM", "http://ide.invalid:80");
    }
    crate::server::role_manifest::init_process_role_from_env();

    let resp = nested_router()
        .oneshot(
            HttpRequest::post("/api/some-uuid/ide-probe")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::MISDIRECTED_REQUEST);
    unsafe {
        std::env::remove_var("OXY_ROLE");
        std::env::remove_var("OXY_IDE_UPSTREAM");
    }
}

#[tokio::test]
async fn already_forwarded_ide_route_on_serve_breaks_loop() {
    install_roles();
    unsafe {
        std::env::set_var("OXY_ROLE", "serve");
        std::env::set_var("OXY_IDE_UPSTREAM", "http://ide.invalid:80");
    }
    crate::server::role_manifest::init_process_role_from_env();

    let resp = nested_router()
        .oneshot(
            HttpRequest::post("/api/some-uuid/ide-probe")
                .header("x-oxy-forwarded-by", "serve")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::MISDIRECTED_REQUEST);
    unsafe {
        std::env::remove_var("OXY_ROLE");
        std::env::remove_var("OXY_IDE_UPSTREAM");
    }
}

#[tokio::test]
async fn fleet_ok_route_on_serve_replica_passes_through_nest() {
    install_roles();
    unsafe { std::env::set_var("OXY_ROLE", "serve") };
    crate::server::role_manifest::init_process_role_from_env();

    let resp = nested_router()
        .oneshot(
            HttpRequest::get("/api/some-uuid/threads")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&body[..], b"threads ok");

    unsafe { std::env::remove_var("OXY_ROLE") };
}

/// A `FleetOk` handler that forwarded from inside — `invocation_placement`,
/// for a workspace the replica cannot serve — hands back the Factory's answer
/// already stamped `forwarded-via`. The middleware's own stamp must then leave
/// the Factory's `served-by` alone: the pair is the only trace of the hop.
#[tokio::test]
async fn a_response_a_handler_relayed_from_the_factory_keeps_the_factorys_served_by() {
    install_roles();
    unsafe { std::env::set_var("OXY_ROLE", "serve") };
    crate::server::role_manifest::init_process_role_from_env();

    let relaying = Router::new()
        .route(
            "/api/{workspace_id}/threads",
            get(|| async {
                let mut resp = "from the factory".into_response();
                resp.headers_mut()
                    .insert(HEADER_SERVED_BY, HeaderValue::from_static("ide@factory#1"));
                stamp_forwarded_via(resp, Role::Serve)
            }),
        )
        .layer(middleware::from_fn(enforce_role));
    let resp = relaying
        .oneshot(
            HttpRequest::get("/api/some-uuid/threads")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(HEADER_SERVED_BY).unwrap(),
        "ide@factory#1",
        "the Factory answered; the replica's stamp must not claim it did"
    );
    assert!(
        resp.headers()
            .get(HEADER_FORWARDED_VIA)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("serve@"),
        "the replica that relayed it is named"
    );

    unsafe { std::env::remove_var("OXY_ROLE") };
}

#[tokio::test]
async fn health_probe_passes_on_every_role_including_worker() {
    install_roles();
    for role in ["ide", "serve", "worker"] {
        unsafe { std::env::set_var("OXY_ROLE", role) };
        crate::server::role_manifest::init_process_role_from_env();
        let resp = nested_router()
            .oneshot(HttpRequest::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "health probe failed under OXY_ROLE={role}"
        );
    }
    unsafe { std::env::remove_var("OXY_ROLE") };
}

#[tokio::test]
async fn all_role_accepts_everything() {
    install_roles();
    unsafe { std::env::remove_var("OXY_ROLE") };
    crate::server::role_manifest::init_process_role_from_env();

    let resp = nested_router()
        .oneshot(
            HttpRequest::post("/api/some-uuid/ide-probe")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers()
            .get(HEADER_SERVED_BY)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("all@")
    );
}

/// `?branch=` escalates nothing: there is no working copy to read, so a fleet
/// route naming a branch is served on the replica it lands on and nothing is
/// forwarded to a Factory, even with an ide upstream configured.
mod branch_is_served_locally {
    use super::*;

    const WS: &str = "7a4e3c1e-0b5f-4c36-9d55-3f1c2b8a9e01";

    fn router() -> Router {
        let workspace_routes = Router::new()
            .route("/threads", get(|| async { "served here" }))
            .route("/compile/staging", post(|| async { "staged" }));
        let api_routes = Router::new().nest("/{workspace_id}", workspace_routes);
        Router::new()
            .nest("/api", api_routes)
            .layer(middleware::from_fn(enforce_role))
    }

    async fn call(method: &str, uri: &str) -> (StatusCode, axum::http::HeaderMap, String) {
        let req = HttpRequest::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let resp = router().oneshot(req).await.unwrap();
        let (status, headers) = (resp.status(), resp.headers().clone());
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        (status, headers, String::from_utf8_lossy(&body).into_owned())
    }

    fn serve_replica_with_an_ide_upstream() {
        install_roles();
        unsafe {
            std::env::set_var("OXY_ROLE", "serve");
            std::env::set_var("OXY_IDE_UPSTREAM", "http://ide.invalid:80");
        }
        crate::server::role_manifest::init_process_role_from_env();
    }

    fn restore() {
        unsafe {
            std::env::remove_var("OXY_ROLE");
            std::env::remove_var("OXY_IDE_UPSTREAM");
        }
    }

    #[tokio::test]
    async fn a_branch_on_a_fleet_route_is_served_here() {
        serve_replica_with_an_ide_upstream();
        for uri in [
            format!("/api/{WS}/threads?branch=feat%2Fx"),
            format!("/api/{WS}/threads?branch=main"),
        ] {
            let (status, headers, body) = call("GET", &uri).await;
            assert_eq!(status, StatusCode::OK, "{uri}: {body}");
            assert_eq!(body, "served here");
            assert!(
                headers.get(HEADER_FORWARDED_VIA).is_none(),
                "{uri} was forwarded to a Factory"
            );
        }
        restore();
    }

    #[tokio::test]
    async fn compiling_a_branch_is_served_by_the_replica_it_lands_on() {
        serve_replica_with_an_ide_upstream();
        let (status, headers, body) = call(
            "POST",
            &format!("/api/{WS}/compile/staging?branch=feat%2Fx"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, "staged");
        assert!(headers.get(HEADER_FORWARDED_VIA).is_none());
        restore();
    }
}

/// The `/external/api` surface layers the middleware INSIDE its `nest`, so the
/// request it sees has the prefix stripped. The prefix is put back for the
/// classification (an IdeOnly agentic route is still IdeOnly behind the
/// prefix) and for the forward (the ide must receive what the client sent);
/// `?branch=` on its own moves nothing, as everywhere since the IDE went.
mod external_surface {
    use super::*;

    const WS: &str = "7a4e3c1e-0b5f-4c36-9d55-3f1c2b8a9e01";

    fn external_router() -> Router {
        let workspace_routes = Router::new()
            .route("/ide-probe", post(|| async { "should not reach" }))
            .route("/sql/query", post(|| async { "served here" }));
        Router::new().nest(
            "/external/api",
            Router::new()
                .nest("/{workspace_id}", workspace_routes)
                .layer(middleware::from_fn(enforce_role_external)),
        )
    }

    async fn call(method: &str, uri: &str) -> (StatusCode, String) {
        let req = HttpRequest::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let resp = external_router().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    fn restore() {
        unsafe {
            std::env::remove_var("OXY_ROLE");
            std::env::remove_var("OXY_IDE_UPSTREAM");
        }
    }

    /// An IdeOnly route behind the external prefix is classified by its
    /// `/api/...` declaration, not let through because the stripped
    /// `/{ws}/ide-probe` matches nothing and would default
    /// to FleetOk. With no upstream the refusal names the path the client sent.
    #[tokio::test]
    async fn an_ide_only_route_is_classified_by_its_full_path() {
        install_roles();
        unsafe {
            std::env::set_var("OXY_ROLE", "serve");
            std::env::remove_var("OXY_IDE_UPSTREAM");
        }
        crate::server::role_manifest::init_process_role_from_env();

        let path = format!("/external/api/{WS}/ide-probe");
        let (status, body) = call("POST", &path).await;

        assert_eq!(status, StatusCode::MISDIRECTED_REQUEST, "{body}");
        assert!(
            body.contains(&path),
            "the refusal names the path the client sent: {body}"
        );
        restore();
    }

    /// The ide receives the path and query the client sent, prefix included —
    /// not the stripped remainder, which its static fallback would answer 405.
    #[tokio::test]
    async fn an_ide_only_route_reaches_the_ide_with_the_external_prefix() {
        install_roles();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ide = Router::new().fallback(|uri: axum::http::Uri| async move { uri.to_string() });
        tokio::spawn(async move { axum::serve(listener, ide).await.unwrap() });
        unsafe {
            std::env::set_var("OXY_ROLE", "serve");
            std::env::set_var("OXY_IDE_UPSTREAM", format!("http://{addr}"));
            std::env::set_var("NO_PROXY", "127.0.0.1");
        }
        crate::server::role_manifest::init_process_role_from_env();

        let path = format!("/external/api/{WS}/ide-probe?scope=stale");
        let (status, body) = call("POST", &path).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, path, "the ide must route what the client sent");
        restore();
    }

    /// A FleetOk route stays on this replica — with or without `?branch=`,
    /// which escalates nothing now that there is no working copy to read.
    #[tokio::test]
    async fn a_fleet_route_is_served_here_with_or_without_a_branch() {
        install_roles();
        unsafe {
            std::env::set_var("OXY_ROLE", "serve");
            std::env::remove_var("OXY_IDE_UPSTREAM");
        }
        crate::server::role_manifest::init_process_role_from_env();

        for uri in [
            format!("/external/api/{WS}/sql/query"),
            format!("/external/api/{WS}/sql/query?branch=main"),
        ] {
            let (status, body) = call("POST", &uri).await;
            assert_eq!(status, StatusCode::OK, "{uri}: {body}");
            assert_eq!(body, "served here", "{uri}");
        }
        restore();
    }
}
