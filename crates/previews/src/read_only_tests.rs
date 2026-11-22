use super::*;

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
        (Method::POST, "/compile/staging", "Compiling the workspace"),
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
        // The monitor dry run writes nothing; the scan beside it is refused.
        "/semantic/anomalies/preview",
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
