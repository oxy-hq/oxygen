//! The id filters `/logs` and `/errors` take from the query string.
//!
//! Every id these reads narrow by is a UUID the server minted — an invocation
//! id, an `x-oxy-request-id`, a build id — so a value that is not one can
//! match no row. It is refused here with a `400` instead of being handed to
//! the store, where a backslash once ended the literal it was written into
//! and let the rest of the value run as SQL. The store binds what it is given
//! whatever this module decides; this is the route saying what the parameter
//! *is*, so the answer to a typo is "that is not an id" and not an empty list.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use super::{ErrorQuery, LogQuery};

/// A filter that is not the id its parameter names.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct InvalidFilter {
    parameter: &'static str,
}

impl IntoResponse for InvalidFilter {
    fn into_response(self) -> Response {
        let message = format!("{} must be a UUID", self.parameter);
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid_filter", "message": message })),
        )
            .into_response()
    }
}

/// `raw` as the store compares it: absent when the filter is missing or
/// blank, otherwise the hyphenated lower-case form every writer stores — so a
/// braced or upper-case spelling of a real id still finds its rows.
fn uuid_filter(
    parameter: &'static str,
    raw: Option<&str>,
) -> Result<Option<String>, InvalidFilter> {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    Uuid::parse_str(raw)
        .map(|id| Some(id.to_string()))
        .map_err(|_| InvalidFilter { parameter })
}

impl LogQuery {
    /// The query with both id filters checked and normalized, or the first
    /// one that is not an id.
    pub(super) fn with_valid_ids(mut self) -> Result<Self, InvalidFilter> {
        self.invocation_id = uuid_filter("invocation_id", self.invocation_id.as_deref())?;
        self.request_id = uuid_filter("request_id", self.request_id.as_deref())?;
        Ok(self)
    }
}

impl ErrorQuery {
    /// The query with its build filter checked and normalized.
    pub(super) fn with_valid_ids(mut self) -> Result<Self, InvalidFilter> {
        self.build_id = uuid_filter("build_id", self.build_id.as_deref())?;
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0b7a1c2e-5d3f-4a6b-8c9d-0e1f2a3b4c5d";

    fn log_query(invocation_id: Option<&str>, request_id: Option<&str>) -> LogQuery {
        LogQuery {
            invocation_id: invocation_id.map(str::to_string),
            request_id: request_id.map(str::to_string),
            ..serde_json::from_str("{}").expect("an empty log query")
        }
    }

    /// The values that once reached the store's SQL: a lone backslash, the
    /// payload it opens the door for, and plain non-ids.
    const NOT_IDS: &[&str] = &[
        "\\",
        " OR 1=1 -- ",
        "\\' OR 1=1 -- ",
        "' OR '1'='1",
        "not-a-uuid",
        "0b7a1c2e-5d3f-4a6b-8c9d-0e1f2a3b4c5d'",
        "0b7a1c2e-5d3f-4a6b-8c9d-0e1f2a3b4c5d\\",
    ];

    #[test]
    fn a_log_filter_that_is_not_an_id_is_refused_and_names_its_parameter() {
        for raw in NOT_IDS {
            assert_eq!(
                log_query(Some(raw), None).with_valid_ids().err(),
                Some(InvalidFilter {
                    parameter: "invocation_id"
                }),
                "{raw:?}"
            );
            assert_eq!(
                log_query(Some(ID), Some(raw)).with_valid_ids().err(),
                Some(InvalidFilter {
                    parameter: "request_id"
                }),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn a_build_filter_that_is_not_an_id_is_refused() {
        for raw in NOT_IDS {
            let query = ErrorQuery {
                hours: None,
                limit: None,
                build_id: Some(raw.to_string()),
            };
            assert_eq!(
                query.with_valid_ids().err(),
                Some(InvalidFilter {
                    parameter: "build_id"
                }),
                "{raw:?}"
            );
        }
    }

    /// Absent and blank are "no filter", as before; an id is passed on in the
    /// one spelling the writers store.
    #[test]
    fn an_absent_filter_stays_absent_and_an_id_is_normalized() {
        for blank in [None, Some(""), Some("  ")] {
            let query = log_query(blank, blank).with_valid_ids().expect("no filter");
            assert_eq!((query.invocation_id, query.request_id), (None, None));
        }
        let spelled = format!("{{{}}}", ID.to_uppercase());
        let query = log_query(Some(&spelled), Some(ID))
            .with_valid_ids()
            .expect("two ids");
        assert_eq!(query.invocation_id.as_deref(), Some(ID));
        assert_eq!(query.request_id.as_deref(), Some(ID));
    }

    #[test]
    fn the_refusal_is_a_400_with_a_machine_readable_code() {
        let response = InvalidFilter {
            parameter: "invocation_id",
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
