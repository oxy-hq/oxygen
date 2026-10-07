//! What a one-shot promote route answers from an [`AppOpError`]: the coded
//! body for a build a sandbox agent token published, and the bare status for
//! everything else. A handler that keeps only `status` answers a bare `409`
//! with no reason — the partner console's promote did.

use axum::response::IntoResponse;

use super::*;

async fn answered(failure: AppOpError) -> (StatusCode, Vec<u8>) {
    let response = PromoteRefusal::from(failure).into_response();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("read the body");
    (status, bytes.to_vec())
}

#[tokio::test]
async fn a_promote_refused_for_a_tokens_build_keeps_its_body() {
    let refused = AppOpError::agent_built(AgentBuilt {
        build_id: "agent-draft-1".into(),
        environment: Some("staging".into()),
        token_id: Uuid::from_u128(0x70),
        token_name: Some("nightly agent".into()),
        minter: Some("minter@oxy.tech".into()),
    });
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert_eq!(refused.code(), Some("draft_published_by_agent"));
    let message = refused.message.clone();

    let (status, bytes) = answered(refused).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("a JSON body");
    assert_eq!(body["code"], "draft_published_by_agent");
    assert_eq!(body["error"], "draft_published_by_agent");
    assert_eq!(body["message"], message.as_str());
    assert_eq!(body["build_id"], "agent-draft-1");
    assert_eq!(body["environment"], "staging");
    assert_eq!(body["token_name"], "nightly agent");
    assert_eq!(body["minter"], "minter@oxy.tech");
}

#[tokio::test]
async fn every_other_promote_failure_is_the_bare_status_it_was() {
    for failure in [
        AppOpError::not_found(),
        AppOpError::internal(),
        AppOpError::validation_failed("build validation status is 'pending'"),
    ] {
        let expected = failure.status;
        assert_eq!(failure.code(), None);
        let (status, bytes) = answered(failure).await;
        assert_eq!(status, expected);
        assert!(bytes.is_empty(), "{expected}: no body");
    }
}
