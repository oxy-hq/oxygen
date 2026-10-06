use super::*;
use axum::Router;
use axum::body::to_bytes;
use axum::http::Request as HttpRequest;
use axum::middleware;
use axum::routing::{get, post};
use tower::ServiceExt;

fn install_roles() {
    crate::server::role_manifest::install_route_declarations_for_tests();
}

fn nested_router() -> Router {
    let workspace_routes = Router::new()
        .route("/compile", post(|| async { "should not reach" }))
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
            HttpRequest::post("/api/some-uuid/compile")
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
            HttpRequest::post("/api/some-uuid/compile")
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
            HttpRequest::post("/api/some-uuid/compile")
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
            HttpRequest::post("/api/some-uuid/compile")
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

mod branch_escalation {
    use super::*;

    #[test]
    fn a_branch_escalates_a_fleet_route() {
        assert_eq!(
            escalate_for_branch(RouteRole::FleetOk, Some("branch=feature-x")),
            RouteRole::IdeOnly
        );
        assert_eq!(
            escalate_for_branch(RouteRole::FleetOk, Some("limit=10&branch=feature-x&x=1")),
            RouteRole::IdeOnly
        );
    }

    #[test]
    fn nothing_de_escalates_an_ide_route() {
        for query in [None, Some(""), Some("branch="), Some("branch=main")] {
            assert_eq!(
                escalate_for_branch(RouteRole::IdeOnly, query),
                RouteRole::IdeOnly,
                "query {query:?} must not relax an IdeOnly route"
            );
        }
    }

    #[test]
    fn an_empty_or_absent_branch_does_not_escalate() {
        for query in [None, Some(""), Some("branch="), Some("limit=10")] {
            assert_eq!(
                escalate_for_branch(RouteRole::FleetOk, query),
                RouteRole::FleetOk,
                "query {query:?} must not pin a fleet route to the ide"
            );
        }
    }

    #[test]
    fn a_lookalike_parameter_does_not_escalate() {
        for query in ["default_branch=x", "branchy=x", "base_branch=main"] {
            assert_eq!(
                escalate_for_branch(RouteRole::FleetOk, Some(query)),
                RouteRole::FleetOk,
                "{query} is not a branch hint"
            );
        }
    }

    /// The airway control routes are `FleetOk`, and a replica cannot read a
    /// draft branch — it would answer from the promoted revision and say
    /// nothing. So the same route, asked about a branch, must still go to the
    /// ide. Composed from the real classification and the escalation, which is
    /// what `enforce_role` does with them.
    #[test]
    fn an_airway_request_naming_a_branch_still_goes_to_the_ide() {
        install_roles();
        let ws = "d9830be4-c6a4";
        for (method, path) in [
            ("POST", format!("/api/{ws}/agentic-airway/runs")),
            ("POST", format!("/api/{ws}/agentic-airway/backfill")),
            ("POST", format!("/api/{ws}/agentic-airway/reset-cursors")),
            ("POST", format!("/api/{ws}/agentic-airway/reset-schema")),
        ] {
            let classified = classify(method, &path);
            assert_eq!(
                classified,
                RouteRole::FleetOk,
                "{method} {path}: precondition — served by any replica without a branch"
            );
            assert_eq!(
                escalate_for_branch(classified, Some("branch=feature-x")),
                RouteRole::IdeOnly,
                "{method} {path}?branch= must reach the node that holds the draft"
            );
        }
    }

    /// The world-model routes read the compiled revision, and the IDE page
    /// that calls them names its branch on every request. A replica would
    /// answer a draft branch from the promoted revision and say nothing, so
    /// a request that names one still goes to the ide.
    #[test]
    fn a_world_model_request_naming_a_branch_still_goes_to_the_ide() {
        install_roles();
        let ws = "d9830be4-c6a4";
        for (method, path) in [
            ("GET", format!("/api/{ws}/semantic/world-model")),
            ("GET", format!("/api/{ws}/semantic/world-model/instances")),
            (
                "POST",
                format!("/api/{ws}/semantic/world-model/filter-counts"),
            ),
        ] {
            let classified = classify(method, &path);
            assert_eq!(
                classified,
                RouteRole::FleetOk,
                "{method} {path}: precondition — served by any replica without a branch"
            );
            assert_eq!(
                escalate_for_branch(classified, Some("branch=feature-x")),
                RouteRole::IdeOnly,
                "{method} {path}?branch= must reach the node that holds the draft"
            );
        }
    }
}

/// Workspace previews on a serve replica: a `?branch=` fleet route carrying the
/// preview header stays on the fleet (the workspace middleware finishes the
/// decision), one without it escalates exactly as before, and the previews API
/// — whose `?branch=` names a preview, not a working copy — never escalates.
mod preview_header {
    use super::*;
    use crate::server::previews::pin::DeferredBranchEscalation;

    const WS: &str = "d9830be4-c6a4-4f89-11d3-9a0c0305e82c";

    fn router() -> Router {
        async fn threads(req: axum::extract::Request) -> String {
            let deferred = req.extensions().get::<DeferredBranchEscalation>().is_some();
            format!("served here, deferred={deferred}")
        }
        let workspace_routes = Router::new()
            .route("/threads", get(threads))
            .route(
                "/previews",
                get(|| async { "list" }).delete(|| async { "deleted" }),
            )
            .route("/previews/checks", get(|| async { "checks" }))
            .route("/previews/runs", get(|| async { "runs" }));
        let api_routes = Router::new().nest("/{workspace_id}", workspace_routes);
        Router::new()
            .nest("/api", api_routes)
            .layer(middleware::from_fn(enforce_role))
    }

    async fn call(method: &str, uri: &str, header: Option<&str>) -> (StatusCode, String) {
        let mut req = HttpRequest::builder().method(method).uri(uri);
        if let Some(h) = header {
            req = req.header(crate::server::previews::pin::REQUEST_HEADER, h);
        }
        let resp = router()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    fn as_serve() {
        install_roles();
        unsafe { std::env::set_var("OXY_ROLE", "serve") };
        crate::server::role_manifest::init_process_role_from_env();
    }

    #[tokio::test]
    async fn without_the_header_a_branch_escalates_as_before() {
        as_serve();
        let (status, _) = call("GET", &format!("/api/{WS}/threads?branch=feat%2Fx"), None).await;
        // No ide upstream in this process, so escalation surfaces as the 421.
        assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
    }

    #[tokio::test]
    async fn with_the_header_a_fleet_route_stays_and_the_decision_is_deferred() {
        as_serve();
        let (status, body) = call(
            "GET",
            &format!("/api/{WS}/threads?branch=feat%2Fx"),
            Some("5f0e6c1e-7c3e-4d0a-9f8a-2b1f6f0d9c11"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "served here, deferred=true");
    }

    #[tokio::test]
    async fn the_previews_api_is_never_escalated_by_its_branch_parameter() {
        as_serve();
        let (status, body) = call(
            "DELETE",
            &format!("/api/{WS}/previews?branch=feat%2Fx"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, "deleted");
    }

    /// `?branch=` on the checks route names the preview whose check to read
    /// (Postgres), not a working copy — so a serve replica answers it.
    #[tokio::test]
    async fn the_checks_route_is_never_escalated_by_its_branch_parameter() {
        as_serve();
        let (status, body) = call(
            "GET",
            &format!("/api/{WS}/previews/checks?branch=feat%2Fx"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, "checks");
    }

    /// Same for the held procedure runs list: `?branch=` names the preview
    /// whose runs to list, all of it Postgres.
    #[tokio::test]
    async fn the_runs_route_is_never_escalated_by_its_branch_parameter() {
        as_serve();
        let (status, body) = call(
            "GET",
            &format!("/api/{WS}/previews/runs?branch=feat%2Fx"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, "runs");
    }
}
