//! The error type every anomaly handler returns, and how each variant reaches
//! the client. Kept apart from the handlers because all four of them — `list`,
//! `status`, `explain`, `scan` — use it and none owns it. (`cap` is the fifth
//! module in the split and the one that doesn't: it trims rows already in hand
//! and cannot fail.)

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use oxy_metric_monitoring as monitoring;

use super::list::MAX_OFFSET;
use super::status::MAX_BULK_IDS;

#[derive(Debug)]
pub enum AnomalyError {
    Db(sea_orm::DbErr),
    Scan(monitoring::ScanError),
    NotFound,
    BadStatus(String),
    /// A malformed request that is not about `status` — a bad `as_of`, say.
    /// Exists because routing those through [`AnomalyError::BadStatus`] told
    /// the caller their *status* was invalid while quoting a date, pointing
    /// whoever was debugging at the wrong field entirely.
    BadRequest(String),
    TooManyIds(usize),
    /// The row count that was refused, and the ceiling it was refused
    /// against — carried rather than read off the constant, since
    /// `apply_status_bulk_capped` takes the ceiling as a parameter and a test
    /// driving a different one would otherwise be told the wrong limit.
    TooManyRows {
        rows: u64,
        limit: u64,
    },
    OffsetTooDeep(u64),
    /// The body named a monitor the file does not have — or there is no file.
    NoSuchMonitor,
    /// Something this request needs has not been compiled, or is not on this
    /// node yet. Retryable, and said as such.
    Unavailable(String),
    /// The work outran the handler's own budget, in seconds.
    TimedOut(u64),
    Internal(String),
}

impl IntoResponse for AnomalyError {
    fn into_response(self) -> Response {
        match self {
            AnomalyError::Db(e) => {
                tracing::error!("metric_anomalies db error: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
            }
            AnomalyError::Scan(e) => {
                tracing::error!("metric_anomalies scan error: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("scan failed: {e}"),
                )
                    .into_response()
            }
            AnomalyError::NotFound => (StatusCode::NOT_FOUND, "anomaly not found").into_response(),
            AnomalyError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
            AnomalyError::BadStatus(s) => (
                StatusCode::BAD_REQUEST,
                format!("invalid status '{s}' (expected: new | acknowledged | dismissed)"),
            )
                .into_response(),
            AnomalyError::TooManyIds(n) => (
                StatusCode::BAD_REQUEST,
                format!("too many ids ({n}); at most {MAX_BULK_IDS} per bulk status update"),
            )
                .into_response(),
            // Advice split by what the caller can actually do about it. A
            // selection of several events can be halved and repeated; a single
            // event over the cap is one ~20 000-bucket chain, and telling
            // whoever sent it to "narrow it" names no smaller request that
            // exists.
            AnomalyError::TooManyRows { rows, limit } => (
                StatusCode::BAD_REQUEST,
                format!(
                    "that selection covers {rows} bucket{}; at most {limit} per bulk \
                     status update — send fewer anomalies per call, or act on this \
                     one's buckets individually via `ids`",
                    if rows == 1 { "" } else { "s" }
                ),
            )
                .into_response(),
            AnomalyError::OffsetTooDeep(n) => (
                StatusCode::BAD_REQUEST,
                format!("offset {n} is past the maximum depth of {MAX_OFFSET}"),
            )
                .into_response(),
            AnomalyError::NoSuchMonitor => (
                StatusCode::NOT_FOUND,
                "that monitor is not in .monitor.yml — the file may have changed; reload",
            )
                .into_response(),
            AnomalyError::Unavailable(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                [(axum::http::header::RETRY_AFTER, "5")],
                msg,
            )
                .into_response(),
            AnomalyError::TimedOut(secs) => (
                StatusCode::GATEWAY_TIMEOUT,
                format!(
                    "the warehouse did not answer within {secs}s; nothing was written, and the \
                     same query would run in a scan"
                ),
            )
                .into_response(),
            AnomalyError::Internal(msg) => {
                tracing::error!("metric_anomalies internal error: {msg}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
            }
        }
    }
}

impl From<sea_orm::DbErr> for AnomalyError {
    fn from(e: sea_orm::DbErr) -> Self {
        AnomalyError::Db(e)
    }
}
