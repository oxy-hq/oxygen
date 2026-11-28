//! The Looker credentials check behind
//! `POST /{workspace_id}/integrations/looker/{name}/test`, moved out of the
//! Rust `oxy looker test` (2026-11-21) — which read the integration from a
//! local checkout and the credentials from the shell — and shaped like
//! `oxy_api_data_plane::oltp::probe` (`…/oltp/test-connection`): `ok` and `stage` are the
//! verdict, the rest is what someone needs to act on it.
//!
//! The check is the command's: resolve the client id and secret, build the
//! client, and list the LookML models, which signs in first. A refusal is the
//! answer, never an error status. Credentials never appear in the response:
//! the variables are named, their values are not, and the client sends the
//! secret in a form body that no error text carries.

use std::time::{Duration, Instant};

use oxy::config::model::LookerIntegration;
use oxy_looker::client::LookerApiClient;
use oxy_looker::models::LookmlModel;
use oxy_looker::{LookerAuthConfig, LookerError};
use serde::Serialize;

/// How long the whole check may take: sign-in plus the model list. The
/// client's own per-request timeout is 30s; past this the answer is the
/// timeout itself.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(45);

/// A server's error text is reported, but capped: it is whatever the
/// configured `base_url` answered, and a misconfigured URL can answer a page.
const MAX_ERROR_CHARS: usize = 500;

/// The credentials as the secrets store resolved them, `None` when unset.
pub struct Credentials {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
}

/// One LookML model Looker listed, and how many explores it has.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct LookerModelSummary {
    pub name: String,
    pub explores: usize,
}

/// What one check found. `oxyc looker test --json` prints it as is, so its
/// fields are a contract: pinned by `the_response_shape_is_pinned` below and
/// by `sdk/cli/src/commands/looker.test.ts`.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct LookerConnectionTest {
    /// Looker signed us in and listed the models.
    pub ok: bool,
    /// Where it stopped, when it did: `resolve` (a credential is not in the
    /// workspace's secrets), `client` (the client could not be built), `auth`
    /// (Looker refused the credentials), `connect` (the network, or any other
    /// answer from Looker), or `timeout`. `None` when `ok`.
    pub stage: Option<&'static str>,
    /// What stopped it, as Looker, the network or the resolver said it.
    pub error: Option<String>,
    /// The integration's name in `config.yml`.
    pub integration: String,
    /// The Looker instance it dialled, from the compiled config.
    pub base_url: String,
    /// The models Looker listed. Empty unless `ok`.
    pub models: Vec<LookerModelSummary>,
    pub elapsed_ms: u64,
}

type Failure = (&'static str, String);

/// Run the check for `integration` with the credentials already resolved.
pub async fn check(
    name: &str,
    integration: &LookerIntegration,
    credentials: Credentials,
) -> LookerConnectionTest {
    let started = Instant::now();
    let outcome = match resolved(integration, credentials) {
        Err(failure) => Err(failure),
        Ok(auth) => match tokio::time::timeout(CHECK_TIMEOUT, list_models(auth)).await {
            Ok(outcome) => outcome,
            Err(_) => Err((
                "timeout",
                format!("no answer within {}s", CHECK_TIMEOUT.as_secs()),
            )),
        },
    };
    report(name, &integration.base_url, outcome, started.elapsed())
}

/// Both credentials, or the `resolve` failure naming the variable that is
/// missing — never a value.
fn resolved(
    integration: &LookerIntegration,
    credentials: Credentials,
) -> Result<LookerAuthConfig, Failure> {
    let missing = |what: &str, var: &str| {
        (
            "resolve",
            format!("{what} not found: no secret named {var} in the workspace's secrets"),
        )
    };
    let client_id = credentials
        .client_id
        .ok_or_else(|| missing("client ID", &integration.client_id_var))?;
    let client_secret = credentials
        .client_secret
        .ok_or_else(|| missing("client secret", &integration.client_secret_var))?;
    Ok(LookerAuthConfig {
        base_url: integration.base_url.clone(),
        client_id,
        client_secret,
    })
}

/// Build the client and list the models; listing signs in first.
async fn list_models(auth: LookerAuthConfig) -> Result<Vec<LookmlModel>, Failure> {
    let mut client = LookerApiClient::new(auth).map_err(|e| ("client", e.to_string()))?;
    client
        .list_models()
        .await
        .map_err(|e| (stage_of(&e), e.to_string()))
}

/// The stage a Looker error stopped at, from its variant, not its text.
fn stage_of(error: &LookerError) -> &'static str {
    match error {
        LookerError::AuthenticationError { .. } => "auth",
        LookerError::ConfigError { .. } => "client",
        _ => "connect",
    }
}

fn report(
    name: &str,
    base_url: &str,
    outcome: Result<Vec<LookmlModel>, Failure>,
    elapsed: Duration,
) -> LookerConnectionTest {
    let (ok, stage, error, models) = match outcome {
        Ok(models) => (
            true,
            None,
            None,
            models
                .into_iter()
                .map(|model| LookerModelSummary {
                    explores: model.explores.len(),
                    name: model.name,
                })
                .collect(),
        ),
        Err((stage, error)) => (false, Some(stage), Some(capped(error)), Vec::new()),
    };
    LookerConnectionTest {
        ok,
        stage,
        error,
        integration: name.to_string(),
        base_url: base_url.to_string(),
        models,
        elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
    }
}

fn capped(error: String) -> String {
    if error.chars().count() <= MAX_ERROR_CHARS {
        return error;
    }
    let mut cut: String = error.chars().take(MAX_ERROR_CHARS).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
#[path = "looker_test_tests.rs"]
mod tests;
