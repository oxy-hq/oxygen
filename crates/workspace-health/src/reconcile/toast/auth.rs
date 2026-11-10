//! Toast authentication. Prefers the OAuth2 client-credentials pair
//! (`client_id` + `client_secret`, exchanged for a short-lived access token),
//! falling back to a static `api_token` bearer. These are the logical secret
//! slots; the workspace-secret var names behind them are configured on the
//! `toast` integration in `config.yml`. `extract_access_token` is the
//! unit-tested pure parse step.

use super::super::source::{ReconcileError, SourceCtx};
use super::truncate_body;
use super::{API_TOKEN_SECRET, CLIENT_ID_SECRET, CLIENT_SECRET_SECRET};

/// Resolve a bearer token for the reporting calls. OAuth client-credentials if
/// both are present; else the static token; else `NotConfigured`.
pub async fn resolve_bearer(
    http: &reqwest::Client,
    base_url: &str,
    ctx: &SourceCtx,
) -> Result<String, ReconcileError> {
    match (
        ctx.secret(CLIENT_ID_SECRET),
        ctx.secret(CLIENT_SECRET_SECRET),
    ) {
        (Some(id), Some(secret)) => exchange_client_credentials(http, base_url, id, secret).await,
        _ => ctx
            .secret(API_TOKEN_SECRET)
            .map(str::to_string)
            .ok_or_else(|| ReconcileError::NotConfigured("toast".to_string())),
    }
}

/// OAuth2 client-credentials login: POST the client pair to Toast's auth
/// endpoint and return the issued access token.
async fn exchange_client_credentials(
    http: &reqwest::Client,
    base_url: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<String, ReconcileError> {
    let url = format!("{base_url}/authentication/v1/authentication/login");
    let resp = http
        .post(&url)
        .json(&serde_json::json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "userAccessType": "TOAST_MACHINE_CLIENT",
        }))
        .send()
        .await
        .map_err(|e| ReconcileError::Unreachable(format!("toast auth: {e}")))?;
    // Read the body as text first so a non-JSON / error response (wrong host,
    // 401, HTML error page) surfaces the actual status + payload instead of an
    // opaque "error decoding response body".
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| ReconcileError::Unreachable(format!("toast auth body: {e}")))?;
    if !status.is_success() {
        // Don't echo the raw auth-endpoint body into the verdict reason (it's
        // admin-surfaced and an auth response can carry sensitive detail). Log
        // it for diagnostics; return only the status in the persisted error.
        tracing::warn!(
            target: "health_eval",
            %status,
            body = %truncate_body(&text),
            "toast auth failed"
        );
        return Err(ReconcileError::Fetch(format!(
            "toast auth failed: HTTP {status}"
        )));
    }
    let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        ReconcileError::Fetch(format!(
            "toast auth json: {e} — body: {}",
            truncate_body(&text)
        ))
    })?;
    extract_access_token(&body)
}

/// Pull `token.accessToken` from a Toast login response. Pure.
fn extract_access_token(body: &serde_json::Value) -> Result<String, ReconcileError> {
    body.get("token")
        .and_then(|t| t.get("accessToken"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| ReconcileError::Fetch("toast auth: accessToken missing".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_access_token_from_login_payload() {
        let body = serde_json::json!({
            "token": { "accessToken": "abc.def.ghi", "tokenType": "Bearer" }
        });
        assert_eq!(extract_access_token(&body).unwrap(), "abc.def.ghi");
    }

    #[test]
    fn missing_access_token_is_fetch_error() {
        let body = serde_json::json!({ "token": {} });
        let err = extract_access_token(&body).unwrap_err();
        assert!(matches!(err, ReconcileError::Fetch(_)));
    }
}
