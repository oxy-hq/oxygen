//! Starting the runner: one GitHub `workflow_dispatch` per attempt.
//!
//! `POST {api}/repos/{repo}/actions/workflows/{workflow}/dispatches` with the
//! ref to run from and six string inputs (GitHub takes inputs as strings):
//! the deployment, the app's id, `<org_slug>/<app_slug>`, the build's row id,
//! the attempt and its environment (`staging` or `production`). The workflow
//! does the rest: it signs in as the platform checker and runs `oxyc checks
//! run … --environment <e> --report` against this deployment.
//!
//! Three outcomes ([`dispatch`]):
//!
//! - **started**: GitHub accepted the dispatch (`204`);
//! - **not configured**: this deployment holds no GitHub token or does not
//!   say which deployment it is, so nothing is sent
//!   (`oxy_app_core::config::AppChecksConfig::dispatch`);
//! - **failed**: GitHub refused or could not be reached, [`TRIES`] times.
//!
//! The token goes in the `Authorization` header and nowhere else: never in a
//! log line, a summary or an error.

use std::time::Duration;

use oxy_app_core::config::AppChecksConfig;
use oxy_app_core::config::app_checks::DispatchConfig;
use serde_json::{Value, json};
use uuid::Uuid;

/// How many times a dispatch is sent before it is given up.
pub const TRIES: usize = 3;
/// The waits between tries.
pub const BACKOFF: [Duration; TRIES - 1] = [Duration::from_secs(1), Duration::from_secs(3)];
/// How long one request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

const USER_AGENT: &str = "oxy-app-checks";
const GITHUB_API_VERSION: &str = "2022-11-28";

/// What the workflow is told to check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerInputs {
    pub app_id: Uuid,
    pub org_slug: String,
    pub app_slug: String,
    /// The build's row id (`app_builds.id`).
    pub build_pk: Uuid,
    pub attempt: i32,
    /// The attempt's environment, as stored: `staging` or `production`.
    pub environment: String,
}

/// A dispatch that did not fail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dispatched {
    Started,
    /// Nothing was sent, and why.
    NotConfigured(String),
}

/// Why no runner could be started, in words fit for a check's summary:
/// `GitHub answered 401`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DispatchFailed(pub String);

/// The dispatch's URL.
pub fn url(cfg: &DispatchConfig<'_>) -> String {
    format!(
        "{}/repos/{}/actions/workflows/{}/dispatches",
        cfg.api, cfg.repo, cfg.workflow
    )
}

/// The dispatch's body.
pub fn body(cfg: &DispatchConfig<'_>, inputs: &RunnerInputs) -> Value {
    json!({
        "ref": cfg.git_ref,
        "inputs": {
            "deployment": cfg.deployment,
            "app_id": inputs.app_id.to_string(),
            "app": format!("{}/{}", inputs.org_slug, inputs.app_slug),
            "build": inputs.build_pk.to_string(),
            "attempt": inputs.attempt.to_string(),
            "environment": inputs.environment,
        },
    })
}

/// The client every dispatch uses: the platform's shared builder
/// (`oxy_shared::http`), with this request's own timeout. Each request still
/// sends GitHub its own `User-Agent` ([`send`]).
pub fn client() -> Result<reqwest::Client, DispatchFailed> {
    oxy_shared::http::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| DispatchFailed(format!("no HTTP client: {e}")))
}

/// One try. `Err` is what GitHub answered or why it could not be reached,
/// without the URL's query or any header.
async fn send(
    client: &reqwest::Client,
    cfg: &DispatchConfig<'_>,
    inputs: &RunnerInputs,
) -> Result<(), String> {
    let sent = client
        .post(url(cfg))
        .bearer_auth(cfg.token)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .json(&body(cfg, inputs))
        .send()
        .await;
    match sent {
        Ok(response) if response.status().is_success() => Ok(()),
        Ok(response) => Err(format!("GitHub answered {}", response.status().as_u16())),
        Err(e) if e.is_timeout() => Err("GitHub did not answer in time".to_string()),
        Err(e) if e.is_connect() => Err("GitHub could not be reached".to_string()),
        Err(e) => Err(format!(
            "the request to GitHub failed ({})",
            e.without_url()
        )),
    }
}

/// Start the runner for `inputs`: up to [`TRIES`] sends, waiting `backoff[n]`
/// after the n-th failure (a missing entry is no wait).
pub async fn dispatch(
    client: &reqwest::Client,
    config: &AppChecksConfig,
    inputs: &RunnerInputs,
    backoff: &[Duration],
) -> Result<Dispatched, DispatchFailed> {
    let cfg = match config.dispatch() {
        Ok(cfg) => cfg,
        Err(why) => return Ok(Dispatched::NotConfigured(why)),
    };
    let mut last = String::new();
    for attempt in 0..TRIES {
        match send(client, &cfg, inputs).await {
            Ok(()) => return Ok(Dispatched::Started),
            Err(why) => {
                tracing::warn!(app_id = %inputs.app_id, build_pk = %inputs.build_pk,
                    try_number = attempt + 1, "build checks: the dispatch failed: {why}");
                last = why;
            }
        }
        if attempt + 1 < TRIES {
            tokio::time::sleep(backoff.get(attempt).copied().unwrap_or_default()).await;
        }
    }
    Err(DispatchFailed(last))
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
