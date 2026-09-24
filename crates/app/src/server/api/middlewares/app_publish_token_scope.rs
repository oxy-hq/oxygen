//! Scope enforcement for app-publish-token-authenticated requests.
//!
//! App publish tokens (`oxypublish_...` bearer) are minted for **machine auth**
//! (primarily `oxyc publish` in CI). They are deliberately narrow: they may act
//! as an app-admin for **publishing and read-only inspection of the
//! customer-apps surface only**. The auth middleware attaches an
//! [`AppPublishTokenAuth`] marker whenever a request authenticated via one of
//! these tokens; this middleware reads that marker and rejects anything outside
//! that narrow grant.
//!
//! Why an allow-list rather than per-route guards: the token resolves to a real
//! app-admin user, so it would otherwise pass every guard that user passes —
//! including destructive routes (delete an app, delete users/orgs) and, worse,
//! `POST /customer-apps/{id}/secrets`, which writes a *persisted* credential
//! the app's functions read at runtime (the exact escalation-persistence shape
//! these tokens must not enable). This middleware is the single choke-point that keeps the blast
//! radius to "ship a build + look at the surface". Requests WITHOUT the marker
//! (cookie/JWT/API-key sessions) are unaffected.
//!
//! **Scope, before the grant.** A token minted for one app (OIDC-minted, or
//! partner-minted) reaches `/customer-apps/{its own uuid}[/…]` and the upload
//! `/customer-apps/publish`, and answers `404` for everything else on the
//! surface — another app's id, the registry listing, and the fleet-wide
//! rollups (`fleet-health`, `storage`, `storage/history`, `oxy-access`) that
//! name every app or take their app in a query parameter. An allow-list rather
//! than a list of refusals, because the refusing shape covers only what someone
//! remembered to name. That is what makes "a publish token cannot learn which
//! apps exist" a property rather than an aspiration. An app-UNSCOPED staff
//! token is bounded by the grant below alone.
//!
//! **Grant (method-aware):**
//!   - `GET`/`HEAD` on `/customer-apps/…` — read-only inspection (list, get,
//!     builds, activity).
//!   - `POST /customer-apps/publish` — the CLI tarball upload.
//!   - `POST /customer-apps/{id}/publish` — promote draft → live.
//!   - `POST /customer-apps/{id}/functions/{name}/runs` — run one of the app's
//!     **declared checks**, so CI can verify what it just shipped without
//!     storing an API key. The path is only half the grant: `run_function_job`
//!     refuses a function the manifest does not mark `"check": true`, and an
//!     app-scoped token may not name another app. The justification is narrow
//!     and worth stating — a token that may publish arbitrary code to an app
//!     can already cause any side effect that app can, so running *that app's
//!     own declared checks* adds nothing. It would stop being true the moment
//!     this admitted any function of the app.
//!   - everything else (DELETE/unpublish, PATCH/update, create, rollback,
//!     `POST /{id}/secrets`, preview-draft, and every non-customer-apps path)
//!     → `403`.
//!
//! Runs immediately after `auth_middleware` (which sets the marker). NOTE:
//! `api_router` is mounted with `.nest("/api", …)`, and axum strips the nest
//! prefix before this layer runs — so `request.uri().path()` here is
//! nest-relative (`/customer-apps/…`), WITHOUT the `/api` prefix. The matcher
//! below works on that stripped form (see the nested-router regression tests).

use axum::http::{Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use oxy_auth::types::AppPublishTokenAuth;

pub async fn app_publish_token_scope_middleware(
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // Only constrain requests that authenticated via an app publish token.
    // Every other credential passes straight through.
    let Some(marker) = request.extensions().get::<AppPublishTokenAuth>().cloned() else {
        return Ok(next.run(request).await);
    };

    // An app-scoped token reaches its own app and the upload, and NOTHING else
    // on the surface. Stated as an allow-list on purpose: the refusing version
    // of this rule — "another app's id is refused" — only ever covers the paths
    // that name an app in a place a matcher can see, and the surface is full of
    // ones that do not. `/customer-apps/fleet-health` and `/customer-apps/storage`
    // are fleet-wide rollups naming every app; `/customer-apps/storage/history`
    // takes its app in a QUERY parameter. Each would have to be remembered, and
    // so would the next one added. Enumerating them is the losing game this
    // middleware exists to stop playing.
    //
    // `NOT_FOUND`, not `FORBIDDEN` — the same answer an out-of-scope App
    // Operator gets, so a token cannot use the difference to learn which app
    // ids exist. An app-UNSCOPED staff token is bounded by the grant below
    // alone and reaches all of this.
    if let Some(scoped) = marker.app_id {
        let path = request.uri().path();
        let own_app = app_id_in_path(path) == Some(scoped);
        if !own_app && !is_upload_route(path) {
            tracing::warn!(
                token_app = %scoped,
                path = %path,
                "app-scoped publish token reached outside its app — 404"
            );
            return Err(StatusCode::NOT_FOUND);
        }
    }

    if is_allowed(request.method(), request.uri().path()) {
        Ok(next.run(request).await)
    } else {
        tracing::warn!(
            method = %request.method(),
            path = %request.uri().path(),
            "app-publish-token request out of scope — rejected (tokens may only \
             publish + read the customer-apps surface)"
        );
        Err(StatusCode::FORBIDDEN)
    }
}

/// The narrow grant: reads anywhere on the customer-apps surface, but only the
/// two publish endpoints may mutate.
fn is_allowed(method: &Method, path: &str) -> bool {
    if !under_custom_apps(path) {
        return false;
    }
    match *method {
        // Read-only inspection — no mutation, no credential mint.
        Method::GET | Method::HEAD => true,
        // The two mutating actions a publish token needs: shipping a build,
        // and running the app's own declared checks against what it shipped.
        // The second is narrowed twice over — this matcher admits only the
        // function-run path, and `run_function_job` then refuses a function
        // the manifest does not mark `"check": true`, because a path shape
        // cannot tell a check from any other function.
        Method::POST => is_publish_route(path) || is_function_run_route(path),
        // DELETE (delete/unpublish), PATCH (update), and any other POST
        // (create, rollback, api-key mint, preview-draft) are out of scope.
        _ => false,
    }
}

/// Segment-boundary prefix check so `/customer-apps-evil` can't pass as
/// `/customer-apps`.
fn under_custom_apps(path: &str) -> bool {
    path == "/customer-apps"
        || path
            .strip_prefix("/customer-apps")
            .is_some_and(|rest| rest.starts_with('/'))
}

/// True only for `POST /customer-apps/publish` (CLI upload) and
/// `POST /customer-apps/{id}/publish` (promote). Matched by exact segment
/// shape so siblings like `/customer-apps/{id}/secrets` never qualify.
fn is_publish_route(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/customer-apps/") else {
        return false;
    };
    let mut segs = rest.split('/');
    match (segs.next(), segs.next(), segs.next()) {
        // /customer-apps/publish
        (Some("publish"), None, _) => true,
        // /customer-apps/{id}/publish
        (Some(id), Some("publish"), None) => !id.is_empty(),
        _ => false,
    }
}

/// The one path an app-scoped token may take that names no app: the CLI upload,
/// whose body carries `org/app` and which the publish handler matches against
/// the token's own scope. Exact, not a prefix — this is the single hole in the
/// confinement, so it is the width of the one route that needs it. (The only
/// child today, `/customer-apps/publish/oidc-exchange`, is on the public router
/// and carries no marker, so it never reaches this.)
fn is_upload_route(path: &str) -> bool {
    path == "/customer-apps/publish"
}

/// The app id a `/customer-apps/{id}/…` path names, when its first segment is
/// one. `None` for the static siblings (`/customer-apps/publish`,
/// `/customer-apps/batch/…`), for the fleet-wide rollups, and for the
/// collection itself — none of which name an app a matcher can read, which is
/// why the caller treats `None` as "not your app" rather than "unconstrained".
fn app_id_in_path(path: &str) -> Option<uuid::Uuid> {
    path.strip_prefix("/customer-apps/")?
        .split('/')
        .next()
        .and_then(|seg| uuid::Uuid::parse_str(seg).ok())
}

/// True only for `POST /customer-apps/{id}/functions/{name}/runs` — the
/// "run now" that `oxyc checks run` drives.
///
/// This is the path shape only. Whether the named function is a **check** is
/// decided in `run_function_job`, from the app's own manifest: a path cannot
/// distinguish `check_orders_api` from a function that emails customers, and
/// the difference is the whole grant. Matched by exact segment count so
/// neither `/functions/{name}/invocations` nor a longer sibling qualifies.
fn is_function_run_route(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/customer-apps/") else {
        return false;
    };
    let mut segs = rest.split('/');
    match (
        segs.next(),
        segs.next(),
        segs.next(),
        segs.next(),
        segs.next(),
    ) {
        (Some(id), Some("functions"), Some(name), Some("runs"), None) => {
            !id.is_empty() && !name.is_empty()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Paths are nest-relative: axum strips `/api` before this middleware, so
    // the matcher and these tests use `/customer-apps`, not `/api/customer-apps`.
    // The nested-router tests below drive a real request through
    // `.nest("/api", …)` to prove that's the path shape.

    #[test]
    fn allows_a_function_run_and_nothing_shaped_like_it() {
        // The one POST added for `oxyc checks run`. Whether the named function
        // is a check is decided in `run_function_job`, not here.
        assert!(is_allowed(
            &Method::POST,
            "/customer-apps/3f2504e0/functions/check_orders/runs"
        ));
        // Siblings that are NOT the run route.
        for path in [
            "/customer-apps/3f2504e0/functions",
            "/customer-apps/3f2504e0/functions/check_orders",
            "/customer-apps/3f2504e0/functions/check_orders/invocations",
            "/customer-apps/3f2504e0/functions/check_orders/runs/extra",
            "/customer-apps/3f2504e0/function-runs/abc",
            "/customer-apps//functions/check_orders/runs",
            "/customer-apps/3f2504e0/functions//runs",
        ] {
            assert!(
                !is_allowed(&Method::POST, path),
                "POST {path} must not be allowed"
            );
        }
        // Still no other method on it.
        assert!(!is_allowed(
            &Method::DELETE,
            "/customer-apps/3f2504e0/functions/check_orders/runs"
        ));
        // And still nothing outside the surface.
        assert!(!is_allowed(
            &Method::POST,
            "/admin/apps/3f2504e0/functions/check_orders/runs"
        ));
    }

    #[test]
    fn allows_publish_and_reads() {
        // Reads: any GET on the surface.
        assert!(is_allowed(&Method::GET, "/customer-apps"));
        assert!(is_allowed(&Method::GET, "/customer-apps/3f2504e0"));
        assert!(is_allowed(&Method::GET, "/customer-apps/3f2504e0/builds"));
        assert!(is_allowed(&Method::HEAD, "/customer-apps/3f2504e0"));
        // Publish: the two POST endpoints.
        assert!(is_allowed(&Method::POST, "/customer-apps/publish"));
        assert!(is_allowed(&Method::POST, "/customer-apps/3f2504e0/publish"));
    }

    #[test]
    fn blocks_destructive_and_credential_mint() {
        // The escalation-persistence shape: writing a persisted credential.
        assert!(!is_allowed(
            &Method::POST,
            "/customer-apps/3f2504e0/secrets"
        ));
        assert!(!is_allowed(&Method::DELETE, "/customer-apps/3f2504e0"));
        assert!(!is_allowed(
            &Method::DELETE,
            "/customer-apps/3f2504e0/publish"
        ));
        // Mutations that aren't publish.
        assert!(!is_allowed(&Method::PATCH, "/customer-apps/3f2504e0"));
        assert!(!is_allowed(&Method::POST, "/customer-apps")); // create
        assert!(!is_allowed(
            &Method::POST,
            "/customer-apps/3f2504e0/rollback"
        ));
        assert!(!is_allowed(&Method::POST, "/customer-apps/preview-draft"));
    }

    #[test]
    fn blocks_everything_off_the_custom_apps_surface() {
        // Token self-management must never be reachable by a token.
        assert!(!is_allowed(&Method::GET, "/admin/app-publish-tokens"));
        assert!(!is_allowed(&Method::POST, "/admin/app-publish-tokens"));
        // Destructive owner/admin surfaces.
        assert!(!is_allowed(&Method::DELETE, "/admin/orgs/3f2504e0"));
        assert!(!is_allowed(&Method::GET, "/orgs"));
        // Prefix-boundary attack: a sibling that shares the string prefix but
        // not the segment boundary.
        assert!(!is_allowed(&Method::POST, "/customer-apps-evil/publish"));
    }

    // ── Nested-router regression tests ─────────────────────────────────────
    // Drive requests through the real `.nest("/api", …)` so the path the
    // middleware sees is the actual stripped, nest-relative one — the check a
    // bare `is_allowed` unit test can't provide.
    use axum::body::Body;
    use axum::routing::{get, post};
    use axum::{Router, middleware};
    use tower::ServiceExt;

    async fn ok() -> StatusCode {
        StatusCode::OK
    }

    /// Stamps the `AppPublishTokenAuth` marker, standing in for `auth_middleware`
    /// recognizing an `oxypublish_…` bearer.
    async fn inject_marker(mut req: Request<Body>, next: Next) -> Response {
        req.extensions_mut().insert(AppPublishTokenAuth {
            token_id: uuid::Uuid::nil(),
            app_id: None,
        });
        next.run(req).await
    }

    /// The app an app-scoped token is minted for, in the scoped-marker tests.
    const SCOPED_APP: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
    const OTHER_APP: &str = "7c9e6679-7425-40de-944b-e07fc1f90ae7";

    /// The OIDC-minted shape: a token carrying the one app it may touch.
    async fn inject_scoped_marker(mut req: Request<Body>, next: Next) -> Response {
        req.extensions_mut().insert(AppPublishTokenAuth {
            token_id: uuid::Uuid::nil(),
            app_id: Some(uuid::Uuid::parse_str(SCOPED_APP).unwrap()),
        });
        next.run(req).await
    }

    /// `nested_app`, but the marker names `SCOPED_APP`.
    fn scoped_app() -> Router {
        let inner = Router::new()
            .route("/customer-apps", get(ok))
            .route("/customer-apps/publish", post(ok))
            .route("/customer-apps/publish/anything", post(ok))
            .route("/customer-apps/{id}", get(ok).delete(ok))
            .route("/customer-apps/{id}/publish", post(ok))
            .route("/customer-apps/{id}/builds", get(ok))
            .route("/customer-apps/{id}/functions", get(ok))
            .route("/customer-apps/{id}/functions/{name}/invocations", get(ok))
            .route("/customer-apps/{id}/functions/{name}/runs", post(ok))
            // The fleet-wide siblings, registered so a 404 below is the
            // middleware's answer and not axum's "no such route".
            .route("/customer-apps/fleet-health", get(ok))
            .route("/customer-apps/storage", get(ok))
            .route("/customer-apps/storage/history", get(ok))
            .route("/customer-apps/oxy-access", get(ok))
            .layer(middleware::from_fn(app_publish_token_scope_middleware))
            .layer(middleware::from_fn(inject_scoped_marker));
        Router::new().nest("/api", inner)
    }

    /// Router shaped like production: routes registered WITHOUT `/api`, the
    /// scope middleware layered inside, the whole thing nested under `/api`.
    fn nested_app(with_marker: bool) -> Router {
        let mut inner = Router::new()
            .route("/customer-apps", get(ok))
            .route("/customer-apps/publish", post(ok))
            .route("/customer-apps/{id}", get(ok).delete(ok))
            .route("/customer-apps/{id}/publish", post(ok))
            .route("/customer-apps/{id}/secrets", post(ok))
            .route("/admin/app-publish-tokens", get(ok))
            .layer(middleware::from_fn(app_publish_token_scope_middleware));
        if with_marker {
            // Layered last → outermost → runs before the scope middleware.
            inner = inner.layer(middleware::from_fn(inject_marker));
        }
        Router::new().nest("/api", inner)
    }

    async fn status_of(app: Router, method: &str, uri: &str) -> StatusCode {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        app.oneshot(req).await.unwrap().status()
    }

    #[tokio::test]
    async fn publish_upload_and_promote_are_reachable() {
        assert_eq!(
            status_of(nested_app(true), "POST", "/api/customer-apps/publish").await,
            StatusCode::OK,
            "CLI upload must be reachable by a publish token"
        );
        assert_eq!(
            status_of(nested_app(true), "POST", "/api/customer-apps/abc/publish").await,
            StatusCode::OK,
            "promote must be reachable by a publish token"
        );
    }

    #[tokio::test]
    async fn reads_are_reachable() {
        assert_eq!(
            status_of(nested_app(true), "GET", "/api/customer-apps").await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn credential_write_and_delete_are_blocked() {
        // The escalation-persistence route: writing a persisted credential.
        assert_eq!(
            status_of(nested_app(true), "POST", "/api/customer-apps/abc/secrets").await,
            StatusCode::FORBIDDEN,
            "publish tokens must not write app secrets"
        );
        // Destroying an app registration.
        assert_eq!(
            status_of(nested_app(true), "DELETE", "/api/customer-apps/abc").await,
            StatusCode::FORBIDDEN,
            "publish tokens must not delete apps"
        );
    }

    #[tokio::test]
    async fn an_app_scoped_token_reaches_only_its_own_app() {
        for (method, suffix) in [
            ("GET", ""),
            ("GET", "/builds"),
            ("GET", "/functions"),
            ("POST", "/publish"),
        ] {
            assert_eq!(
                status_of(
                    scoped_app(),
                    method,
                    &format!("/api/customer-apps/{SCOPED_APP}{suffix}")
                )
                .await,
                StatusCode::OK,
                "{method} its own app{suffix} must be reachable"
            );
        }
        assert_eq!(
            status_of(
                scoped_app(),
                "POST",
                &format!("/api/customer-apps/{SCOPED_APP}/functions/check_orders/runs")
            )
            .await,
            StatusCode::OK,
            "its own check must be reachable — the manifest flag is checked in the handler"
        );
        // The upload names no app; the token's own scope is matched server-side
        // against the slug it sends. Exactly that path — the exception is not a
        // prefix, so a child of it would be confined like anything else.
        assert_eq!(
            status_of(scoped_app(), "POST", "/api/customer-apps/publish").await,
            StatusCode::OK
        );
        assert_eq!(
            status_of(scoped_app(), "POST", "/api/customer-apps/publish/anything").await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn an_app_scoped_token_cannot_see_another_app_or_the_registry() {
        // Every read shape on the surface, including the ones no handler guards:
        // this is why the rule lives in the middleware and not in each handler.
        for (method, suffix) in [
            ("GET", ""),
            ("GET", "/builds"),
            ("GET", "/functions"),
            ("GET", "/functions/check_orders/invocations"),
            ("POST", "/publish"),
            ("POST", "/functions/check_orders/runs"),
        ] {
            assert_eq!(
                status_of(
                    scoped_app(),
                    method,
                    &format!("/api/customer-apps/{OTHER_APP}{suffix}")
                )
                .await,
                StatusCode::NOT_FOUND,
                "{method} another app{suffix} must answer 404, not 403 — a 403 would \
                 confirm the app exists"
            );
        }
        // The other ways to learn what exists: the registry, and the fleet-wide
        // rollups. `storage/history` takes its app in a QUERY parameter, which
        // no path matcher can see — the reason this rule is an allow-list.
        for path in [
            "/api/customer-apps",
            "/api/customer-apps/fleet-health",
            "/api/customer-apps/storage",
            "/api/customer-apps/oxy-access",
            "/api/customer-apps/storage/history?appId=7c9e6679-7425-40de-944b-e07fc1f90ae7",
        ] {
            assert_eq!(
                status_of(scoped_app(), "GET", path).await,
                StatusCode::NOT_FOUND,
                "GET {path} names every app, or names one in a query — an app-scoped \
                 token must not reach it"
            );
        }
    }

    #[tokio::test]
    async fn an_unscoped_staff_token_is_bounded_by_the_grant_alone() {
        // `nested_app`'s marker carries no app_id: the confinement above does
        // not apply, and the allow-list is what bounds it. Proving this keeps
        // the two rules from being conflated.
        assert_eq!(
            status_of(nested_app(true), "GET", "/api/customer-apps").await,
            StatusCode::OK
        );
        assert_eq!(
            status_of(
                nested_app(true),
                "GET",
                &format!("/api/customer-apps/{OTHER_APP}")
            )
            .await,
            StatusCode::OK
        );
        assert_eq!(
            status_of(
                nested_app(true),
                "DELETE",
                &format!("/api/customer-apps/{OTHER_APP}")
            )
            .await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn token_self_management_is_blocked() {
        assert_eq!(
            status_of(nested_app(true), "GET", "/api/admin/app-publish-tokens").await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn non_publish_token_requests_pass_through() {
        // No marker → not scope-limited → reaches even /admin/*.
        assert_eq!(
            status_of(nested_app(false), "GET", "/api/admin/app-publish-tokens").await,
            StatusCode::OK
        );
    }
}
