//! Failure modes of the IDE cohort handler, and the `{ message, code }`
//! envelope they answer with.
//!
//! The envelope is deliberately the same one `projects::cohort` produces via
//! `semantic_boundary::err_with_code`. A client that learned to branch on
//! `code == "cohort_not_resolved"` against one surface must not have to learn
//! a second dialect for the other — and the World Model panel does exactly
//! that, to tell "this measure declares no `default_cohort:`" (an authoring
//! gap, rendered as an honest empty state) apart from a real failure.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// The error body both cohort surfaces answer with.
#[derive(Debug, Serialize, PartialEq)]
pub struct CohortErrorBody {
    pub message: String,
    /// Stable, machine-readable. Never localise or reword it — a client
    /// branches on this.
    pub code: &'static str,
}

/// Everything the IDE cohort handler can fail with.
///
/// Every variant carries the code its customer-app counterpart already
/// returns for the same condition, so the two surfaces stay string-for-string
/// comparable.
#[derive(Debug)]
pub enum CohortError {
    /// No semantic model is reachable on this node: no compiled revision and
    /// no working copy to fall back to. Retryable — a compile is enqueued on
    /// the way out — so it must not be flattened into a generic 500.
    ScanUnavailable(String),
    LayerLoad(String),
    InvalidStatistic(String),
    /// No cohort was named and the measure declares no `default_cohort:`, or
    /// the named cohort belongs to a different entity. **Not a server fault**:
    /// it is what a workspace that has not authored `cohorts:` yet gets, which
    /// is why it has a code of its own rather than sharing a generic 400.
    CohortNotResolved(String),
    /// `augment_layer_for_cohort` refused, so the truncation guard would have
    /// been silently inert.
    AugmentFailed(String),
    EngineBuild(String),
    /// airlayer refused or failed the resolution itself.
    Failed(String),
    Panicked(String),
    Timeout,
}

impl CohortError {
    /// The stable `code` a client may branch on.
    pub fn code(&self) -> &'static str {
        match self {
            CohortError::ScanUnavailable(_) => "semantic_needs_recompile",
            CohortError::LayerLoad(_) => "semantic_layer_load_failed",
            CohortError::InvalidStatistic(_) => "invalid_statistic",
            CohortError::CohortNotResolved(_) => "cohort_not_resolved",
            CohortError::AugmentFailed(_) => "cohort_augment_failed",
            CohortError::EngineBuild(_) => "semantic_engine_build_failed",
            CohortError::Failed(_) => "cohort_failed",
            CohortError::Panicked(_) => "cohort_panicked",
            CohortError::Timeout => "cohort_timeout",
        }
    }

    /// Status and client-facing message. Kept beside [`CohortError::code`] so
    /// a new variant cannot acquire a code without acquiring a status.
    pub fn parts(&self) -> (StatusCode, String) {
        match self {
            CohortError::ScanUnavailable(m) => (StatusCode::SERVICE_UNAVAILABLE, m.clone()),
            CohortError::LayerLoad(e) => {
                tracing::error!("cohort layer load failed: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to load semantic model".to_string(),
                )
            }
            CohortError::InvalidStatistic(m)
            | CohortError::CohortNotResolved(m)
            | CohortError::AugmentFailed(m)
            | CohortError::EngineBuild(m) => (StatusCode::BAD_REQUEST, m.clone()),
            // A caller error — an unresolvable measure, a period the warehouse
            // has no rows for — far more often than a server one, and the
            // message is the only thing that says which. So it goes back
            // verbatim, matching the customer-app surface rather than being
            // swallowed into a generic 500.
            CohortError::Failed(m) => (StatusCode::BAD_REQUEST, m.clone()),
            CohortError::Panicked(e) => {
                tracing::error!("cohort task panicked: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Cohort resolution failed".to_string(),
                )
            }
            CohortError::Timeout => (
                StatusCode::GATEWAY_TIMEOUT,
                "cohort resolution timed out".to_string(),
            ),
        }
    }
}

impl IntoResponse for CohortError {
    fn into_response(self) -> Response {
        let code = self.code();
        let (status, message) = self.parts();
        let body = Json(CohortErrorBody { message, code });
        if status == StatusCode::SERVICE_UNAVAILABLE {
            // A compile is enqueued on the way out of `resolve_query_scan_source`,
            // so give the client a concrete interval instead of leaving the
            // retry cadence to guesswork — same as the metric-tree surface.
            return (status, [(axum::http::header::RETRY_AFTER, "5")], body).into_response();
        }
        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The condition the panel exists to render honestly, and the one a
    /// workspace that has never authored `cohorts:` hits on every measure.
    #[test]
    fn a_measure_with_no_default_cohort_is_a_400_the_ui_can_recognise() {
        let e = CohortError::CohortNotResolved(
            "no cohort given and measure `sales.wage_pct` declares no `default_cohort:`"
                .to_string(),
        );
        let (status, msg) = e.parts();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(e.code(), "cohort_not_resolved");
        // The message names the measure — it is what the panel shows a
        // modeller who has not authored a cohort yet.
        assert!(msg.contains("default_cohort"), "{msg}");
    }

    /// Every code this surface can emit is one the customer-app twin already
    /// emits for the same condition. Pinned as a list so adding a variant
    /// forces the question "what does `projects::cohort` call this?".
    #[test]
    fn every_error_code_matches_the_customer_app_surface() {
        let cases: Vec<(CohortError, &str, StatusCode)> = vec![
            (
                CohortError::InvalidStatistic("x".into()),
                "invalid_statistic",
                StatusCode::BAD_REQUEST,
            ),
            (
                CohortError::CohortNotResolved("x".into()),
                "cohort_not_resolved",
                StatusCode::BAD_REQUEST,
            ),
            (
                CohortError::AugmentFailed("x".into()),
                "cohort_augment_failed",
                StatusCode::BAD_REQUEST,
            ),
            (
                CohortError::EngineBuild("x".into()),
                "semantic_engine_build_failed",
                StatusCode::BAD_REQUEST,
            ),
            (
                CohortError::Failed("x".into()),
                "cohort_failed",
                StatusCode::BAD_REQUEST,
            ),
            (
                CohortError::Panicked("x".into()),
                "cohort_panicked",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                CohortError::Timeout,
                "cohort_timeout",
                StatusCode::GATEWAY_TIMEOUT,
            ),
            (
                CohortError::ScanUnavailable("x".into()),
                "semantic_needs_recompile",
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                CohortError::LayerLoad("x".into()),
                "semantic_layer_load_failed",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];
        for (err, code, status) in cases {
            assert_eq!(err.code(), code, "{err:?}");
            assert_eq!(err.parts().0, status, "{err:?}");
        }
    }

    /// The body a client parses: `{ message, code }`, both always present.
    #[tokio::test]
    async fn the_error_body_carries_message_and_code() {
        let response = CohortError::CohortNotResolved("no cohort".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["code"], serde_json::json!("cohort_not_resolved"));
        assert_eq!(json["message"], serde_json::json!("no cohort"));
    }

    /// A workspace whose semantic model is not compiled yet is retryable, and
    /// the client is told how long to wait rather than left to guess — the
    /// same contract the metric-tree surface answers with.
    #[tokio::test]
    async fn a_missing_compiled_model_answers_503_with_retry_after() {
        let response = CohortError::ScanUnavailable("not compiled".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("5")
        );
    }
}
