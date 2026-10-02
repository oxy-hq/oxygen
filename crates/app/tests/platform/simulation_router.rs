//! The simulation routes, driven through the **real** router.
//!
//! Every other test in `simulation_routes` calls `list_worlds` / `start_run`
//! directly, which proves the logic and skips the transport layer. Three bugs
//! in a row lived exactly in that gap, and none of them could fail a compile:
//!
//!   * `Path<String>` under a router mounted at `/{workspace_id}` — axum sees
//!     two segments and rejects with "Wrong number of path arguments".
//!   * `Extension<Uuid>` that no middleware anywhere inserts — "Missing request
//!     extension".
//!
//! Both are runtime rejections raised *before* the handler body runs, because
//! axum's extractors are checked dynamically: `Extension<T>` compiles for any
//! `T`, and `Path` arity is a runtime property of the matched route. A test
//! that hand-constructs the extractors asserts the one thing that was never in
//! doubt.
//!
//! So this asserts something narrower and far more useful: whatever these
//! routes answer — 401, 404, 500 from an absent database — it is never an
//! *extractor rejection*. That is the whole class, and it needs no auth and no
//! seeded data to check.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxy_app::server::router::api_router;
use oxy_app_core::serve_mode::ServeMode;
use tower::ServiceExt;

/// Stand up a per-test database and point the process at it.
///
/// **Not** an ambient-`OXY_DATABASE_URL` skip like `local_mode_router`. A guard
/// that silently returns when the environment is bare is a guard that passes
/// forever on a laptop — this one skipped in 0.00s and reported success while
/// the very bug it was written for was still in the tree. `api_router` builds
/// database-backed middleware, so it needs a real database; `fresh_db` gives it
/// one unconditionally.
async fn boot_db() {
    let (_db, url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    // SAFETY: single-threaded test setup; nextest isolates each test in its own
    // process.
    unsafe {
        std::env::set_var("OXY_DATABASE_URL", &url);
        std::env::remove_var("OXY_DATABASE_AUTH_MODE");
    }
}

/// Phrases axum uses when an extractor cannot be satisfied. Matching on the
/// message rather than the status is deliberate: these surface as 500, and so
/// does an absent database, which is a legitimate answer here.
const EXTRACTOR_REJECTIONS: &[&str] = &[
    "Wrong number of path arguments",
    "Missing request extension",
    "Invalid URL param",
];

async fn body_text(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test]
async fn no_simulation_route_fails_on_its_own_extractors() {
    boot_db().await;
    let ws = uuid::Uuid::nil();
    let cases: [(&str, &str); 5] = [
        ("GET", &format!("/{ws}/simulations").leak()[..]),
        ("GET", &format!("/{ws}/simulations/runs").leak()[..]),
        (
            "GET",
            &format!("/{ws}/simulations/runs/{}", uuid::Uuid::nil()).leak()[..],
        ),
        ("POST", &format!("/{ws}/simulations/demo/runs").leak()[..]),
        // Three path segments under a router mounted at `/{workspace_id}`, so
        // the handler's `Path` tuple has to be a triple's worth of arity — the
        // same trap `/simulations/{name}/runs` fell into.
        ("GET", &format!("/{ws}/simulations/demo/race").leak()[..]),
    ];

    for (method, path) in cases {
        let (router, _external, _preagg) = api_router(
            ServeMode::Local,
            false,
            None,
            std::path::PathBuf::new(),
            tokio_util::sync::CancellationToken::new(),
            false,
            oxy_app::server::router::SurfaceSeams::empty(),
        )
        .await
        .expect("build router");

        let req = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(req).await.expect("router responded");
        let status = response.status();
        let body = body_text(response).await;

        // A route that isn't mounted is its own failure — the point is that
        // these are reachable. But `get_run` legitimately 404s for a run that
        // does not exist, so the status alone cannot tell the two apart. The
        // body can: axum's unmatched-route fallback is empty, and every 404
        // this module raises carries a reason.
        if status == StatusCode::NOT_FOUND {
            assert!(
                !body.trim().is_empty(),
                "{method} {path} is not mounted — the router fell through"
            );
        }
        for phrase in EXTRACTOR_REJECTIONS {
            assert!(
                !body.contains(phrase),
                "{method} {path} failed on its own extractors ({status}): {body}"
            );
        }
    }
}
