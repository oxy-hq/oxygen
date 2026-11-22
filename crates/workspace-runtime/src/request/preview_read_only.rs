//! The axum adapter of the preview read-only guard: what the workspace
//! middleware calls on a request pinned to a preview.
//!
//! The table — which mutating routes a preview serves ([`ALLOWED`]), which it
//! refuses and with what sentence ([`REFUSED`]), and the verdict on one
//! request — is `oxy_previews::read_only` since 2026-11-22 (the decompose's
//! P2), re-exported here. What stays is axum-shaped:
//! [`check`] buffers a run's body to read what it asks for, and [`refusal`]
//! is the `409 preview_read_only` response. It went down with the workspace
//! middleware into `oxy-workspace-runtime` (decompose W7, 2026-11-22), and
//! oxy-app re-exports it at its old path, `server::previews::read_only`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
pub use oxy_previews::read_only::*;

use oxy_previews::pin::PreviewPin;

/// Check a request pinned to `pin`. `Ok` hands the request back (its body
/// re-assembled if it had to be read); `Err` is the 409 to return instead.
pub async fn check(request: Request<Body>, pin: &PreviewPin) -> Result<Request<Body>, Response> {
    match verdict(request.method(), request.uri().path()) {
        Verdict::Allowed => Ok(request),
        Verdict::Refused(what) => Err(refusal(what, pin)),
        Verdict::InspectRunBody => {
            let (parts, body) = request.into_parts();
            let Ok(bytes) = axum::body::to_bytes(body, RUN_BODY_LIMIT).await else {
                return Err(refusal(UNLISTED, pin));
            };
            match analytics_run_refusal(&bytes) {
                Some(what) => Err(refusal(what, pin)),
                None => Ok(Request::from_parts(parts, Body::from(bytes))),
            }
        }
    }
}

/// `409 {"code":"preview_read_only","message":"<what> isn't available in a
/// preview; merge the branch to run it"}`, stamped like any preview response.
pub fn refusal(what: &str, pin: &PreviewPin) -> Response {
    let mut response = (
        StatusCode::CONFLICT,
        axum::Json(serde_json::json!({
            "code": "preview_read_only",
            "message": format!("{what} isn't available in a preview; merge the branch to run it"),
        })),
    )
        .into_response();
    pin.stamp(&mut response);
    response
}

#[cfg(test)]
#[path = "preview_read_only_tests.rs"]
mod tests;
