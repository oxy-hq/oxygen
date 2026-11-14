//! The sign-in half of what `ops` used to hold: the magic-link rate limit,
//! OAuth state and the three providers' code exchanges, the user insert a
//! first sign-in races on, and the magic-link request and email. The session
//! plumbing every sign-in path shares (the cookie, the token, the payload) is
//! `ops`, below.

use axum::{
    http::{HeaderMap, StatusCode},
    response::Json,
};
use chrono::{Duration, Utc};
use entity::{prelude::Users, users, users::UserStatus};
use governor::{
    DefaultKeyedRateLimiter, Quota, RateLimiter,
    clock::{Clock, DefaultClock},
};
use handlebars::Handlebars;
use jsonwebtoken::{Validation, decode};
use once_cell::sync::Lazy;
use oxy::config::auth::MagicLinkAuth;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use serde::Deserialize;
use std::num::NonZeroU32;
use uuid::Uuid;

use oxy::database::errors::is_unique_violation;
use oxy::database::{client::establish_connection, filters::UserQueryFilterExt};
use oxy_shared::errors::OxyError;

use oxy_app::surface::session::validate_return_to_url;

use super::anonymous_link::SignInLink;
use super::dto::*;

// ─── Magic Link Rate Limiter ────────────────────────────────────────────────
//
// Token-bucket rate limiter (governor). State is in-process only and resets
// on restart — intentional, no external dependency required.
// Checked before the allowlist so timing cannot reveal allowlist membership.
//
// Limit: 5 requests per email per hour.

static MAGIC_LINK_RATE_LIMITER: Lazy<DefaultKeyedRateLimiter<String>> =
    Lazy::new(|| RateLimiter::keyed(Quota::per_hour(NonZeroU32::new(5).expect("5 > 0"))));

/// Returns `None` if the request is allowed, or `Some(seconds)` with the wait
/// time until the next request is permitted.
pub(super) fn check_magic_link_rate_limit(email: &str) -> Option<u64> {
    match MAGIC_LINK_RATE_LIMITER.check_key(&email.to_lowercase()) {
        Ok(()) => None,
        Err(not_until) => {
            let wait = not_until.wait_time_from(DefaultClock::default().now());
            Some(wait.as_secs().max(1))
        }
    }
}
// ─── OAuth state (CSRF defense) ────────────────────────────────────────────
//
// The frontend fetches a signed state token via `POST /auth/oauth/state`,
// echoes it through the OAuth provider redirect, and sends it back with the
// code. We verify the HMAC + short expiry before exchanging the code, which
// prevents an attacker from splicing a captured code into another user's
// session.

/// Time-to-live for an OAuth state token. Long enough to complete the round
/// trip through an interactive provider, short enough to limit replay.
pub(super) const OAUTH_STATE_TTL_SECS: i64 = 10 * 60;

pub(super) const OAUTH_STATE_PURPOSE: &str = "oauth-state";

pub(super) async fn verify_oauth_state(state: &str) -> Result<(), StatusCode> {
    let validation = Validation::default();
    // Its own key, not the session's: a `state` can never be presented as a
    // session, and a session never as a `state`.
    let key = oxy_auth::session_key::decoding_key(oxy_auth::session_key::Purpose::OAuthState)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let data = decode::<OAuthStateClaims>(state, &key, &validation).map_err(|e| {
        tracing::warn!("OAuth state rejected: {}", e);
        StatusCode::UNAUTHORIZED
    })?;
    if data.claims.purpose != OAUTH_STATE_PURPOSE {
        tracing::warn!("OAuth state has wrong purpose claim");
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

/// Exchange a GitHub OAuth authorization code for user profile info and the raw
/// GitHub access token. The token is returned so callers can store it for later
/// use (e.g. listing GitHub App installations without a second sign-in).
/// Base origin for an OAuth `redirect_uri` at token-exchange time.
///
/// Local multi-instance dev: when several instances share one registered
/// redirect URI via the OAuth bounce proxy, the token-exchange `redirect_uri`
/// must equal the one the provider saw at authorize time (the proxy origin),
/// not this instance's request origin. `OXY_OAUTH_REDIRECT_ORIGIN` overrides it;
/// unset → the request origin, exactly as before.
fn oauth_redirect_base(base_url: &str) -> String {
    std::env::var("OXY_OAUTH_REDIRECT_ORIGIN")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| base_url.to_string())
}

pub(super) async fn exchange_github_code_for_user_info(
    code: &str,
    base_url: &str,
) -> Result<OAuthUserInfo, OxyError> {
    let client_id = std::env::var("GITHUB_CLIENT_ID")
        .map_err(|_| OxyError::ConfigurationError("GITHUB_CLIENT_ID not configured".to_string()))?;
    let client_secret = std::env::var("GITHUB_CLIENT_SECRET").map_err(|_| {
        OxyError::ConfigurationError("GITHUB_CLIENT_SECRET not configured".to_string())
    })?;

    let redirect_uri = format!("{}/github/callback", oauth_redirect_base(base_url));

    let client = oxy_shared::http::builder()
        .user_agent("Oxy/1.0")
        .build()
        .map_err(|e| OxyError::RuntimeError(e.to_string()))?;

    // Exchange the authorization code for an access token.
    let token_response = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
        ])
        .send()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("GitHub token request failed: {e}")))?;

    if !token_response.status().is_success() {
        return Err(OxyError::RuntimeError(format!(
            "GitHub token exchange error: {}",
            token_response.status()
        )));
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: Option<String>,
        error: Option<String>,
    }

    let token_data: TokenResponse = token_response.json().await.map_err(|e| {
        OxyError::RuntimeError(format!("Failed to parse GitHub token response: {e}"))
    })?;

    if let Some(err) = token_data.error {
        return Err(OxyError::RuntimeError(format!("GitHub OAuth error: {err}")));
    }

    let access_token = token_data.access_token.ok_or_else(|| {
        OxyError::RuntimeError("GitHub token response missing access_token".to_string())
    })?;

    let user_resp: GitHubUserInfo = client
        .get("https://api.github.com/user")
        .bearer_auth(&access_token)
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("GitHub /user request failed: {e}")))?
        .json()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Failed to parse GitHub user: {e}")))?;

    // Use the profile email if set; otherwise fetch the primary verified email.
    let email = if let Some(e) = user_resp.email.filter(|e| !e.is_empty()) {
        e
    } else {
        let emails: Vec<GitHubEmailEntry> = client
            .get("https://api.github.com/user/emails")
            .bearer_auth(&access_token)
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .await
            .map_err(|e| {
                OxyError::RuntimeError(format!("GitHub /user/emails request failed: {e}"))
            })?
            .json()
            .await
            .map_err(|e| OxyError::RuntimeError(format!("Failed to parse GitHub emails: {e}")))?;

        emails
            .into_iter()
            .find(|e| e.primary && e.verified)
            .map(|e| e.email)
            .ok_or_else(|| {
                OxyError::RuntimeError(
                    "No verified primary email found on GitHub account".to_string(),
                )
            })?
    };

    let name = user_resp.name.unwrap_or_else(|| user_resp.login.clone());

    Ok(OAuthUserInfo {
        email,
        name,
        picture: user_resp.avatar_url,
    })
}

/// Insert a new user, handling the race condition where another request may have
/// created the same user concurrently.
pub(super) async fn insert_user_or_fetch_existing(
    new_user: users::ActiveModel,
    email: &str,
    connection: &DatabaseConnection,
) -> Result<users::Model, StatusCode> {
    match new_user.insert(connection).await {
        Ok(user) => Ok(user),
        Err(e) if is_unique_violation(&e) => {
            // Race condition: another request created the user concurrently.
            // Fetch the existing user.
            Users::find()
                .filter_by_email(email)
                .one(connection)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to query user after unique violation: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?
                .ok_or_else(|| {
                    tracing::error!(
                        "User '{}' not found after unique constraint violation",
                        email
                    );
                    StatusCode::INTERNAL_SERVER_ERROR
                })
        }
        Err(e) => {
            tracing::error!("Failed to create user: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

pub(super) async fn exchange_google_code_for_user_info(
    code: &str,
    base_url: &str,
) -> Result<OAuthUserInfo, OxyError> {
    let auth_config = oxy::config::oxy::get_oxy_config()
        .ok()
        .and_then(|config| config.authentication);

    let google_config = auth_config.and_then(|auth| auth.google).ok_or_else(|| {
        OxyError::ConfigurationError("Google OAuth configuration not found".to_string())
    })?;

    let client = oxy_shared::http::client();

    let redirect_uri = format!("{}/auth/google/callback", oauth_redirect_base(base_url));

    let client_secret = google_config.client_secret;

    let token_request = serde_json::json!({
        "client_id": google_config.client_id,
        "client_secret": client_secret,
        "code": code,
        "grant_type": "authorization_code",
        "redirect_uri": redirect_uri
    });

    // Note: Google supports application/json for token exchange (non-standard but accepted)
    // Standard OAuth 2.0 requires application/x-www-form-urlencoded
    let token_response = client
        .post("https://oauth2.googleapis.com/token")
        .header("Content-Type", "application/json")
        .json(&token_request)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Failed to send token request to Google: {}", e);
            OxyError::ConfigurationError(format!("Failed to exchange code for token: {e}"))
        })?;

    let status = token_response.status();
    if !status.is_success() {
        let error_body = token_response.text().await.unwrap_or_default();
        tracing::error!(
            "Google token exchange failed with status {}: {}",
            status,
            error_body
        );
        return Err(OxyError::ConfigurationError(format!(
            "Google token exchange failed with status {}: {}",
            status, error_body
        )));
    }

    let token_data: serde_json::Value = token_response.json().await.map_err(|e| {
        tracing::error!("Failed to parse Google token response: {}", e);
        OxyError::ConfigurationError(format!("Failed to parse token response: {e}"))
    })?;

    let access_token = token_data["access_token"]
        .as_str()
        .ok_or_else(|| OxyError::ConfigurationError("No access token in response".to_string()))?;

    let user_info_response = client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Failed to send userinfo request to Google: {}", e);
            OxyError::ConfigurationError(format!("Failed to get user info: {e}"))
        })?;

    let status = user_info_response.status();
    if !status.is_success() {
        let error_body = user_info_response.text().await.unwrap_or_default();
        tracing::error!(
            "Google userinfo request failed with status {}: {}",
            status,
            error_body
        );
        return Err(OxyError::ConfigurationError(format!(
            "Google userinfo request failed with status {}: {}",
            status, error_body
        )));
    }

    let user_info: OAuthUserInfo = user_info_response.json().await.map_err(|e| {
        tracing::error!("Failed to parse Google userinfo response: {}", e);
        OxyError::ConfigurationError(format!("Failed to parse user info: {e}"))
    })?;

    Ok(user_info)
}

pub(super) async fn exchange_okta_code_for_user_info(
    code: &str,
    base_url: &str,
) -> Result<OktaUserInfo, OxyError> {
    let auth_config = oxy::config::oxy::get_oxy_config()
        .ok()
        .and_then(|config| config.authentication);

    let okta_config = auth_config.and_then(|auth| auth.okta).ok_or_else(|| {
        OxyError::ConfigurationError("Okta OAuth configuration not found".to_string())
    })?;

    let client = oxy_shared::http::client();

    let redirect_uri = format!("{base_url}/auth/okta/callback");

    let client_secret = okta_config.client_secret;
    let okta_domain = okta_config.domain;

    // Exchange authorization code for tokens
    // OAuth 2.0 requires application/x-www-form-urlencoded for token requests
    let token_params = [
        ("client_id", okta_config.client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("code", code),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri.as_str()),
    ];

    // Use org authorization server (matches /oauth2/v1/authorize from frontend)
    let token_url = format!("https://{}/oauth2/v1/token", okta_domain);

    let token_response = client
        .post(&token_url)
        .form(&token_params)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Failed to send token request to Okta: {}", e);
            OxyError::ConfigurationError(format!("Failed to exchange code for token: {e}"))
        })?;

    let status = token_response.status();
    if !status.is_success() {
        let error_body = token_response.text().await.unwrap_or_default();
        tracing::error!(
            "Okta token exchange failed with status {}: {}",
            status,
            error_body
        );
        return Err(OxyError::ConfigurationError(format!(
            "Okta token exchange failed with status {}: {}",
            status, error_body
        )));
    }

    let token_data: serde_json::Value = token_response.json().await.map_err(|e| {
        tracing::error!("Failed to parse Okta token response: {}", e);
        OxyError::ConfigurationError(format!("Failed to parse token response: {e}"))
    })?;

    let access_token = token_data["access_token"]
        .as_str()
        .ok_or_else(|| OxyError::ConfigurationError("No access token in response".to_string()))?;

    // Get user info using the access token (use org authorization server)
    let userinfo_url = format!("https://{}/oauth2/v1/userinfo", okta_domain);

    let user_info_response = client
        .get(&userinfo_url)
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Failed to send userinfo request to Okta: {}", e);
            OxyError::ConfigurationError(format!("Failed to get user info: {e}"))
        })?;

    let status = user_info_response.status();
    if !status.is_success() {
        let error_body = user_info_response.text().await.unwrap_or_default();
        tracing::error!(
            "Okta userinfo request failed with status {}: {}",
            status,
            error_body
        );
        return Err(OxyError::ConfigurationError(format!(
            "Okta userinfo request failed with status {}: {}",
            status, error_body
        )));
    }

    let user_info: OktaUserInfo = user_info_response.json().await.map_err(|e| {
        tracing::error!("Failed to parse Okta userinfo response: {}", e);
        OxyError::ConfigurationError(format!("Failed to parse user info: {e}"))
    })?;

    Ok(user_info)
}

// ─── Magic Link ────────────────────────────────────────────────────────────

/// Basic RFC-5321-bounded email format check. Not exhaustive, but filters out
/// obviously malformed inputs before they reach the DB or SES.
pub(super) fn is_valid_email_format(email: &str) -> bool {
    if email.len() > 254 {
        return false;
    }
    // split_once splits at the first '@'; rejecting any additional '@' in the
    // domain part enforces exactly one '@' (RFC 5321 §4.1.2).
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    if local.is_empty() || domain.is_empty() || domain.contains('@') {
        return false;
    }
    // Domain must have at least one '.' with non-empty labels on both sides.
    let labels: Vec<&str> = domain.split('.').collect();
    labels.len() >= 2 && labels.iter().all(|l| !l.is_empty())
}

fn is_email_allowed(email: &str, config: &MagicLinkAuth) -> bool {
    // No domain is refused: an empty allowlist admits every address. email is
    // already lowercased at ingestion; compare case-insensitively so operators
    // can list "Crew@Gmail.com" or "crew@gmail.com" interchangeably.
    config.allowed_emails.is_empty()
        || config
            .allowed_emails
            .iter()
            .any(|e| e.eq_ignore_ascii_case(email))
}

#[cfg(test)]
mod magic_link_eligibility_tests {
    use super::is_email_allowed;
    use oxy::config::auth::{Authentication, MagicLinkAuth};

    /// Builds the config the way the server does, from the environment. nextest
    /// runs each test in its own process, so these writes don't leak.
    fn config_from_env(vars: &[(&str, &str)]) -> MagicLinkAuth {
        unsafe {
            std::env::set_var("MAGIC_LINK_LOCAL_TEST", "true");
            std::env::remove_var("MAGIC_LINK_BLOCKED_DOMAINS");
            std::env::remove_var("MAGIC_LINK_ALLOWED_EMAILS");
            for (key, value) in vars {
                std::env::set_var(key, value);
            }
        }
        Authentication::from_env()
            .expect("auth config loads")
            .magic_link
            .expect("magic link is configured")
    }

    #[test]
    fn a_personal_mailbox_is_eligible_by_default() {
        let config = config_from_env(&[]);
        assert!(is_email_allowed("store.manager@gmail.com", &config));
    }

    #[test]
    fn a_leftover_domain_block_list_no_longer_refuses_anyone() {
        let config = config_from_env(&[("MAGIC_LINK_BLOCKED_DOMAINS", "gmail.com, Yahoo.com")]);
        assert!(is_email_allowed("store.manager@gmail.com", &config));
        assert!(is_email_allowed("store.manager@yahoo.com", &config));
    }

    #[test]
    fn an_allowlist_still_admits_only_the_listed_addresses() {
        let config = config_from_env(&[("MAGIC_LINK_ALLOWED_EMAILS", "Crew@Gmail.com")]);
        assert!(is_email_allowed("crew@gmail.com", &config));
        assert!(!is_email_allowed("someone.else@gmail.com", &config));
    }
}

pub(super) async fn request_magic_link_inner(
    headers: HeaderMap,
    req: MagicLinkRequest,
) -> Result<Json<MessageResponse>, StatusCode> {
    let auth_config = oxy::config::oxy::get_oxy_config()
        .ok()
        .and_then(|c| c.authentication)
        .and_then(|a| a.magic_link);

    let magic_link_config = auth_config.ok_or_else(|| {
        tracing::error!("Magic link auth not configured");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Always return 200 — don't leak whether email is allowed
    if !is_email_allowed(&req.email, &magic_link_config) {
        tracing::info!(
            "Magic link requested for non-allowlisted email: {}",
            req.email
        );
        return Ok(Json(MessageResponse {
            message: "If your email is eligible, a sign-in link has been sent.".to_string(),
        }));
    }

    let connection = establish_connection().await.map_err(|e| {
        tracing::error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let existing = Users::find()
        .filter_by_email(&req.email)
        .one(&connection)
        .await
        .map_err(|e| {
            tracing::error!("Failed to query user: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Resolve the user model — either the existing one or a freshly inserted one.
    // We capture the model here so we can reuse it for the token update below
    // without an extra DB round-trip.
    let user_for_update = match existing {
        Some(u) if u.status == UserStatus::Deleted => {
            // Silently succeed — don't reveal account status
            return Ok(Json(MessageResponse {
                message: "If your email is eligible, a sign-in link has been sent.".to_string(),
            }));
        }
        Some(u) => u, // existing active user — reuse model directly
        None => {
            // Auto-create new user
            let name = req.email.split('@').next().unwrap_or("User").to_string();
            let new_user = users::ActiveModel {
                id: Set(Uuid::new_v4()),
                email: Set(Some(req.email.clone())),
                name: Set(name),
                picture: Set(None),
                email_verified: Set(false),
                magic_link_token: Set(None),
                magic_link_token_expires_at: Set(None),
                status: Set(UserStatus::Active),
                created_at: sea_orm::ActiveValue::NotSet,
                last_login_at: sea_orm::ActiveValue::NotSet,
            };
            match new_user.insert(&connection).await {
                Ok(inserted) => inserted,
                Err(e) if is_unique_violation(&e) => {
                    // Race condition — another request created the user concurrently.
                    Users::find()
                        .filter_active_by_email(&req.email)
                        .one(&connection)
                        .await
                        .map_err(|e| {
                            tracing::error!("Failed to query user after race condition: {}", e);
                            StatusCode::INTERNAL_SERVER_ERROR
                        })?
                        .ok_or_else(|| {
                            tracing::error!("User not found after unique violation: {}", req.email);
                            StatusCode::INTERNAL_SERVER_ERROR
                        })?
                }
                Err(e) => {
                    tracing::error!("Failed to create user: {}", e);
                    return Err(StatusCode::INTERNAL_SERVER_ERROR);
                }
            }
        }
    };

    // Generate 256-bit random token
    let token_bytes: [u8; 32] = rand::random();
    let token = hex::encode(token_bytes);
    let expires_at = Utc::now() + Duration::minutes(15);

    // Update user row with token + expiry — reuses the model from above, no extra query.
    let mut active: users::ActiveModel = user_for_update.into();
    active.magic_link_token = Set(Some(token.clone()));
    active.magic_link_token_expires_at = Set(Some(expires_at.into()));
    active.update(&connection).await.map_err(|e| {
        tracing::error!("Failed to save magic link token: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Validate return_to before stuffing it into the email. The web-app
    // performs the actual redirect, but if the proxy hands us a malicious
    // URL we don't want to embed it in the outbound link at all.
    let return_to = match req.return_to.as_deref() {
        Some(url) if validate_return_to_url(url) => Some(url.to_string()),
        Some(url) => {
            tracing::warn!("rejecting return_to URL outside allowlist: {url}");
            None
        }
        None => None,
    };

    let link = super::anonymous_link::magic_link_for(&headers, &token, return_to.as_deref());

    // Log the magic link URL at debug level only — the token is a session credential
    // and must not appear in production log aggregators.
    tracing::debug!("Magic link for {}: {}", req.email, link.as_str());

    // Send email async
    let email_addr = req.email.clone();
    let cfg_clone = magic_link_config.clone();
    tokio::spawn(async move {
        if let Err(e) = send_magic_link_email(&email_addr, &link, &cfg_clone).await {
            tracing::error!("Failed to send magic link email: {}", e);
        }
    });

    Ok(Json(MessageResponse {
        message: "If your email is eligible, a sign-in link has been sent.".to_string(),
    }))
}

/// Mails `link`. It takes a [`SignInLink`], which only
/// `anonymous_link::magic_link_for` makes, so no caller can mail a sign-in
/// link built on a host the request named.
async fn send_magic_link_email(
    to_email: &str,
    link: &SignInLink,
    config: &MagicLinkAuth,
) -> Result<(), OxyError> {
    use oxy_app_core::emails::{
        EmailMessage, EmailProvider, local_test::LocalTestEmailProvider, ses::SesEmailProvider,
    };

    let magic_link_url = link.as_str();
    let message = EmailMessage {
        subject: "Sign in to Oxygen".to_string(),
        html_body: build_magic_link_email_html(magic_link_url, to_email)?,
        text_body: format!(
            "Your sign-in link for Oxygen\n\nClick the link below to sign in. For security, this link expires in 15 minutes and can only be used once.\n\n{magic_link_url}\n\nThis link was requested for {to_email}. If you didn't request this, you can safely ignore this email — your account remains secure."
        ),
    };

    if std::env::var("MAGIC_LINK_LOCAL_TEST").is_ok() {
        LocalTestEmailProvider
            .send(&config.from_email, to_email, message)
            .await
    } else {
        SesEmailProvider::new(config.aws_region.as_deref())
            .await
            .send(&config.from_email, to_email, message)
            .await
    }
}

static MAGIC_LINK_TEMPLATE: Lazy<Handlebars<'static>> = Lazy::new(|| {
    let mut hbs = Handlebars::new();
    hbs.register_template_string("magic_link", include_str!("magic_link.hbs"))
        .expect("magic_link.hbs is valid");
    hbs
});

fn build_magic_link_email_html(magic_link_url: &str, to_email: &str) -> Result<String, OxyError> {
    let data = serde_json::json!({
        "magic_link_url": magic_link_url,
        "to_email": to_email,
        "year": Utc::now().format("%Y").to_string(),
    });

    MAGIC_LINK_TEMPLATE
        .render("magic_link", &data)
        .map_err(|e| OxyError::RuntimeError(format!("Failed to render magic link template: {e}")))
}
