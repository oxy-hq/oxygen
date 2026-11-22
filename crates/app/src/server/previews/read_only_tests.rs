use super::*;
use axum::http::Method;
use uuid::Uuid;

// The table's own tests (`verdict`, `analytics_run_refusal`) went down with
// it to `oxy-previews` (`crates/previews/src/read_only_tests.rs`); these two
// drive the axum adapter that stays here.

#[tokio::test]
async fn a_refusal_is_a_409_with_the_contracted_body_and_the_preview_header() {
    let pin = PreviewPin {
        branch: "feat/x".into(),
        revision_id: Uuid::nil(),
    };
    let request = Request::builder()
        .method(Method::POST)
        .uri("/agentic-schedules")
        .body(Body::empty())
        .unwrap();
    let response = check(request, &pin).await.expect_err("refused");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response
            .headers()
            .get(super::super::pin::RESPONSE_HEADER)
            .unwrap(),
        "feat/x@00000000-0000-0000-0000-000000000000"
    );
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "preview_read_only");
    assert_eq!(
        json["message"],
        "Creating a schedule isn't available in a preview; merge the branch to run it"
    );
}

#[tokio::test]
async fn an_allowed_run_keeps_its_body() {
    let pin = PreviewPin {
        branch: "feat/x".into(),
        revision_id: Uuid::nil(),
    };
    let body = r#"{"agent_id":"a","question":"q"}"#;
    let request = Request::builder()
        .method(Method::POST)
        .uri("/analytics/runs")
        .body(Body::from(body))
        .unwrap();
    let request = check(request, &pin).await.expect("chat is allowed");
    let bytes = axum::body::to_bytes(request.into_body(), 4096)
        .await
        .unwrap();
    assert_eq!(bytes, body.as_bytes(), "the handler still gets the body");
}

// `every_mutating_route_is_classified_and_every_entry_is_real` checks these
// lists against the generated route table, so it lives beside that table in
// `oxy-route-catalog` (`crates/route-catalog/src/tests/previews_read_only.rs`).
