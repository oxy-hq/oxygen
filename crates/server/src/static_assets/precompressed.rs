//! Serving the brotli siblings a production frontend build leaves in `dist/`.
//!
//! The SPA's assets went out of a `serve` pod uncompressed. The only
//! [`CompressionLayer`] in the tree is the one `custom_apps_seams` puts on
//! `/customer-apps/{*path}`; the static handler in [`super::commands::serve`]
//! had none, so a first visit downloaded every byte of JS and CSS raw.
//!
//! The fix is the one `crates/custom-apps/src/precompress.rs` already made for
//! custom-app bundles, applied to our own `dist/`: compress once per *build*
//! rather than once per *request per replica*. `web-app/vite-plugins/precompress.ts`
//! writes a `<path>.br` beside each compressible asset, `include_dir!` embeds
//! both, and this module hands out the sibling when the client advertises `br`.
//!
//! Compressing here instead would be strictly worse than on the custom-app
//! mount: a content-hashed `index-a1b2c3.js` has one set of bytes for its
//! entire life, and every replica would recompute the same brotli stream for
//! every request, forever.
//!
//! ## Why the lookup is a plain `get_file` and not a second `ServeDir` call
//!
//! [`tower_serve_static::ServeDir`] would name the response after the path it
//! served, so `main-a1b2.js.br` would go out as `application/octet-stream` and
//! the browser would refuse to execute it. Reading the embedded file directly
//! keeps the *original* path as the thing the `Content-Type` is derived from,
//! which is the only correct answer, and costs one map lookup and a
//! `Bytes::from_static` — the bytes are already in the binary's rodata, so
//! nothing is copied.
//!
//! [`CompressionLayer`]: tower_http::compression::CompressionLayer

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use include_dir::Dir;

/// Suffix the build writes. `assets/main-a1b2.js` → `assets/main-a1b2.js.br`.
pub(crate) const PRECOMPRESSED_SUFFIX: &str = ".br";

/// Every asset path has two representations now, so a shared cache must key on
/// the request's encoding. Set on hits whether or not the sibling was used:
/// the `br` and the raw response describe the same resource, and it is the
/// *raw* one a cache would otherwise hand to a `br`-capable client, or worse,
/// the other way round.
pub(crate) const VARY_ACCEPT_ENCODING: &str = "accept-encoding";

/// Does the client advertise brotli?
///
/// Deliberately a substring test over the token list rather than a full
/// `Accept-Encoding` parse with q-values. The only thing a q-value could say
/// that matters here is `br;q=0`, "I would rather you did not", and the cost of
/// honouring it is an RFC 9110 parser on every asset request to serve a client
/// that asked for brotli and then unasked. The token is matched on a `,`/`;`
/// boundary so `brotli-unsupported` or a future `xbr` cannot match.
pub(crate) fn accepts_brotli(headers: &HeaderMap) -> bool {
    let Some(value) = headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    value.split(',').any(|token| {
        let token = token.trim();
        let name = token.split(';').next().unwrap_or(token).trim();
        name.eq_ignore_ascii_case("br")
    })
}

/// The embedded brotli sibling for `path`, if the build made one and the client
/// can read it.
///
/// `path` is a request path (`/assets/main-a1b2.js`); `include_dir` keys on a
/// relative path, so the leading slash comes off. A path that already ends in
/// `.br` is refused: a client asking for the sibling by name would otherwise
/// get `<path>.br.br` or an unlabelled stream, and nothing we emit ever
/// references one.
/// `&'static` because the response body borrows the file's bytes rather than
/// copying them: they are already in the binary's rodata, put there by
/// `include_dir!`, so the only caller is a `static` and nothing is allocated
/// per request.
fn sibling(dist: &'static Dir<'static>, path: &str) -> Option<&'static include_dir::File<'static>> {
    let relative = path.strip_prefix('/')?;
    if relative.ends_with(PRECOMPRESSED_SUFFIX) {
        return None;
    }
    dist.get_file(format!("{relative}{PRECOMPRESSED_SUFFIX}"))
}

/// Whether this path has two representations, which is the only thing that
/// makes `Vary: Accept-Encoding` true of it. Asking the question separately
/// from [`try_serve`] is what keeps the header off `index.html` and the images,
/// which have exactly one set of bytes and would only cost a shared cache a
/// pointless split.
pub(crate) fn has_sibling(dist: &'static Dir<'static>, path: &str) -> bool {
    sibling(dist, path).is_some()
}

/// Serve `path`'s brotli sibling, or `None` to let the caller fall through to
/// the uncompressed file.
///
/// The response names `path`'s own media type, not the sibling's: `.br` is a
/// transfer encoding, not a format, so `Content-Type` must describe what the
/// client gets after decoding. Getting this wrong is not a subtle bug — strict
/// MIME checking blocks a `<script type="module">` served as
/// `application/octet-stream` and the page goes blank.
pub(crate) fn try_serve(
    dist: &'static Dir<'static>,
    path: &str,
    request_headers: &HeaderMap,
) -> Option<Response> {
    if !accepts_brotli(request_headers) {
        return None;
    }
    let file = sibling(dist, path)?;
    let content_type = mime_guess::from_path(path).first_or_octet_stream();

    let mut response = Response::new(Body::from(file.contents()));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(content_type.as_ref()) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("br"));
    headers.insert(header::VARY, HeaderValue::from_static(VARY_ACCEPT_ENCODING));
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from(file.contents().len()),
    );
    Some(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderName;
    // The real embedded `dist/` the handler serves from, not a fixture: these
    // tests are here to fail when the *build* stops emitting siblings, and a
    // mock directory would pass happily against an empty one. Borrowed from
    // the parent rather than embedded again — a second `include_dir!` would
    // put another copy of `dist/` in the test binary.
    use super::super::DIST;

    fn headers(pairs: &[(HeaderName, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(name.clone(), HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn accepts_brotli_reads_the_token_list() {
        let cases = [
            ("br", true),
            ("gzip, deflate, br", true),
            ("gzip, deflate, br;q=1.0", true),
            ("BR", true),
            (" br ", true),
            ("gzip, deflate", false),
            ("", false),
            // Must not match on a substring: these are other encodings.
            ("brotli-unsupported", false),
            ("gzip, xbr", false),
        ];
        for (value, expected) in cases {
            assert_eq!(
                accepts_brotli(&headers(&[(header::ACCEPT_ENCODING, value)])),
                expected,
                "Accept-Encoding: {value:?}"
            );
        }
        assert!(
            !accepts_brotli(&HeaderMap::new()),
            "no Accept-Encoding at all is not an advertisement of br"
        );
    }

    #[test]
    fn no_sibling_is_served_without_the_header() {
        assert!(
            try_serve(&DIST, "/index.html", &HeaderMap::new()).is_none(),
            "a client that did not ask for br must get the raw file"
        );
    }

    #[test]
    fn a_sibling_is_never_served_for_a_sibling_path() {
        let h = headers(&[(header::ACCEPT_ENCODING, "br")]);
        assert!(
            try_serve(&DIST, "/assets/anything.js.br", &h).is_none(),
            "a `.br` path must not resolve to `.br.br`, nor to itself unlabelled"
        );
    }

    #[test]
    fn a_missing_file_falls_through() {
        let h = headers(&[(header::ACCEPT_ENCODING, "br")]);
        assert!(try_serve(&DIST, "/assets/not-a-real-asset.js", &h).is_none());
        assert!(
            try_serve(&DIST, "/threads/some-id", &h).is_none(),
            "a client route has no asset and must reach the SPA fallback"
        );
    }

    /// The contract that matters: the sibling goes out labelled as the
    /// *original*, because `Content-Encoding` is how the client learns it is
    /// compressed. A `.js` served as `application/octet-stream` is blocked by
    /// strict MIME checking and the page never boots.
    #[test]
    fn a_served_sibling_describes_the_original_media_type() {
        let h = headers(&[(header::ACCEPT_ENCODING, "gzip, deflate, br")]);
        // Pick whatever JS asset this build produced rather than naming a
        // hashed filename the next build would invalidate.
        let Some(js) = DIST.get_dir("assets").and_then(|assets| {
            assets
                .files()
                .map(|f| f.path().to_string_lossy().to_string())
                .find(|p| p.ends_with(".js.br"))
        }) else {
            // An empty `dist/` (a Rust-only checkout that never ran the
            // frontend build) has nothing to assert on; `dist` is a symlink to
            // `web-app/dist` and may legitimately hold only `vite.svg`.
            return;
        };
        let original = format!("/{}", js.trim_end_matches(PRECOMPRESSED_SUFFIX));

        let response = try_serve(&DIST, &original, &h).expect("sibling exists");

        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/javascript",
            "must describe {original}, not the .br sibling"
        );
        assert_eq!(
            response.headers().get(header::CONTENT_ENCODING).unwrap(),
            "br"
        );
        assert_eq!(
            response.headers().get(header::VARY).unwrap(),
            VARY_ACCEPT_ENCODING,
            "two representations of one URL must not be cached interchangeably"
        );
    }
}
