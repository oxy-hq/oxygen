use super::*;
use uuid::Uuid;

fn refused(method: Method, path: &str) -> &'static str {
    match verdict(&method, path) {
        Verdict::Refused(what) => what,
        other => panic!("{method} {path} must be refused in a preview, got {other:?}"),
    }
}

fn allowed(method: Method, path: &str) {
    assert_eq!(
        verdict(&method, path),
        Verdict::Allowed,
        "{method} {path} is served in a preview (its writes held) and must stay allowed"
    );
}

#[test]
fn reads_always_pass() {
    for path in [
        "/files/Zm9v",
        "/agentic-schedules",
        "/agentic-schedules/abc",
        "/automations",
        "/analytics/runs/r1/events",
        "/semantic/world-model",
    ] {
        allowed(Method::GET, path);
        allowed(Method::HEAD, path);
    }
}

/// The action endpoints the brief names, by the words the frontend shows.
#[test]
fn the_named_actions_are_refused_with_what_they_are() {
    for (method, path, what) in [
        (
            Method::POST,
            "/agentic-automations/runs",
            "Running an automation",
        ),
        (
            Method::POST,
            "/agentic-workflows/runs",
            "Running an automation",
        ),
        (
            Method::POST,
            "/agentic-workflows/runs/r1/cancel",
            "Running an automation",
        ),
        (
            Method::POST,
            "/agentic-airway/runs",
            "Running an Airway pipeline",
        ),
        (
            Method::POST,
            "/agentic-airway/backfill",
            "Running an Airway pipeline",
        ),
        (Method::POST, "/agentic-schedules", "Creating a schedule"),
        (Method::POST, "/agentic-schedules/", "Creating a schedule"),
        (
            Method::PATCH,
            "/agentic-schedules/s1",
            "Changing a schedule",
        ),
        (
            Method::DELETE,
            "/agentic-schedules/s1",
            "Deleting a schedule",
        ),
        (
            Method::POST,
            "/agentic-schedules/s1/run-now",
            "Running a schedule",
        ),
        (
            Method::POST,
            "/agentic-schedules/s1/backfill",
            "Backfilling a schedule",
        ),
        (
            Method::POST,
            "/semantic/anomalies/scan",
            "Running a monitor scan",
        ),
        (Method::POST, "/files/Zm9v", "Editing files"),
        (Method::POST, "/files/Zm9v/new-file", "Editing files"),
        (Method::DELETE, "/files/Zm9v/delete-file", "Editing files"),
        (Method::PUT, "/files/Zm9v/rename-file", "Editing files"),
        (Method::POST, "/compile", "Compiling the workspace"),
        (
            Method::POST,
            "/push-changes",
            "Changing the workspace's git state",
        ),
    ] {
        assert_eq!(refused(method.clone(), path), what, "{method} {path}");
    }
}

#[test]
fn queries_and_chat_stay_allowed() {
    for path in [
        "/sql/query",
        "/sql/Zm9v",
        "/semantic",
        "/semantic/compile",
        "/semantic/metric-tree/explain",
        "/apps/Zm9v/run",
        "/threads",
        "/threads/t1/stop",
        "/analytics/runs/r1/answer",
    ] {
        allowed(Method::POST, path);
    }
}

#[test]
fn a_mutating_route_on_neither_list_is_refused() {
    assert_eq!(refused(Method::POST, "/brand-new-action"), UNLISTED);
    assert_eq!(refused(Method::DELETE, "/semantic"), UNLISTED);
}

#[test]
fn a_run_body_decides_between_chat_and_the_builder() {
    assert_eq!(
        verdict(&Method::POST, "/analytics/runs"),
        Verdict::InspectRunBody
    );
    assert_eq!(
        analytics_run_refusal(br#"{"agent_id":"a","question":"q"}"#),
        None
    );
    assert_eq!(
        analytics_run_refusal(br#"{"agent_id":"a","question":"q","domain":"analytics"}"#),
        None
    );
    assert_eq!(
        analytics_run_refusal(br#"{"agent_id":"a","question":"q","domain":"builder"}"#),
        Some("Building with the Builder Agent")
    );
    assert_eq!(
        analytics_run_refusal(br#"{"agent_id":"a","question":"q","onboarding_context":{}}"#),
        Some("Onboarding")
    );
    assert_eq!(analytics_run_refusal(b"not json"), None);
}

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

/// Relative to the workspace root, as the middleware sees it. The previews API
/// itself is mounted beside the workspace tree, outside `workspace_middleware`,
/// so the guard never sees it.
fn workspace_relative(path: &str) -> Option<&str> {
    path.strip_prefix("/api/{workspace_id}")
        .or_else(|| path.strip_prefix("/external/api/{workspace_id}"))
        .filter(|p| !p.starts_with("/previews"))
}

fn is_mutating(method: &str) -> bool {
    !matches!(method, "GET" | "HEAD" | "OPTIONS")
}

/// Every mutating route the workspace middleware guards is on one of the two
/// lists — so a new one is a decision, not a default — and every entry on the
/// lists names a route that exists, so the lists cannot rot into fiction.
#[test]
fn every_mutating_route_is_classified_and_every_entry_is_real() {
    let routes: Vec<(&str, &str)> = crate::server::route_catalog::routes()
        .iter()
        .filter(|r| is_mutating(r.method))
        .filter_map(|r| workspace_relative(r.path).map(|p| (r.method, p)))
        .collect();
    assert!(
        routes.len() > 100,
        "the catalog should list the workspace surface ({} found)",
        routes.len()
    );

    let listed = |method: &str, path: &str| {
        let m = if method == "ANY" { "POST" } else { method };
        (m == "POST" && path == "/analytics/runs")
            || ALLOWED
                .iter()
                .any(|(am, p)| *am == m && pattern_matches(p, path))
            || REFUSED
                .iter()
                .any(|(rm, p, _)| (*rm == "*" || *rm == m) && pattern_matches(p, path))
    };
    let unclassified: Vec<String> = routes
        .iter()
        .filter(|(m, p)| !listed(m, p))
        .map(|(m, p)| format!("{m} {p}"))
        .collect();
    assert!(
        unclassified.is_empty(),
        "these mutating routes are on neither preview list — put each on ALLOWED \
         (what it executes goes through `request_hold`, so its writes are held) or \
         REFUSED (it changes something the execution layer cannot hold):\n  {}",
        unclassified.join("\n  ")
    );

    let hits = |method: &str, pattern: &str| {
        routes.iter().any(|(m, p)| {
            (method == "*" || *m == method || *m == "ANY") && pattern_matches(pattern, p)
        })
    };
    let stale: Vec<String> = ALLOWED
        .iter()
        .map(|(m, p)| (*m, *p))
        .chain(REFUSED.iter().map(|(m, p, _)| (*m, *p)))
        .filter(|(m, p)| !hits(m, p))
        .map(|(m, p)| format!("{m} {p}"))
        .collect();
    assert!(
        stale.is_empty(),
        "these preview-list entries match no mounted route:\n  {}",
        stale.join("\n  ")
    );
}
