use super::*;
use axum::http::StatusCode;
use oxy::database::client::DbFailure;
use oxy_custom_apps::gate::GateError;
use oxy_custom_apps::staging_pin::PinNotReady;
use serde_json::json;

fn pin_not_ready() -> PinNotReady {
    PinNotReady::branch("staging", "feat-x".into(), Some("abc123".into()))
}

/// The world-model and cohort twins answer a gate refusal themselves (a
/// surface may not import another), so their mapping is pinned here with the
/// same cases as `oxy-api-custom-apps`' test of the same name: the decision
/// half states a status, a message and a code as plain values, and the
/// response must put exactly those on the wire, in the `{ message, code? }`
/// envelope the SDK reads.
#[tokio::test]
async fn a_gate_error_becomes_the_response_it_always_was() {
    let unreachable = DbFailure::Unreachable.code();
    let cases = [
        (
            GateError::UserNotFound,
            StatusCode::UNAUTHORIZED,
            json!({ "message": "user not found" }),
        ),
        (
            GateError::UserLookupFailed { code: unreachable },
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "message": "user lookup failed", "code": "database_unreachable" }),
        ),
        (
            GateError::ProjectNotFound,
            StatusCode::NOT_FOUND,
            json!({ "message": "project not found" }),
        ),
        (
            GateError::WorkspaceLookupFailed {
                code: DbFailure::Query.code(),
            },
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "message": "workspace lookup failed", "code": "database_query_failed" }),
        ),
        (
            GateError::NoOwningOrganization,
            StatusCode::FORBIDDEN,
            json!({ "message": "workspace has no owning organization" }),
        ),
        (
            GateError::MembershipCheckFailed { code: unreachable },
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "message": "org membership check failed", "code": "database_unreachable" }),
        ),
        (
            GateError::SemanticBranchNotReady(pin_not_ready()),
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "error": "semantic_branch_not_ready",
                "environment": "staging",
                "message": pin_not_ready().fix(),
                "git_sha": "abc123",
                "fix": pin_not_ready().fix(),
            }),
        ),
    ];
    for (error, status, body) in cases {
        let response = gate_error_response(&error);
        assert_eq!(response.status(), status, "{error:?}");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let sent: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(sent, body, "{error:?}");
    }
}
