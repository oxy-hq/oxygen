//! The middleware with a stub planes lookup: who is asked, and what each
//! answer lets through.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use super::*;
use crate::custom_app_env_request::environment_guard_middleware;

const STAGING_HOST: &str = "staging--acme--store.customer-apps.oxygen-hq.com";
const SANDBOX_HOST: &str = "dev-luong--acme--store.customer-apps.oxygen-hq.com";
const PROD_HOST: &str = "acme--store.customer-apps.oxygen-hq.com";
const ASK: &str = "/api/projects/p1/agents/analyst/asks";
const RUN_CANCEL: &str = "/api/projects/p1/procedures/runs/r1/cancel";

/// Answers `answer` to `held`, `previews` to `previews`, and counts the
/// questions.
struct Stub {
    answer: Result<bool, String>,
    previews: bool,
    asked: AtomicUsize,
}

impl Stub {
    fn new(answer: Result<bool, String>) -> Arc<Self> {
        Arc::new(Self {
            answer,
            previews: false,
            asked: AtomicUsize::new(0),
        })
    }

    /// A plane that is `preview`: it holds the three surfaces and previews.
    fn preview() -> Arc<Self> {
        Arc::new(Self {
            answer: Ok(true),
            previews: true,
            asked: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl WorkspaceActions for Stub {
    async fn held(&self, _: &HeaderMap, _: &AppEnvironment) -> Result<bool, String> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.answer.clone()
    }

    async fn previews(&self, _: &HeaderMap, _: &AppEnvironment) -> Result<bool, String> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        Ok(self.previews)
    }
}

/// `method uri` on `host` through the middleware; the status, and whether a
/// handler behind it ran.
async fn through(stub: Arc<Stub>, method: &str, host: &str, uri: &str) -> (StatusCode, bool) {
    let reached = Arc::new(AtomicUsize::new(0));
    let seen = reached.clone();
    let app = Router::new()
        .route(
            "/{*path}",
            axum::routing::any(move || {
                let seen = seen.clone();
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        )
        .layer(axum::middleware::from_fn_with_state(
            stub as Arc<dyn WorkspaceActions>,
            environment_guard_middleware,
        ));
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("authorization", "Bearer t")
        .body(Body::empty())
        .expect("request");
    let response = app.oneshot(request).await.expect("response");
    (response.status(), reached.load(Ordering::SeqCst) == 1)
}

#[tokio::test]
async fn a_held_plane_lets_the_surface_through_in_staging_and_a_sandbox() {
    for host in [STAGING_HOST, SANDBOX_HOST] {
        let stub = Stub::new(Ok(true));
        assert_eq!(
            through(stub.clone(), "POST", host, ASK).await,
            (StatusCode::OK, true),
            "{host}"
        );
        assert_eq!(stub.asked.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn a_refused_plane_keeps_the_403() {
    for host in [STAGING_HOST, SANDBOX_HOST] {
        assert_eq!(
            through(Stub::new(Ok(false)), "POST", host, ASK).await,
            (StatusCode::FORBIDDEN, false),
            "{host}"
        );
    }
}

/// A held plane opens the three surfaces and nothing else.
#[tokio::test]
async fn a_held_plane_does_not_open_any_other_write() {
    let stub = Stub::new(Ok(true));
    assert_eq!(
        through(
            stub.clone(),
            "POST",
            SANDBOX_HOST,
            "/api/projects/p1/threads"
        )
        .await,
        (StatusCode::FORBIDDEN, false)
    );
    assert_eq!(
        stub.asked.load(Ordering::SeqCst),
        0,
        "only the three surfaces cost a lookup"
    );
}

/// An automation run's cancel passes only where the plane is `preview`: the
/// bundle's run is real there. Under `held` nothing runs, so it stays `403`.
#[tokio::test]
async fn only_a_preview_plane_lets_a_run_cancel_through() {
    for host in [STAGING_HOST, SANDBOX_HOST] {
        assert_eq!(
            through(Stub::preview(), "POST", host, RUN_CANCEL).await,
            (StatusCode::OK, true),
            "{host}"
        );
        assert_eq!(
            through(Stub::new(Ok(true)), "POST", host, RUN_CANCEL).await,
            (StatusCode::FORBIDDEN, false),
            "held, {host}"
        );
    }
    let preview = Stub::preview();
    assert_eq!(
        through(
            preview.clone(),
            "POST",
            SANDBOX_HOST,
            "/api/projects/p1/threads"
        )
        .await,
        (StatusCode::FORBIDDEN, false),
        "preview opens no other write"
    );
    assert_eq!(preview.asked.load(Ordering::SeqCst), 0);
}

/// An unreadable plane refuses the request, `409`, and never falls back.
#[tokio::test]
async fn an_unreadable_plane_refuses_with_409() {
    let stub = Stub::new(Err("unknown field `olpt`".to_string()));
    let (status, reached) = through(stub, "POST", SANDBOX_HOST, ASK).await;
    assert_eq!((status, reached), (StatusCode::CONFLICT, false));
}

/// Production, and every read, ask nothing.
#[tokio::test]
async fn production_and_reads_are_not_asked() {
    let stub = Stub::new(Err("never asked".to_string()));
    assert_eq!(
        through(stub.clone(), "POST", PROD_HOST, ASK).await,
        (StatusCode::OK, true)
    );
    assert_eq!(
        through(
            stub.clone(),
            "GET",
            SANDBOX_HOST,
            "/api/projects/p1/threads"
        )
        .await,
        (StatusCode::OK, true)
    );
    assert_eq!(stub.asked.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_empty_lookup_holds_nothing() {
    let none: Arc<dyn WorkspaceActions> = Arc::new(NoWorkspaceActions);
    assert_eq!(
        none.held(&HeaderMap::new(), &AppEnvironment::Staging).await,
        Ok(false)
    );
}
