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
//! `api_router` connects to `OXY_DATABASE_URL` while it builds, and runs the
//! unscoped `cleanup_stale_runs` over `agentic_runs` — so these skip without a
//! database, and `.config/nextest.toml` pins this module into `serial-db`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxy_app::server::router::api_router;
use oxy_app_core::serve_mode::ServeMode;
use tower::ServiceExt;

use crate::surface_seams;

fn db_unavailable() -> bool {
    std::env::var("OXY_DATABASE_URL").is_err()
}

/// `GET /orgs` against the router `serve` builds in `mode`, from the seams boot
/// hands it.
async fn get_orgs(mode: ServeMode) -> StatusCode {
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
    let req = Request::builder().uri("/orgs").body(Body::empty()).unwrap();
    router.oneshot(req).await.expect("oneshot").status()
}

#[tokio::test]
async fn cloud_mode_mounts_the_organizations_surface() {
    if db_unavailable() {
        return;
    }
    // Mounted → the request reaches auth or the handler, not the router's 404.
    let status = get_orgs(ServeMode::Cloud).await;
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "cloud mode must keep /orgs mounted, got {status}"
    );
}

#[tokio::test]
async fn local_mode_drops_the_organizations_surface() {
    if db_unavailable() {
        return;
    }
    let status = get_orgs(ServeMode::Local).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "local mode never mounts the org tree, so it must drop the api seam /orgs rides"
    );
}
