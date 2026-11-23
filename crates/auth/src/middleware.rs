use axum::{
    extract::State,
    http::{Method, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

use crate::token::CredentialContext;
use crate::user::UserService;

use crate::{authenticator::Authenticator, built_in::BuiltInAuthenticator};
use entity::users::UserStatus;
use oxy_shared::errors::OxyError;

/// On every response to a request that authenticated with a token that
/// expires: when it does, RFC 3339 UTC (API-tokens design §8 Phase 5). Absent
/// for a session and for a credential that never expires.
pub const TOKEN_EXPIRATION_HEADER: &str = "x-oxy-token-expiration";

/// What a surface answers for every refusal that is not the legacy one.
type Fallback<E> = fn(E) -> Response;

/// `/api`: the error's own status, which logs it and reports it to Sentry.
fn mapped<E: Into<StatusCode>>(err: E) -> Response {
    err.into().into_response()
}

/// `/external/api`: a bare 401, whatever went wrong. The caller has already
/// logged one `warn` with the detail.
fn unauthorized<E>(_: E) -> Response {
    StatusCode::UNAUTHORIZED.into_response()
}

/// The response a refused request gets.
///
/// A failed authentication has always been a **bodyless** 401 here, and stays
/// one: the body of a refusal tells an attacker which guess was closer, and
/// every caller that should see detail is a browser being redirected to sign
/// in.
///
/// One exception, and it is the reason this function exists: a **removed legacy
/// API key**. That caller is a script or a cron nobody is watching, the key has
/// worked for a year, and the 401 is the only place the explanation can land —
/// a log line on our side reaches nobody who can act. Saying "legacy API keys
/// were removed, mint a token" gives away nothing: the shape is recognised
/// offline, so the answer is the same whether or not such a key ever existed.
/// Generic over the authenticator's error, because `auth_middleware` is: the
/// one case this answers is matched on the message, which `OxyError`'s
/// `Display` renders verbatim for an `AuthenticationError`. A different
/// authenticator simply never produces it.
///
/// Everything else is the caller's own `fallback`, unchanged. That is not
/// ceremony: `/api` maps the error through `From<OxyError> for StatusCode`,
/// which logs at `error` and calls `capture_to_sentry`, while `/external/api`
/// has always answered a bare 401 and logged one `warn`. Routing the public,
/// `CORS: *` surface through the former would let anyone fill the Sentry quota
/// with junk `X-API-Key` values, and would turn a `DBError` during lookup into
/// a 500 where that surface has always said 401.
fn refusal<E: std::error::Error>(err: E, fallback: Fallback<E>) -> Response {
    if err.to_string() == crate::token::dispatch::LEGACY_KEY_REMOVED {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({
                "error": crate::token::dispatch::LEGACY_KEY_REMOVED,
                "code": "legacy_api_key_removed",
            })),
        )
            .into_response();
    }
    fallback(err)
}

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
    /// `sandbox_agent` is whether the tree this guards admits a sandbox agent
    /// token. `Admit` belongs only on a tree that also mounts the route
    /// allow-list (`app_grant_scope`), which is `api_auth_layers`.
    pub fn built_in(sandbox_agent: crate::token::SandboxAgent) -> Self {
        Self {
            authenticator: Arc::new(BuiltInAuthenticator::new(sandbox_agent)),
            guest_only: false,
        }
    }

    /// Returns an `AuthState` that bypasses the authenticator entirely and
    /// always attaches the local guest user. Only appropriate for the
    /// local-mode router.
    pub fn guest_only() -> Self {
        Self {
            authenticator: Arc::new(BuiltInAuthenticator::new(
                crate::token::SandboxAgent::Refuse,
            )),
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
    let (claims, credential) = match auth_state
        .authenticator
        .authenticate_with_credential(request.headers())
        .await
    {
        Ok(pair) => pair,
        Err(err) => {
            // No log line of our own. `mapped` goes through
            // `From<OxyError> for StatusCode`, which logs this refusal once at
            // `warn`; logging here too put the same 401 in the platform logs
            // twice, both times at `error`. The one refusal `mapped` never
            // sees — a removed legacy key, which `refusal` answers itself — is
            // already logged at `info` by `token::dispatch`.
            return Ok(refusal(err, mapped));
        }
    };

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

    let (identity, credential) = match crate::token::authenticate_request(
        request.headers(),
        crate::token::AuthSurface::ApiKeyOnly,
        crate::token::SandboxAgent::Refuse,
    )
    .await
    {
        Ok(pair) => pair,
        Err(err) => {
            tracing::warn!("external API: API key authentication failed: {err}");
            return Ok(refusal(err, unauthorized));
        }
    };

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
        let state = AuthState::<BuiltInAuthenticator>::built_in(crate::token::SandboxAgent::Refuse);
        assert!(!state.guest_only);
    }

    #[test]
    fn guest_only_state_flags_itself() {
        let state = AuthState::<BuiltInAuthenticator>::guest_only();
        assert!(state.guest_only);
    }

    async fn body_of(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("read the body");
        String::from_utf8_lossy(&bytes).to_string()
    }

    /// The one refusal that carries a body. Pinned here because the value of
    /// recognising a removed format at all is that the caller is *told*: a
    /// bodyless 401 sends a script's owner looking for a network fault.
    #[tokio::test]
    async fn a_removed_legacy_key_is_refused_with_the_text_that_names_its_replacement() {
        let refused = refusal(
            OxyError::AuthenticationError(crate::token::dispatch::LEGACY_KEY_REMOVED.to_string()),
            mapped,
        );
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
        let body = body_of(refused).await;
        assert!(body.contains("legacy_api_key_removed"), "{body}");
        assert!(body.contains("were removed"), "{body}");
        assert!(body.contains("API token"), "{body}");
    }

    /// Every other authentication failure stays bodyless, as it has always
    /// been: a refusal's body would say which guess was closer. On both
    /// surfaces, since each has its own fallback.
    #[tokio::test]
    async fn every_other_refusal_stays_bodyless() {
        for message in ["Invalid API key", "No API key found in headers"] {
            for fallback in [mapped as Fallback<OxyError>, unauthorized] {
                let err = OxyError::AuthenticationError(message.to_string());
                let refused = refusal(err, fallback);
                assert_eq!(refused.status(), StatusCode::UNAUTHORIZED, "{message}");
                assert_eq!(body_of(refused).await, "", "{message}");
            }
        }
    }

    /// `/external/api` answers 401 for **every** failure, including one that is
    /// our fault. It is a public, `CORS: *` surface: mapping the error through
    /// `From<OxyError>` instead would report each junk `X-API-Key` to Sentry at
    /// `error`, and turn a database error during lookup into a 500 where this
    /// surface has always said 401.
    #[tokio::test]
    async fn the_external_surface_answers_401_even_when_the_fault_is_ours() {
        let refused = refusal(
            OxyError::DBError("pool timed out".to_string()),
            unauthorized,
        );
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_of(refused).await, "");

        // …and that is a real difference, not a restatement: the same error on
        // `/api` is a 500.
        let on_api = refusal(OxyError::DBError("pool timed out".to_string()), mapped);
        assert_eq!(on_api.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// The legacy body wins over either fallback: it is the one case the
    /// function exists for, and a script on `/external/api` is exactly the
    /// caller that needs telling.
    #[tokio::test]
    async fn the_legacy_body_reaches_the_external_surface_too() {
        let refused = refusal(
            OxyError::AuthenticationError(crate::token::dispatch::LEGACY_KEY_REMOVED.to_string()),
            unauthorized,
        );
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
        assert!(body_of(refused).await.contains("legacy_api_key_removed"));
    }
}
