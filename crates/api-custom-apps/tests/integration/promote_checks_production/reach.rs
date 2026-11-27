//! The checker on production (`Action::AppCheckProduction`): the page, its
//! assets, the beacon and the app's own two reads, and nothing else; and its
//! visits, on any environment, are not usage.

use axum::http::HeaderValue;
use entity::prelude::{CustomAppEvent, CustomAppViewEvent};
use oxy_auth::token::SandboxAgent;
use oxy_custom_apps::authenticate::authenticate_and_authorize;
use oxy_custom_apps::gate::{check_custom_app_gates, check_custom_app_read_gates};
use oxy_custom_apps::{telemetry, usage_queue};
use sea_orm::PaginatorTrait;

use super::*;
use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::invocations;
use crate::promote_rollback::fixture_builds;

/// The checker's credential, addressed to production (no environment named).
const AS_CHECKER: [(&str, &str); 1] = [("x-api-key", TOKEN)];
/// The same, addressed to staging.
const ON_STAGING: [(&str, &str); 2] = [("x-api-key", TOKEN), ("x-oxy-app-env", "staging")];

/// A batch a ready page posts: its ready signal, a pageview, the open it
/// reports itself, and an error with its detail.
const BEACON: &str = r#"{"v":1,"events":[
    {"n":"oxy-app-ready","p":{"how":"auto"}},
    {"n":"oxy-pageview","p":{"path":"/stores","kind":"spa"}},
    {"n":"oxy-view","p":{"referrer":""}},
    {"n":"oxy-error","p":{"counts":{"TypeError":1},"details":[{"message":"x is undefined"}]}}
]}"#;

fn data_headers(app_id: Uuid, extra: &[(&'static str, &'static str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-api-key", HeaderValue::from_static(TOKEN));
    headers.insert("x-oxy-app", app_id.to_string().parse().unwrap());
    for (name, value) in extra {
        headers.insert(*name, HeaderValue::from_static(value));
    }
    headers
}

/// The fixture and the checker. What the checker reaches does not depend on
/// the gate, so it is left off: the tests below publish a second app.
async fn with_the_checker() -> Agent {
    let agent = agent().await;
    with_checker(&agent.t).await;
    agent
}

#[tokio::test]
async fn the_checker_opens_production_and_reads_the_apps_data_and_nothing_else_there() {
    let agent = with_the_checker().await;
    let (production, staging) = fixture_builds(&agent).await;
    let app_id = agent.app.id.to_string();
    let workspace = demo_workspace_id();
    assert_it_opens_production_and_runs_nothing(&agent, production, staging).await;
    assert_the_apps_two_reads_pass(&agent, &app_id, workspace).await;
    assert_every_other_data_door_refuses_it(&agent, &app_id, workspace).await;

    // Production's other surfaces share the resolver and stay closed: they
    // ask what a viewer is asked, which the checker is not.
    let credential = data_headers(agent.app.id, &[]);
    let surface =
        authenticate_and_authorize(&credential, ORG_SLUG, &agent.app.slug, SandboxAgent::Refuse)
            .await;
    assert_eq!(
        surface.err(),
        Some(StatusCode::FORBIDDEN),
        "health, logs, debug"
    );

    assert_the_console_is_not_its_door(&agent, production).await;
}

/// Production's serve tree, as the checker reaches it.
async fn assert_it_opens_production_and_runs_nothing(
    agent: &Agent,
    production: Uuid,
    staging: Uuid,
) {
    // The page, an asset of it, and the beacon: production's, by its build.
    let (status, _, html) = serve(agent, "GET", "", &AS_CHECKER, "").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        html.contains(&format!("\"buildId\":\"{production}\""))
            && html.contains("\"environment\":\"production\""),
        "production's build is served: {html}"
    );
    assert!(
        !html.contains(&staging.to_string()),
        "never staging's build"
    );
    for (method, rest) in [("GET", "__oxy/sw.js"), ("HEAD", ""), ("GET", "index.html")] {
        let (status, _, _) = serve(agent, method, rest, &AS_CHECKER, "").await;
        assert_eq!(status, StatusCode::OK, "{method} {rest}");
    }
    let (status, _, _) = serve(agent, "POST", "__oxy/beacon", &AS_CHECKER, BEACON).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "the ready beacon");

    // Anything but a read is refused before it runs, a function above all.
    for (method, rest) in [
        ("POST", "fn/whoami"),
        ("POST", ""),
        ("PUT", "x"),
        ("DELETE", ""),
    ] {
        let (status, _, _) = serve(agent, method, rest, &AS_CHECKER, "{}").await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {rest} on production"
        );
    }
    let (status, _, _) = serve(agent, "GET", "fn/whoami", &AS_CHECKER, "").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    for function in ["whoami", "smoke"] {
        assert!(
            invocations(&agent.t.db, agent.app.id, function)
                .await
                .is_empty(),
            "no function ran for the checker: {function}"
        );
    }
}

/// The data plane's two reads, as the checker makes them on production.
async fn assert_the_apps_two_reads_pass(agent: &Agent, app_id: &str, workspace: Uuid) {
    // The app's own two reads pass the read gate through the served API: the
    // answers below are the handlers' own, past the gate.
    let named = [("x-api-key", TOKEN), ("x-oxy-app", app_id)];
    let query = format!("/projects/{workspace}/query");
    let (status, body) = send("POST", &query, &named, Some(json!({ "sql": "" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["message"], "`sql` must be non-empty", "past the gate");
    let semantic = format!("/projects/{workspace}/semantic-query");
    let (status, body) = send("POST", &semantic, &named, Some(json!({}))).await;
    assert!(
        !matches!(status, StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED),
        "the semantic read is past the gate: {status} {body}"
    );
    let read = check_custom_app_read_gates(&data_headers(agent.app.id, &[]), workspace).await;
    let ctx = read.unwrap_or_else(|e| panic!("production's read: {}", e.message()));
    assert_eq!(ctx.project_id, workspace);
}

/// The data plane past those two reads, as the checker finds it.
async fn assert_every_other_data_door_refuses_it(agent: &Agent, app_id: &str, workspace: Uuid) {
    let named = [("x-api-key", TOKEN), ("x-oxy-app", app_id)];
    let query = format!("/projects/{workspace}/query");
    // Naming no app, another workspace, a sandbox, or any other data-plane
    // door (an ask, an automation run, the metric tree) is refused.
    let unnamed = [("x-api-key", TOKEN)];
    let (status, _) = send("POST", &query, &unnamed, Some(json!({ "sql": "" }))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "no app named");
    let elsewhere = format!("/projects/{}/query", Uuid::new_v4());
    let (status, _) = send("POST", &elsewhere, &named, Some(json!({ "sql": "" }))).await;
    assert_ne!(status, StatusCode::BAD_REQUEST, "another workspace's data");
    let sandbox = data_headers(agent.app.id, &[("x-oxy-app-env", "dev-a1")]);
    assert!(
        check_custom_app_read_gates(&sandbox, workspace)
            .await
            .is_err()
    );
    let other_door = check_custom_app_gates(&data_headers(agent.app.id, &[]), workspace).await;
    assert_eq!(other_door.err().map(|e| e.status()), Some(403));
    for uri in [
        format!("/projects/{workspace}/procedures/nightly/runs"),
        format!("/projects/{workspace}/agents/default/asks"),
    ] {
        let (status, body) = send("POST", &uri, &named, Some(json!({}))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
    }
}

/// The staff console, as the checker finds it.
async fn assert_the_console_is_not_its_door(agent: &Agent, production: Uuid) {
    // The console, and the checks' own console routes, are not its door.
    let base = checks(agent, production);
    for (method, uri) in [
        ("GET", format!("/customer-apps/{}", agent.app.id)),
        ("GET", format!("/customer-apps/{}/builds", agent.app.id)),
        ("GET", format!("{base}?environment=production")),
        (
            "GET",
            format!("{base}/files/shot.png?environment=production&attempt=1"),
        ),
        ("POST", format!("{base}/changes/seen")),
        ("POST", format!("{base}/rerun")),
        (
            "POST",
            format!(
                "/customer-apps/{}/environments/production/promote",
                agent.app.id
            ),
        ),
        (
            "POST",
            format!(
                "/customer-apps/{}/environments/production/rollback",
                agent.app.id
            ),
        ),
    ] {
        let (status, body) = checker(method, &uri, Some(json!({}))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {body}");
    }
    // Another app's build under this app, and an app that does not exist.
    let other = crate::sandbox_agent_token::fixture::published(&agent.t, "pc-other").await;
    let theirs = build_pk(&agent.t.db, other.id, "agent-prod").await.unwrap();
    let claim = Some(claim_body("production"));
    let (status, _) = checker(
        "POST",
        &format!("{}/claim", checks(agent, theirs)),
        claim.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "another app's build");
    let nowhere = format!(
        "/customer-apps/{}/builds/{production}/checks/claim",
        Uuid::new_v4()
    );
    let (status, _) = checker("POST", &nowhere, claim).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// What a page load and a beacon can write: view rows, event rows, the jobs
/// queued for them, and the records handed to telemetry.
async fn usage(agent: &Agent) -> (u64, u64, u64) {
    let db = &agent.t.db;
    let views = CustomAppViewEvent::find().count(db).await.expect("views");
    let events = CustomAppEvent::find().count(db).await.expect("events");
    (views, events, telemetry::handed_total())
}

/// A checker's page load and beacon, on production and on staging, write
/// nothing: no view, no event row, no queued job, no serve or client record,
/// and no tracking cookie. The same visit by a person writes all of them.
#[tokio::test]
async fn the_checkers_visits_write_no_usage_on_production_or_staging() {
    let agent = with_the_checker().await;
    usage_queue::flush().await;
    let before = usage(&agent).await;

    for (environment, headers) in [
        ("production", &AS_CHECKER[..]),
        ("staging", &ON_STAGING[..]),
    ] {
        let (status, answered, html) = serve(&agent, "GET", "", headers, "").await;
        assert_eq!(status, StatusCode::OK, "{environment}: {html}");
        assert!(
            html.contains(&format!("\"environment\":\"{environment}\"")),
            "{html}"
        );
        assert!(
            answered.get(header::SET_COOKIE).is_none(),
            "{environment}: the checker's page load started a tracking session"
        );
        let (status, _, _) = serve(&agent, "GET", "index.html", headers, "").await;
        assert_eq!(status, StatusCode::OK, "{environment}: an asset");
        let (status, answered, _) = serve(&agent, "POST", "__oxy/beacon", headers, BEACON).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{environment}: the beacon");
        assert!(answered.get(header::SET_COOKIE).is_none(), "{environment}");
        // A request it is refused is not recorded as the app's traffic either.
        let (status, _, _) = serve(&agent, "POST", "fn/whoami", headers, "{}").await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    // A sandbox is not the checker's: refused, and not recorded.
    let sandbox = [("x-api-key", TOKEN), ("x-oxy-app-env", "dev-a1")];
    let (status, _, _) = serve(&agent, "GET", "", &sandbox, "").await;
    assert_ne!(status, StatusCode::OK);

    assert_eq!(usage_queue::flush().await, 0, "a usage job was queued");
    assert_eq!(
        usage(&agent).await,
        before,
        "the checker's visits wrote usage"
    );

    // The same page load and beacon by a person are usage, so the zeros above
    // measure the checker and not a recorder that stopped working.
    let person = [("cookie", agent.cookie.as_str())];
    let (status, answered, html) = serve(&agent, "GET", "", &person, "").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        answered.get(header::SET_COOKIE).is_some(),
        "a person's session"
    );
    let (status, _, _) = serve(&agent, "POST", "__oxy/beacon", &person, BEACON).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        usage_queue::flush().await >= 4,
        "the view, the open and the events"
    );
    let after = usage(&agent).await;
    assert!(after.0 >= before.0 + 2, "views: {before:?} -> {after:?}");
    assert!(after.1 >= before.1 + 3, "events: {before:?} -> {after:?}");
    assert!(after.2 >= before.2 + 4, "records: {before:?} -> {after:?}");
}
