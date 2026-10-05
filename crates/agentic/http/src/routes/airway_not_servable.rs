//! The one answer every airway route gives when **this node** cannot serve a
//! `pipeline_ref`, and the error→response mapping the reset routes share.
//!
//! Start, single-window backfill, cancel and both resets are `FleetOk`
//! (`airway_router_roles`), so they run on replicas with no working copy. There
//! a pipeline's YAML comes from the compile boundary or not at all, and "not at
//! all" is never the caller's mistake: it answers `503` + `Retry-After`, with
//! the reason in [`HEADER_ERROR_CODE`]. Never `400` — that tells a client to fix
//! a request that is not broken — and never `500`.

use axum::http::{HeaderValue, StatusCode, header::RETRY_AFTER};
use axum::response::{IntoResponse, Response};

use agentic_pipeline::executor::{ResetCursorsError, ResetSchemaError};

/// Response header naming *why* a pipeline ref was not servable here.
///
/// A header rather than a JSON body because the body of these 503s is a
/// plain-text sentence the web app shows a person verbatim; a client that
/// branches reads this instead of parsing prose.
pub const HEADER_ERROR_CODE: &str = "x-oxy-error-code";

/// The compile boundary could not be *asked* (a database blip), or nothing is
/// promoted and this node holds no working copy. Waiting is the whole fix.
pub const CODE_UNAVAILABLE: &str = "airway_unavailable";

/// The promoted revision does not serve the ref and this node holds no working
/// copy. A compile that promotes the ref is the fix; one has been requested
/// when the message says so.
pub const CODE_NEEDS_RECOMPILE: &str = "airway_needs_recompile";

/// Why this node could not serve a pipeline ref. Both are retryable.
#[derive(Debug)]
pub(super) enum NotServable {
    Unavailable(String),
    NeedsRecompile(String),
}

impl NotServable {
    fn code(&self) -> &'static str {
        match self {
            Self::Unavailable(_) => CODE_UNAVAILABLE,
            Self::NeedsRecompile(_) => CODE_NEEDS_RECOMPILE,
        }
    }
}

/// `Retry-After` for every airway 503, in seconds.
///
/// Derived from the executor's defer cadence rather than restated: the two
/// answer the same question ("when is it worth asking again?") for the same
/// condition, and hand-written copies across routes and another crate are that
/// many places for one number to drift.
fn retry_after() -> HeaderValue {
    HeaderValue::from(agentic_pipeline::executor::AIRWAY_UNAVAILABLE_RETRY_SECS)
}

impl IntoResponse for NotServable {
    fn into_response(self) -> Response {
        let code = HeaderValue::from_static(self.code());
        let (Self::Unavailable(message) | Self::NeedsRecompile(message)) = self;
        let mut response = (StatusCode::SERVICE_UNAVAILABLE, message).into_response();
        let headers = response.headers_mut();
        headers.insert(RETRY_AFTER, retry_after());
        headers.insert(HEADER_ERROR_CODE, code);
        response
    }
}

/// `/reset-schema`'s failure, as a response. Caller mistakes → 400; a failed
/// destination drop or state delete → 500; not servable here → 503.
pub(super) fn reset_schema_error_response(e: ResetSchemaError) -> Response {
    match e {
        ResetSchemaError::BadRequest(m) => (StatusCode::BAD_REQUEST, m).into_response(),
        ResetSchemaError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m).into_response(),
        ResetSchemaError::Unavailable(m) => NotServable::Unavailable(m).into_response(),
        ResetSchemaError::NotInRevision(m) => NotServable::NeedsRecompile(m).into_response(),
    }
}

/// A cursor route's failure, as a response — the same mapping as
/// [`reset_schema_error_response`], so a `pipeline_ref` that 503s for the list
/// cannot 400 for the reset that follows it.
///
/// The two `409`s (`Refused`, `PipelineRunning`) carry typed bodies and are
/// answered by `reset_airway_cursors` before it gets here; a read raises
/// neither. They are listed so a new variant breaks the build here, and answer
/// `500` rather than panic if a refactor ever makes one reachable.
pub(super) fn reset_cursors_error_response(e: ResetCursorsError) -> Response {
    match e {
        ResetCursorsError::BadRequest(m) => (StatusCode::BAD_REQUEST, m).into_response(),
        ResetCursorsError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m).into_response(),
        ResetCursorsError::Unavailable(m) => NotServable::Unavailable(m).into_response(),
        ResetCursorsError::NotInRevision(m) => NotServable::NeedsRecompile(m).into_response(),
        e @ (ResetCursorsError::Refused(_) | ResetCursorsError::PipelineRunning { .. }) => {
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_of(response: &Response) -> Option<&str> {
        response
            .headers()
            .get(HEADER_ERROR_CODE)
            .and_then(|v| v.to_str().ok())
    }

    async fn body_of(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        String::from_utf8(bytes.to_vec()).expect("utf-8 body")
    }

    /// Both reasons are the same status and pacing, and differ only in the
    /// code — which is the whole point of having one.
    #[tokio::test]
    async fn not_servable_is_a_retryable_503_that_names_its_reason() {
        for (reason, code) in [
            (NotServable::Unavailable("db blip".into()), CODE_UNAVAILABLE),
            (
                NotServable::NeedsRecompile("not promoted".into()),
                CODE_NEEDS_RECOMPILE,
            ),
        ] {
            let response = reason.into_response();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(code_of(&response), Some(code));
            assert_eq!(
                response.headers().get(RETRY_AFTER).unwrap(),
                &agentic_pipeline::executor::AIRWAY_UNAVAILABLE_RETRY_SECS.to_string(),
                "the route and the executor must agree on when to ask again"
            );
        }
    }

    /// The body stays the plain sentence the web app shows; the code rides the
    /// header and must not leak into it.
    #[tokio::test]
    async fn the_body_is_the_message_and_nothing_else() {
        let response = NotServable::NeedsRecompile("not promoted".into()).into_response();
        assert_eq!(body_of(response).await, "not promoted");
    }

    /// A ref the revision does not serve is retryable on BOTH reset routes.
    /// The cursor one answered 400 for it.
    #[test]
    fn a_reset_for_a_ref_the_revision_does_not_serve_is_503_on_both_routes() {
        let schema = reset_schema_error_response(ResetSchemaError::NotInRevision("gone".into()));
        let cursors = reset_cursors_error_response(ResetCursorsError::NotInRevision("gone".into()));
        for response in [schema, cursors] {
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(code_of(&response), Some(CODE_NEEDS_RECOMPILE));
        }
    }

    #[test]
    fn a_reset_during_a_boundary_blip_is_503_unavailable_on_both_routes() {
        let schema = reset_schema_error_response(ResetSchemaError::Unavailable("blip".into()));
        let cursors = reset_cursors_error_response(ResetCursorsError::Unavailable("blip".into()));
        for response in [schema, cursors] {
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(code_of(&response), Some(CODE_UNAVAILABLE));
        }
    }

    /// The converse: a genuine caller mistake and a genuine server fault keep
    /// their statuses and carry no retry signal.
    #[test]
    fn caller_mistakes_and_server_faults_are_not_dressed_as_retryable() {
        for (response, status) in [
            (
                reset_schema_error_response(ResetSchemaError::BadRequest("bad ref".into())),
                StatusCode::BAD_REQUEST,
            ),
            (
                reset_cursors_error_response(ResetCursorsError::BadRequest("bad ref".into())),
                StatusCode::BAD_REQUEST,
            ),
            (
                reset_schema_error_response(ResetSchemaError::Internal("drop failed".into())),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                reset_cursors_error_response(ResetCursorsError::Internal("write failed".into())),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ] {
            assert_eq!(response.status(), status);
            assert_eq!(code_of(&response), None);
            assert!(response.headers().get(RETRY_AFTER).is_none());
        }
    }
}
