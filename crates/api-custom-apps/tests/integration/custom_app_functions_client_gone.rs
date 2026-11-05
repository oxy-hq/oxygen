//! A route caller that hangs up mid-run.
//!
//! The run is awaited inside the HTTP handler and its row is settled after the
//! isolate returns, so when the caller leaves (a reload, a navigation) hyper
//! drops the handler future and that code never runs. The row used to stay
//! `running` until the lazy reaper turned it into a `timeout` — a paged
//! failure reading "instance died or detached" — fifteen minutes later. It must
//! settle as `cancelled`, promptly, with no failure fingerprint.

use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use entity::app_function_invocations;
use serde_json::json;
use tower::ServiceExt;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    FunctionSpec, ORG_SLUG, call_function_with, invocations, publish_app, seeded_tenant,
    serve_router,
};

const APP: &str = "fn-client-gone";

/// Spins far longer than the test waits, so the only way it ends is the
/// caller leaving. No `setTimeout` in the sandbox, so a compute loop; that is
/// also the shape `terminate_execution` exists to interrupt.
const SPIN_JS: &str = r#"
export default async () => {
  const until = Date.now() + 20000;
  while (Date.now() < until) {}
  return Response.json({ finished: true });
};
"#;

fn spin() -> Vec<FunctionSpec> {
    vec![FunctionSpec {
        name: "spin",
        manifest: json!({ "route": true }),
        js: SPIN_JS,
    }]
}

/// Call `spin` and hang up two seconds in: `timeout` drops the handler future
/// exactly as hyper does when the connection closes.
async fn call_and_hang_up(headers: &[(&str, &str)]) {
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/customer-apps/{ORG_SLUG}/{APP}/fn/spin"))
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request.body(Body::from("{}")).expect("request");
    let gave_up =
        tokio::time::timeout(Duration::from_secs(2), serve_router().oneshot(request)).await;
    assert!(
        gave_up.is_err(),
        "the function must still be running when the caller leaves"
    );
}

#[tokio::test]
async fn a_route_call_whose_caller_hangs_up_settles_cancelled_not_running() {
    let t = seeded_tenant().await;
    let published = publish_app(&t, APP, demo_workspace_id(), &spin()).await;

    call_and_hang_up(&[]).await;

    let row = settled_row(&t.db, published.app_id).await;
    assert_eq!(row.status, "cancelled", "row: {row:?}");
    assert_eq!(
        row.error.as_deref(),
        Some("client disconnected before the function finished")
    );
    assert_eq!(
        row.failure_fingerprint, None,
        "a caller leaving is not the function failing, so it pages nobody"
    );
    let ran_ms = row
        .duration_ms
        .expect("the abandoned run records how long it ran");
    assert!((1_500..20_000).contains(&ran_ms), "duration {ran_ms}ms");
}

/// A keyed call is the exception: the key exists so a client that lost its
/// connection can retry safely. Settled `cancelled`, the row would be
/// reclaimed and the function run a second time; left `running`, the retry is
/// refused as in progress, and nothing runs twice.
#[tokio::test]
async fn a_keyed_call_whose_caller_hangs_up_still_refuses_its_retry() {
    let t = seeded_tenant().await;
    let published = publish_app(&t, APP, demo_workspace_id(), &spin()).await;
    let key = [("idempotency-key", "client-gone-retry")];

    call_and_hang_up(&key).await;
    // Past the point the keyless row would have settled.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let retry = call_function_with(ORG_SLUG, APP, "spin", json!({}), &key).await;
    assert_eq!(
        retry.status,
        axum::http::StatusCode::CONFLICT,
        "the retry is refused, not run again: {}",
        retry.raw
    );
    assert!(
        retry.raw.contains("IdempotencyKeyInProgress"),
        "{}",
        retry.raw
    );
    let rows = invocations(&t.db, published.app_id, "spin").await;
    let seen: Vec<_> = rows.iter().map(|r| r.status.as_str()).collect();
    assert_eq!(seen, vec!["running"], "one run, still guarding its key");
}

/// The invocation row once it has left `running`. The guard settles it on a
/// spawned task, so poll briefly rather than read once.
async fn settled_row(
    db: &sea_orm::DatabaseConnection,
    app_id: uuid::Uuid,
) -> app_function_invocations::Model {
    for _ in 0..50 {
        let rows = invocations(db, app_id, "spin").await;
        assert_eq!(rows.len(), 1, "one call, one row: {rows:?}");
        if rows[0].status != "running" {
            return rows[0].clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the abandoned invocation was left `running` for the reaper");
}
