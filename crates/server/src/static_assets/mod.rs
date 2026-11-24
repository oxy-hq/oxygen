//! Serving the SPA: the embedded `dist/`, its caching contract, and the SPA
//! fallback.
//!
//! Lifted out of `commands::serve` whole — the composition root should mount a
//! static handler, not define one. What lives here is everything that answers
//! a request for a file: the `include_dir!` of `web-app/dist`, the three
//! `Cache-Control` policies those files get, the extension allowlist that
//! decides a miss is a 404 rather than the shell, and the org-injection +
//! ETag pass over the shell itself.
//!
//! [`precompressed`] is the brotli half: a production build writes a `.br`
//! beside each compressible asset and this module hands it out when the client
//! can read it.

pub(crate) mod precompressed;

use axum::body::Body;
use axum::handler::Handler;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get_service;
use include_dir::{Dir, include_dir};
use oxy_app_core::http_cache::{if_none_match, wants_html, weak_etag};
use tower_serve_static::ServeDir;

static DIST: Dir = include_dir!("$CARGO_MANIFEST_DIR/dist");

/// Content-hashed, so the URL names these exact bytes forever.
const ASSETS_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// `/fonts/*` — unhashed, so a week rather than a year. See the call site.
const FONTS_CACHE_CONTROL: &str = "public, max-age=604800";

pub(crate) async fn handle_static_files(
    req: Request<Body>,
) -> Result<axum::response::Response, std::convert::Infallible> {
    // The org-subdomain dispatch middleware attaches this for product routes
    // on a bare org host; when present we splice `window.__OXY_ORG__` into the
    // served index.html so the SPA boots pre-scoped to the org + default
    // project (and skips the org/workspace picker).
    let org_ctx = req
        .extensions()
        .get::<oxy_app_core::org_host_dispatch::OrgSubdomainCtx>()
        .cloned();
    let request_headers = req.headers().clone();
    let uri = req.uri().clone();
    let path = uri.path();

    let mut response = serve_file(req, path, &request_headers).await;

    if response.status().is_success() {
        stamp_static_headers(&mut response, path);
    } else if response.status() == StatusCode::NOT_FOUND {
        // Two independent gates, OR'd so neither can regress the other: what
        // the client asked for (`Accept`, precise but absent on some clients)
        // and what the path looks like (a suffix, coarse but header-free).
        if !wants_html(&request_headers) || is_static_file_request(path) {
            return Ok(response);
        }
        response = serve_shell().await;
    }

    Ok(finalize_html(response, org_ctx.as_ref(), &request_headers).await)
}

/// The file for this request: the brotli sibling the build left beside it
/// (`web-app/vite-plugins/precompress.ts`) when the client can read one, and
/// otherwise the raw file, exactly as before there were siblings.
async fn serve_file(
    req: Request<Body>,
    path: &str,
    request_headers: &HeaderMap,
) -> axum::response::Response {
    match precompressed::try_serve(&DIST, path, request_headers) {
        Some(response) => response,
        None => {
            get_service(ServeDir::new(&DIST))
                .call(req, None::<()>)
                .await
        }
    }
}

/// The SPA shell, for a URL the client routes rather than fetches.
async fn serve_shell() -> axum::response::Response {
    let request = Request::builder()
        .uri("/index.html")
        .body(Body::empty())
        .expect("a static URI and an empty body cannot fail to build");
    get_service(ServeDir::new(&DIST))
        .call(request, None::<()>)
        .await
}

/// The caching and encoding contract for a static **hit**.
///
/// Nothing here may run on a miss: stamping `immutable` on a 404 would pin it
/// in the browser cache for a year, and misses under `/assets/` are exactly
/// what a client holding a stale `index.html` generates.
///
/// One policy per path, so the two arms are exclusive by construction:
///
/// - `/assets/*` is content-hashed — the URL names these bytes forever.
/// - `/fonts/*` is not, so never `immutable`:
///   `/fonts/Inter-VariableFont.woff2` is whatever the current build put
///   there. A week is the trade — the 0.33 MiB woff2 and 0.83 MiB ttf are on
///   the critical path of every first paint and change about never, and a font
///   that lags a deploy by a few days renders the same text in the same face.
///   Deliberately not widened to the rest of `publicDir`: `oxy-push-sw.js` is
///   a service worker, and a week-old one is a week of an app that cannot
///   update itself.
fn stamp_static_headers(response: &mut axum::response::Response, path: &str) {
    let cache_control = if path.starts_with("/assets/") {
        Some(ASSETS_CACHE_CONTROL)
    } else if path.starts_with("/fonts/") {
        Some(FONTS_CACHE_CONTROL)
    } else {
        None
    };
    let headers = response.headers_mut();
    if let Some(policy) = cache_control {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(policy));
    }
    // Only where there are two representations to choose between — see
    // `VARY_ACCEPT_ENCODING`.
    if precompressed::has_sibling(&DIST, path) {
        headers.insert(
            header::VARY,
            HeaderValue::from_static(precompressed::VARY_ACCEPT_ENCODING),
        );
    }
}

/// `Cache-Control` for the SPA shell.
///
/// `no-cache` means "you may store this, but revalidate before reusing it" —
/// not "don't store it". That distinction is the whole fix. Previously the
/// shell went out with *no* cache directives at all: this handler set a header
/// only for `/assets/*`, and `tower-serve-static` emits nothing but
/// `Content-Type` (its `Last-Modified` lives behind a non-default `metadata`
/// feature we don't enable). With no directive and no validator, a browser was
/// free to replay a stored copy on a back/forward navigation without ever
/// asking us. After a deploy rotates the chunk hashes, that copy names files
/// which no longer exist, every one 404s, and the SPA never boots.
///
/// Deliberately NOT `no-store`: that would make the page ineligible for the
/// back/forward cache, turning every Back into a full reload. `no-cache`
/// governs the HTTP cache only and leaves bfcache — the fast path — alone.
const HTML_CACHE_CONTROL: &str = "no-cache";

/// Cap on the shell body we'll buffer to hash. The shell is a few KB; this is
/// a backstop, not a budget.
const HTML_BUFFER_LIMIT: usize = 16 * 1024 * 1024;

/// Extensions that mark a request as a request for a *file*, so a miss must
/// answer 404 rather than fall through to the SPA shell.
const STATIC_FILE_EXTENSIONS: &[&str] = &[
    "js",
    "mjs",
    "cjs",
    "css",
    "map",
    "wasm",
    "json",
    "html",
    "htm",
    "svg",
    "png",
    "jpg",
    "jpeg",
    "gif",
    "webp",
    "avif",
    "ico",
    "woff",
    "woff2",
    "ttf",
    "otf",
    "eot",
    "txt",
    "xml",
    "webmanifest",
    "mp4",
    "webm",
];

/// True when `path` addresses a static file rather than a client route.
///
/// The SPA fallback exists so client-routed URLs (`/threads/<id>`, `/ide`)
/// load the shell. A request for a file is not a client route, and answering
/// its miss with `index.html` is actively harmful: the browser gets `text/html`
/// for a `<script type="module">`, strict MIME checking blocks it, and the page
/// goes blank with nothing the app can react to — the `vite:preloadError`
/// handler that would self-heal lives inside the bundle that just got blocked.
/// A real 404 keeps that failure loud and recoverable.
///
/// The *primary* gate is `wants_html` — a subresource announces itself in
/// `Accept` whether or not its URL carries a suffix, which is what catches an
/// extensionless `fetch("/some/data")`. This is the second gate, for clients
/// that send no usable `Accept` at all (where `wants_html` deliberately
/// defaults to "yes" so the SPA still renders for `curl`).
///
/// Matched by extension allowlist rather than "has a dot" so a client route
/// that happens to contain one keeps rendering the shell.
fn is_static_file_request(path: &str) -> bool {
    if path.starts_with("/assets/") {
        return true;
    }
    let last_segment = path.rsplit('/').next().unwrap_or_default();
    let Some((_, ext)) = last_segment.rsplit_once('.') else {
        return false;
    };
    STATIC_FILE_EXTENSIONS
        .iter()
        .any(|known| ext.eq_ignore_ascii_case(known))
}

/// Apply the shell's caching contract to an HTML response, passing anything
/// else through untouched.
///
/// Org injection happens first so the ETag covers the bytes we actually ship —
/// two org subdomains serve different shells from the same file and must not
/// share a validator.
async fn finalize_html(
    response: axum::response::Response,
    org_ctx: Option<&oxy_app_core::org_host_dispatch::OrgSubdomainCtx>,
    request_headers: &axum::http::HeaderMap,
) -> axum::response::Response {
    let is_html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/html"));
    if !is_html {
        return response;
    }

    let response = match org_ctx {
        Some(ctx) => oxy_app_core::org_host_dispatch::inject_org_into_response(response, ctx).await,
        None => response,
    };

    let (mut parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, HTML_BUFFER_LIMIT).await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!("static: failed to buffer html shell: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let etag = weak_etag(&bytes);
    parts.headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(HTML_CACHE_CONTROL),
    );
    if let Ok(value) = HeaderValue::from_str(&etag) {
        parts.headers.insert(header::ETAG, value);
    }
    // Set from the body below; a value carried over from the pre-injection
    // response would be wrong.
    parts.headers.remove(header::CONTENT_LENGTH);

    if if_none_match(request_headers, &etag) {
        parts.status = StatusCode::NOT_MODIFIED;
        parts.headers.remove(header::CONTENT_TYPE);
        return axum::response::Response::from_parts(parts, Body::empty());
    }

    axum::response::Response::from_parts(parts, Body::from(bytes))
}

#[cfg(test)]
mod tests {
    // ── SPA shell caching ────────────────────────────────────────────────
    //
    // Regression cover for the blank-page-on-back bug: a browser replayed a
    // stored index.html on a history navigation, its chunk hashes were stale
    // after a deploy, and every `/assets/*.js` miss came back as `text/html`.

    use super::*;
    use axum::Router;
    use tower::{ServiceExt, service_fn};

    /// Drive the real handler, the way the router mounts it.
    async fn serve_get(uri: &str, accept: Option<&str>) -> axum::response::Response {
        let mut req = Request::get(uri);
        if let Some(accept) = accept {
            req = req.header(header::ACCEPT, accept);
        }
        Router::new()
            .fallback_service(service_fn(handle_static_files))
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    /// A navigation's `Accept`, as Chrome sends it.
    const NAV_ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";

    /// The same handler, with the encoding a browser advertises.
    async fn serve_get_br(uri: &str) -> axum::response::Response {
        Router::new()
            .fallback_service(service_fn(handle_static_files))
            .oneshot(
                Request::get(uri)
                    .header(header::ACCEPT, "*/*")
                    .header(header::ACCEPT_ENCODING, "gzip, deflate, br, zstd")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    /// One JS asset's request path, picked out of whatever this build emitted
    /// rather than named — a hashed filename is invalidated by the next build.
    /// `None` in a Rust-only checkout, where `dist` is the committed symlink
    /// holding nothing but `vite.svg`.
    fn some_built_js() -> Option<String> {
        DIST.get_dir("assets")?
            .files()
            .map(|f| f.path().to_string_lossy().to_string())
            .find(|p| p.ends_with(".js"))
            .map(|p| format!("/{p}"))
    }

    /// The whole point of the build-time compression: the bytes a first visit
    /// downloads. Asserted through the mounted handler, not the probe, because
    /// what matters is what leaves the router.
    #[tokio::test]
    async fn an_asset_goes_out_brotli_compressed() {
        let Some(js) = some_built_js() else { return };

        let res = serve_get_br(&js).await;

        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get(header::CONTENT_ENCODING)
                .and_then(|v| v.to_str().ok()),
            Some("br"),
            "{js} must be served from its build-time .br sibling"
        );
        assert_eq!(
            res.headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some(ASSETS_CACHE_CONTROL),
            "the compressed representation is the same immutable resource"
        );
        assert_eq!(
            res.headers()
                .get(header::VARY)
                .and_then(|v| v.to_str().ok()),
            Some(precompressed::VARY_ACCEPT_ENCODING),
        );
    }

    /// The fallback that makes brotli-only safe: a client that does not
    /// advertise `br` still gets the file, uncompressed, as it always did.
    #[tokio::test]
    async fn an_asset_without_accept_encoding_is_served_raw() {
        let Some(js) = some_built_js() else { return };

        let res = serve_get(&js, Some("*/*")).await;

        assert_eq!(res.status(), StatusCode::OK);
        assert!(
            !res.headers().contains_key(header::CONTENT_ENCODING),
            "nothing may claim an encoding the client did not ask for"
        );
    }

    /// The shell is rewritten per org subdomain and hashed into an ETag, so it
    /// must never be answered from a file on disk that predates the rewrite.
    #[tokio::test]
    async fn the_shell_is_never_served_precompressed() {
        let res = serve_get_br("/index.html").await;

        assert!(
            !res.headers().contains_key(header::CONTENT_ENCODING),
            "a .br index.html would ship one org's context to another's host"
        );
    }

    #[tokio::test]
    async fn fonts_are_cacheable_but_not_immutable() {
        let res = serve_get("/fonts/Inter-VariableFont.woff2", Some("*/*")).await;
        if res.status() != StatusCode::OK {
            return; // Rust-only checkout, no built dist.
        }

        let cache_control = res
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok());
        assert_eq!(
            cache_control,
            Some(FONTS_CACHE_CONTROL),
            "a font on the first-paint path went out with no caching policy at all"
        );
        assert!(
            !cache_control.unwrap().contains("immutable"),
            "the filename carries no content hash, so the URL does not name these bytes"
        );
    }

    // The bug this PR exists to prevent, asserted through the handler rather
    // than its helpers: a stale shell asks for a chunk that no longer exists,
    // and the answer must be a 404 — not the shell at 200, which the browser
    // blocks on MIME and cannot recover from.
    #[tokio::test]
    async fn missing_asset_404s_instead_of_returning_the_shell() {
        // `*/*` is what a `<script type="module">` sends.
        let res = serve_get("/assets/react-dom-vendor-VMPilgUW.js", Some("*/*")).await;

        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        assert_ne!(
            res.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|ct| ct.starts_with("text/html")),
            Some(true),
            "answering a module request with HTML is what blanks the page"
        );
        assert_ne!(
            res.headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some(ASSETS_CACHE_CONTROL),
            "`immutable` on a miss would pin the 404 in the browser cache for a year"
        );
    }

    #[tokio::test]
    async fn extensionless_subresource_404s_too() {
        // No suffix to match on — only `Accept` distinguishes this from a
        // client route, which is why the two gates are OR'd.
        let res = serve_get("/some/sdk/data", Some("*/*")).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn client_route_still_gets_the_shell() {
        let res = serve_get("/threads/0effae8f-c261-4cd3-afb5", Some(NAV_ACCEPT)).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some(HTML_CACHE_CONTROL),
            "the shell must revalidate however it was reached"
        );
    }

    #[tokio::test]
    async fn client_route_gets_the_shell_without_an_accept_header() {
        // `wants_html` defaults to true when `Accept` is absent so curl and
        // header-less clients still render the SPA.
        let res = serve_get("/threads/0effae8f-c261-4cd3-afb5", None).await;
        assert_eq!(res.status(), StatusCode::OK);
    }

    fn html_response(body: &'static str) -> axum::response::Response {
        axum::response::Response::builder()
            .header(header::CONTENT_TYPE, "text/html")
            .body(Body::from(body))
            .unwrap()
    }

    #[test]
    fn asset_requests_never_fall_back_to_the_shell() {
        // The exact shape that blanked the page: a chunk from a stale shell.
        assert!(is_static_file_request(
            "/assets/react-dom-vendor-VMPilgUW.js"
        ));
        assert!(is_static_file_request("/assets/index-DRPa-Ace.css"));
        assert!(is_static_file_request("/favicon.ico"));
        assert!(is_static_file_request("/oxy-logo.svg"));
    }

    #[test]
    fn client_routes_still_fall_back_to_the_shell() {
        assert!(!is_static_file_request("/"));
        assert!(!is_static_file_request("/home"));
        assert!(!is_static_file_request("/threads/0effae8f-c261-4cd3-afb5"));
        assert!(!is_static_file_request("/ide"));
        // A dot in a client route is not an extension — matched by allowlist
        // precisely so this keeps rendering the SPA.
        assert!(!is_static_file_request("/apps/my.dashboard"));
    }

    #[tokio::test]
    async fn shell_is_served_revalidate_on_every_load() {
        let res = finalize_html(
            html_response("<html><head></head></html>"),
            None,
            &HeaderMap::new(),
        )
        .await;
        assert_eq!(
            res.headers()
                .get(header::CACHE_CONTROL)
                .map(|v| v.to_str().unwrap()),
            Some(HTML_CACHE_CONTROL),
            "the shell must revalidate, or a back navigation replays a stale \
             copy whose chunk hashes no longer exist"
        );
        assert!(
            res.headers().contains_key(header::ETAG),
            "revalidation needs a validator or every navigation refetches the body"
        );
    }

    #[tokio::test]
    async fn unchanged_shell_revalidates_to_304() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            weak_etag(b"<html><head></head></html>").parse().unwrap(),
        );
        let res = finalize_html(html_response("<html><head></head></html>"), None, &headers).await;
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn non_html_passes_through_untouched() {
        let res = axum::response::Response::builder()
            .header(header::CONTENT_TYPE, "application/javascript")
            .body(Body::from("console.log(1)"))
            .unwrap();
        let res = finalize_html(res, None, &HeaderMap::new()).await;
        assert!(
            !res.headers().contains_key(header::CACHE_CONTROL),
            "hashed assets keep their own immutable policy"
        );
    }
}
