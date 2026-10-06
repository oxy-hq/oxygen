use axum::{
    extract::State,
    http::{Method, Request, StatusCode},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

use crate::token::CredentialContext;
use crate::user::UserService;

use crate::{authenticator::Authenticator, built_in::BuiltInAuthenticator};
use entity::users::UserStatus;
use oxy_shared::errors::OxyError;

/// On every response to a request that authenticated with a key or token that
/// expires: when it does, RFC 3339 UTC (API-tokens design §8 Phase 5). Legacy
/// keys included — a header informs and changes nothing. Absent for a session
/// and for a credential that never expires.
pub const TOKEN_EXPIRATION_HEADER: &str = "x-oxy-token-expiration";

/// Run the rest of the stack, then stamp [`TOKEN_EXPIRATION_HEADER`]. The one
/// place the header is written, so no handler has to remember it.
async fn run_stamped(
    request: Request<axum::body::Body>,
    next: Next,
    credential: Option<&CredentialContext>,
) -> Response {
    let expires_at = credential.and_then(|c| c.expires_at);
    let mut response = next.run(request).await;
    if let Some(value) = expires_at.and_then(|at| {
        let text = at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        axum::http::HeaderValue::from_str(&text).ok()
    }) {
        response
            .headers_mut()
            .insert(TOKEN_EXPIRATION_HEADER, value);
    }
    response
}

pub struct AuthState<T> {
    authenticator: Arc<T>,
    /// When true, auth_middleware short-circuits: it never calls the
    /// authenticator and injects the local guest user into extensions.
    /// Set by `AuthState::guest_only()`. Used only by the local-mode router.
    pub guest_only: bool,
}

impl<T> Clone for AuthState<T> {
    fn clone(&self) -> Self {
        Self {
            authenticator: Arc::clone(&self.authenticator),
            guest_only: self.guest_only,
        }
    }
}

impl AuthState<BuiltInAuthenticator> {
    pub fn built_in() -> Self {
        Self {
            authenticator: Arc::new(BuiltInAuthenticator::new()),
            guest_only: false,
        }
    }

    /// Returns an `AuthState` that bypasses the authenticator entirely and
    /// always attaches the local guest user. Only appropriate for the
    /// local-mode router.
    pub fn guest_only() -> Self {
        Self {
            authenticator: Arc::new(BuiltInAuthenticator::new()),
            guest_only: true,
        }
    }
}

pub async fn auth_middleware<T: Authenticator>(
    State(auth_state): State<AuthState<T>>,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // Allow OPTIONS requests (CORS preflight) to pass through without authentication
    if request.method() == Method::OPTIONS {
        return Ok(next.run(request).await);
    }

    // Guest-only mode (local server): never consult the authenticator, always
    // attach the local guest user. Reuses the existing LOCAL_GUEST_EMAIL
    // mechanism rather than inventing a new sentinel.
    if auth_state.guest_only {
        let identity = crate::types::Identity {
            // Same reasoning as the built-in guest: this must be able to create.
            user_id: None,
            email: crate::user::LOCAL_GUEST_EMAIL.to_string(),
            name: Some("Local User".to_string()),
            picture: None,
        };
        let user = UserService::get_or_create_user(&identity)
            .await
            .map_err(|e| {
                tracing::error!("Failed to get or create local guest user: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        if user.status != UserStatus::Active {
            tracing::warn!(
                "Local guest user is not active (status: {}) — refusing request",
                user.status.as_str()
            );
            return Err(StatusCode::FORBIDDEN);
        }
        request.extensions_mut().insert(user);
        return Ok(next.run(request).await);
    }

    // App-publish-token bearer: a long-lived machine credential (`oxypublish_...`) that
    // resolves to its owning app-admin. Checked before the standard
    // authenticator because it is neither a JWT nor an X-API-Key and would
    // otherwise fall through to a 401. On success we attach the resolved user
    // AND an `AppPublishTokenAuth` marker so the scope-enforcement middleware can
    // confine these requests to the customer-apps admin surface. On a
    // present-but-invalid admin bearer we reject immediately rather than
    // falling through — a revoked/unknown app publish token must never be retried as
    // some other credential.
    if let Some(bearer) = extract_bearer(request.headers())
        && crate::app_publish_token_domain::is_app_publish_token(&bearer)
    {
        return authenticate_app_publish_token(&bearer, request, next).await;
    }

    // Authenticate using the configured authenticator — `authenticate_request`
    // for the built-in one, which also names the key or token when one was used.
    let (claims, credential) = auth_state
        .authenticator
        .authenticate_with_credential(request.headers())
        .await
        .map_err(|err| {
            tracing::error!("Authentication failed: {}", err);
            err.into()
        })?;

    let user = UserService::get_or_create_user(&claims)
        .await
        .map_err(|e| match e {
            // A token that names a user who no longer exists is an
            // AUTHENTICATION failure, not a server fault. This became reachable
            // when `Claims.sub` started resolving by user id: delete a user and
            // their still-valid token arrives here forever, and mapping it to
            // 500 both lies to the caller — who should re-authenticate, not
            // retry — and fills the error log with a line nobody can act on.
            OxyError::AuthenticationError(_) => StatusCode::UNAUTHORIZED,
            other => {
                tracing::error!("Failed to find or create user: {}", other);
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;

    if user.status != UserStatus::Active {
        tracing::warn!(
            "Inactive user {} (status: {}) attempted to access protected route",
            user.label(),
            user.status.as_str()
        );
        return Err(StatusCode::FORBIDDEN);
    }

    // Add user to request extensions for downstream handlers, and — beside it,
    // never instead of it — the credential marker when a key or token was used.
    // The user carries the same credential, so code that holds only the user
    // still authorizes as the token allows.
    request
        .extensions_mut()
        .insert(user.with_credential(credential.clone()));
    if let Some(credential) = &credential {
        request.extensions_mut().insert(credential.clone());
    }

    Ok(run_stamped(request, next, credential.as_ref()).await)
}

/// Pull a bearer token out of the `Authorization` header, tolerating the
/// case-insensitive `Bearer ` scheme prefix. Returns `None` when the header is
/// absent/undecodable or empty after trimming.
fn extract_bearer(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())?;
    let token = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
        .unwrap_or(raw)
        .trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// Resolve an app-publish-token bearer to its owner and attach both the user and the
/// `AppPublishTokenAuth` scope marker before running downstream. A revoked/unknown
/// token, an inactive owner, or a DB error all yield `401`.
async fn authenticate_app_publish_token(
    bearer: &str,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let db = oxy_platform::db::establish_connection()
        .await
        .map_err(|e| {
            tracing::error!("app-publish-token auth: DB connection failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let resolved = crate::app_publish_token_domain::resolve_app_publish_token(&db, bearer)
        .await
        .map_err(|e| {
            tracing::error!("app-publish-token auth: resolve failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or_else(|| {
            tracing::warn!("app-publish-token auth: presented token is unknown or revoked");
            StatusCode::UNAUTHORIZED
        })?;

    if resolved.user.status != UserStatus::Active {
        tracing::warn!(
            "app-publish-token auth: owner {} is not active — refusing",
            resolved.user.label()
        );
        return Err(StatusCode::FORBIDDEN);
    }

    request
        .extensions_mut()
        .insert(crate::types::AppPublishTokenAuth {
            token_id: resolved.token_id,
            app_id: resolved.app_id,
            machine_identity: resolved.machine_identity,
        });
    request.extensions_mut().insert(resolved.user);
    Ok(next.run(request).await)
}

/// Authenticate strictly via an API key or token — no session cookie, no
/// session JWT, no guest fallback, no publish token. Used by the external API
/// surface (`/external/api/*`), which is served with wide-open CORS. That is
/// safe precisely *because* this middleware only accepts a key: a key is not
/// an ambient browser credential (unlike the `oxy_session` cookie), so a
/// malicious cross-origin page cannot read it or have the browser attach it
/// automatically — there is no CSRF vector. The cookie-accepting
/// [`auth_middleware`] must never be combined with `*`-origin CORS for the
/// same reason.
///
/// Keys arrive in `X-API-Key` as before; a new-prefix token or an
/// `oxy_<hex>` key is also accepted as `Authorization: Bearer`, which is not
/// ambient either.
pub async fn api_key_only_middleware(
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // CORS preflight carries no credentials and must pass through.
    if request.method() == Method::OPTIONS {
        return Ok(next.run(request).await);
    }

    let (identity, credential) = crate::token::authenticate_request(
        request.headers(),
        crate::token::AuthSurface::ApiKeyOnly,
    )
    .await
    .map_err(|err| {
        tracing::warn!("external API: API key authentication failed: {err}");
        StatusCode::UNAUTHORIZED
    })?;

    let user = UserService::get_or_create_user(&identity)
        .await
        .map_err(|e| {
            tracing::error!("external API: failed to resolve user from API key: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if user.status != UserStatus::Active {
        return Err(StatusCode::FORBIDDEN);
    }

    request
        .extensions_mut()
        .insert(user.with_credential(credential.clone()));
    if let Some(credential) = &credential {
        request.extensions_mut().insert(credential.clone());
    }
    Ok(run_stamped(request, next, credential.as_ref()).await)
}

/// Middleware for the internal port that auto-authenticates as an internal user.
/// Uses UserService::get_or_create_user to ensure the user exists in the database,
/// so that foreign key constraints in downstream handlers work correctly.
pub async fn internal_auth_middleware(
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if request.method() == Method::OPTIONS {
        return Ok(next.run(request).await);
    }

    let internal_identity = crate::types::Identity {
        user_id: None,
        email: "internal@localhost".to_string(),
        name: Some("Internal".to_string()),
        picture: None,
    };

    let user = UserService::get_or_create_user(&internal_identity)
        .await
        .map_err(|e| {
            tracing::error!("Failed to get or create internal user: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    request.extensions_mut().insert(user);
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_state_is_not_guest_only() {
        let state = AuthState::<BuiltInAuthenticator>::built_in();
        assert!(!state.guest_only);
    }

    #[test]
    fn guest_only_state_flags_itself() {
        let state = AuthState::<BuiltInAuthenticator>::guest_only();
        assert!(state.guest_only);
    }
}
