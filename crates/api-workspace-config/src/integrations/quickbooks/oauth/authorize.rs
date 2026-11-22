//! POST /api/{project_id}/integrations/quickbooks/authorize
//!
//! Authenticated, workspace-scoped. Optionally stores the client secret in
//! the workspace secret manager (so the public callback can resolve it),
//! records a state row, and returns the Intuit consent URL for the FE to
//! hand off to (popup or full-page redirect).

use crate::integrations::quickbooks::oauth::state::{CreateState, QuickbooksOauthStateService};
use axum::Json;
use axum::http::{HeaderMap, StatusCode, header};
use oxy_app::surface::WorkspaceManagerReadOnly;
use oxy_app::surface::role_guards::WorkspaceAdmin;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use serde::{Deserialize, Serialize};

use crate::integrations::oauth_provider::{self, Provider};

/// This handler is QuickBooks-specific only in which descriptor it passes.
/// Everything below is provider-neutral — see
/// `internal-docs/customer-apps-integrations.md`.
const PROVIDER: Provider = oauth_provider::QUICKBOOKS;

#[derive(Debug, Deserialize)]
pub struct AuthorizeRequest {
    pub client_id: String,
    /// Plaintext client secret to store under `client_secret_var`. Omit to
    /// reuse an already-stored secret of that name (e.g. on reconnect).
    #[serde(default)]
    pub client_secret: Option<String>,
    pub client_secret_var: String,
    pub refresh_token_var: String,
    /// `popup` (default) or `redirect` — how the success page returns.
    #[serde(default)]
    pub mode: Option<String>,
    /// FE path to return to in `redirect` mode.
    #[serde(default)]
    pub return_path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AuthorizeResponse {
    /// Pre-built Intuit consent URL. The FE opens it in a popup or navigates.
    pub url: String,
}

/// Legacy QuickBooks authorize — `/integrations/quickbooks/authorize`.
pub async fn authorize(
    _: WorkspaceAdmin,
    user: AuthenticatedUserExtractor,
    workspace: WorkspaceManagerReadOnly,
    headers: HeaderMap,
    req: Json<AuthorizeRequest>,
) -> Result<Json<AuthorizeResponse>, (StatusCode, String)> {
    authorize_for(PROVIDER, user, workspace, headers, req).await
}

/// Uniform authorize — `/integrations/oauth/{provider}/authorize`. An unknown
/// slug is a 404, never a fallback: defaulting would send a user's consent, and
/// their client secret, to a vendor they did not pick.
pub async fn authorize_by_slug(
    _: WorkspaceAdmin,
    axum::extract::Path((_workspace_id, slug)): axum::extract::Path<(uuid::Uuid, String)>,
    user: AuthenticatedUserExtractor,
    workspace: WorkspaceManagerReadOnly,
    headers: HeaderMap,
    req: Json<AuthorizeRequest>,
) -> Result<Json<AuthorizeResponse>, (StatusCode, String)> {
    let provider = oauth_provider::by_slug(&slug)
        .ok_or((StatusCode::NOT_FOUND, format!("unknown provider '{slug}'")))?;
    authorize_for(provider, user, workspace, headers, req).await
}

/// # Authorization
///
/// **Both entry points take `WorkspaceAdmin`, and must.** This writes a
/// caller-supplied plaintext under a **caller-supplied secret name**
/// (`client_secret_var`), and the callback later writes the refresh token under
/// another (`refresh_token_var`). Neither name is allowlisted, so without the
/// guard any workspace Member could POST
/// `{"client_secret_var": "OPENAI_API_KEY", ...}` and overwrite an unrelated
/// workspace secret — before any consent redirect is even returned.
///
/// `WorkspaceManagerReadOnly` does NOT gate this: it is a filesystem-capability
/// marker, not a role. Every route into the secret store enforces
/// `WorkspaceAdmin` through its extractor signature (the workspace's own are
/// `oxy-api-workspace-config`'s `secret_routes`); this is one
/// of those routes reached through a different door, and the UI already assumes
/// it (`Connections` renders inside `CanWorkspaceAdmin`).
async fn authorize_for(
    provider: Provider,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    headers: HeaderMap,
    Json(req): Json<AuthorizeRequest>,
) -> Result<Json<AuthorizeResponse>, (StatusCode, String)> {
    // VALIDATE, then write. Everything that can reject this request runs before
    // the secret upsert below, so a 400 leaves no trace: the caller's plaintext
    // must not land in `client_secret_var` on a request we then refuse. That is
    // not a security hole now the route is admin-only, but a rejected request
    // with a side effect reads as a bug during a reconnect — the secret changed
    // and the connection did not.
    let redirect_uri = callback_redirect_uri(provider, &headers)?;
    let mode = match req.mode.as_deref() {
        Some("redirect") => "redirect",
        // Refused HERE, before consent. By the time the callback runs the token
        // is already stored, so a provider with no popup landing page would
        // otherwise 404 the user on an otherwise-successful connect.
        _ if provider.popup_path.is_none() => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "{} has no popup landing page — use mode \"redirect\"",
                    provider.slug
                ),
            ));
        }
        _ => "popup",
    }
    .to_string();

    // Persist the client secret so the (unauthenticated) callback can resolve
    // it for the token exchange. Blank value → reuse an existing secret.
    if let Some(secret) = req
        .client_secret
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        workspace_manager
            .secrets_manager
            .upsert_secret(&req.client_secret_var, secret, user.id)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("store client secret: {e}"),
                )
            })?;
    }

    let nonce = QuickbooksOauthStateService::create(CreateState {
        provider: provider.slug.to_string(),
        project_id: workspace_manager.workspace_id,
        client_id: req.client_id.clone(),
        client_secret_var: req.client_secret_var,
        refresh_token_var: req.refresh_token_var,
        redirect_uri: redirect_uri.clone(),
        mode,
        return_path: req.return_path,
        created_by: Some(user.id),
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let url = provider.consent_url(&req.client_id, &redirect_uri, &nonce);
    Ok(Json(AuthorizeResponse { url }))
}

/// Build the public callback URL from the inbound request host/proto. BYO
/// users must register exactly this URL in their Intuit app, and it must
/// match what the token exchange later sends — so it's computed once here
/// and stored on the state row for verbatim reuse in the callback.
fn callback_redirect_uri(
    provider: Provider,
    headers: &HeaderMap,
) -> Result<String, (StatusCode, String)> {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .ok_or((StatusCode::BAD_REQUEST, "missing Host header".to_string()))?;
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| {
            if host.starts_with("localhost") || host.starts_with("127.0.0.1") {
                "http".to_string()
            } else {
                "https".to_string()
            }
        });
    Ok(provider.callback_url(&proto, host))
}
