//! The dispatch against a fake GitHub on a loopback port: the request GitHub
//! would get, the retry, and the three outcomes.

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

const TOKEN: &str = "ghp_FakeTokenForDispatchTestsOnly";
const DISPATCHES: &str =
    "/repos/oxy-hq/oxygen-internal/actions/workflows/custom-app-build-checks.yaml/dispatches";

fn config(api: &str, token: Option<&str>, deployment: Option<&str>) -> AppChecksConfig {
    let (api, token, deployment) = (
        api.to_string(),
        token.map(str::to_string),
        deployment.map(str::to_string),
    );
    AppChecksConfig::parse(&move |name| match name {
        "OXY_APP_CHECKS_GITHUB_API" => Some(api.clone()),
        "OXY_APP_CHECKS_GITHUB_TOKEN" => token.clone(),
        "OXY_APP_CHECKS_DEPLOYMENT" => deployment.clone(),
        _ => None,
    })
}

fn inputs() -> RunnerInputs {
    RunnerInputs {
        app_id: Uuid::from_u128(0xa1),
        org_slug: "acme".into(),
        app_slug: "store".into(),
        build_pk: Uuid::from_u128(0xb1),
        attempt: 2,
        environment: "staging".into(),
    }
}

async fn run(config: &AppChecksConfig) -> Result<Dispatched, DispatchFailed> {
    dispatch(&client().unwrap(), config, &inputs(), &[]).await
}

/// A fake GitHub answering every dispatch with `status`.
async fn github(status: u16) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(DISPATCHES))
        .respond_with(ResponseTemplate::new(status))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn the_dispatch_is_the_request_github_expects() {
    let server = github(204).await;
    let config = config(&server.uri(), Some(TOKEN), Some("staging"));
    assert_eq!(run(&config).await, Ok(Dispatched::Started));

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "accepted the first time, sent once");
    let request = &requests[0];
    assert_eq!(request.url.path(), DISPATCHES);
    let header = |name: &str| request.headers.get(name).unwrap().to_str().unwrap();
    assert_eq!(header("authorization"), format!("Bearer {TOKEN}"));
    assert_eq!(header("accept"), "application/vnd.github+json");
    assert_eq!(header("x-github-api-version"), "2022-11-28");
    assert_eq!(header("user-agent"), "oxy-app-checks");
    let sent: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(
        sent,
        json!({
            "ref": "main",
            "inputs": {
                "deployment": "staging",
                "app_id": "00000000-0000-0000-0000-0000000000a1",
                "app": "acme/store",
                "build": "00000000-0000-0000-0000-0000000000b1",
                "attempt": "2",
                "environment": "staging",
            },
        }),
        "every input is a string"
    );
    // A production attempt names production, and nothing else differs.
    let production = RunnerInputs {
        environment: "production".into(),
        ..inputs()
    };
    let cfg = config.dispatch().unwrap();
    assert_eq!(
        body(&cfg, &production)["inputs"]["environment"],
        "production"
    );
    let mut same = body(&cfg, &production);
    same["inputs"]["environment"] = json!("staging");
    assert_eq!(same, sent);
}

#[tokio::test]
async fn a_refusal_is_tried_three_times_then_named_without_the_token() {
    let server = github(401).await;
    let config = config(&server.uri(), Some(TOKEN), Some("dev"));
    let failed = run(&config).await.unwrap_err();
    assert_eq!(failed.to_string(), "GitHub answered 401");
    assert!(!format!("{failed:?}").contains(TOKEN));
    assert_eq!(server.received_requests().await.unwrap().len(), TRIES);
}

#[tokio::test]
async fn a_failure_that_clears_starts_the_runner_on_a_later_try() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(DISPATCHES))
        .respond_with(ResponseTemplate::new(502))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(DISPATCHES))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let config = config(&server.uri(), Some(TOKEN), Some("prod"));
    assert_eq!(run(&config).await, Ok(Dispatched::Started));
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn an_unreachable_github_is_a_failure_that_says_so() {
    // A loopback port nothing listens on: bound, read, released.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let api = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let failed = run(&config(&api, Some(TOKEN), Some("dev")))
        .await
        .unwrap_err();
    assert_eq!(failed.to_string(), "GitHub could not be reached");
}

#[tokio::test]
async fn a_deployment_that_is_not_configured_sends_nothing() {
    let server = github(204).await;
    for (token, deployment, why) in [
        (None, Some("dev"), "OXY_APP_CHECKS_GITHUB_TOKEN is not set"),
        (Some(TOKEN), None, "OXY_APP_CHECKS_DEPLOYMENT is not set"),
    ] {
        let config = config(&server.uri(), token, deployment);
        assert_eq!(
            run(&config).await,
            Ok(Dispatched::NotConfigured(why.to_string()))
        );
    }
    let unknown = config(&server.uri(), Some(TOKEN), Some("production"));
    assert!(matches!(
        run(&unknown).await,
        Ok(Dispatched::NotConfigured(why)) if why.contains("OXY_APP_CHECKS_DEPLOYMENT")
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}
