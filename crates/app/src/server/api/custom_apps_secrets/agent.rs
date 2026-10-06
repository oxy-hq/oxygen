//! A **sandbox agent token** (`oxy_sbx_`) on the app-secrets surface (sandbox
//! agent credential design §1 rows S1–S3, §6.4).
//!
//! The token lists, sets and deletes the secrets of a sandbox it created, of
//! an app it is granted — and no other environment's. Production is not the
//! default for it: the absent `environment` is refused like any other
//! environment that is not its own, with the `404` a missing one gets.
//!
//! * [`authorize`] — the environment must be an own sandbox (oxy-authz,
//!   through `may_open_environment`).
//! * [`hold_own`] — a set or a delete then holds the sandbox's row lock for
//!   the write, and looks at who created it again under that lock.
//! * [`refuse_credential`] — the token may not store a value shaped like an
//!   Oxy credential. A secret is the one thing the API writes that a function
//!   reads, so this is what stops an agent parking its own token (or any
//!   other) where sandbox code can pick it up.

use axum::http::StatusCode;
use entity::apps;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{DatabaseConnection, DatabaseTransaction, TransactionTrait};
use uuid::Uuid;

use super::Failure;
use crate::server::api::custom_apps_env_resolve::may_open_environment;
use crate::server::api::custom_apps_sandboxes::own::lock_own;

/// The sandbox agent token `user` authenticated with, if that is their
/// credential.
pub(super) fn token_of(user: &AuthenticatedUser) -> Option<Uuid> {
    user.credential
        .as_ref()
        .filter(|credential| credential.is_sandbox_agent())
        .map(|credential| credential.token_id)
}

fn not_found(environment: &AppEnvironment) -> Failure {
    (
        StatusCode::NOT_FOUND,
        format!("this app has no environment {environment}"),
    )
}

/// Hold the token to `environment` being a sandbox it created, of `app`.
/// Call only for a sandbox agent token.
pub(super) async fn authorize(
    db: &DatabaseConnection,
    app: &apps::Model,
    user: &AuthenticatedUser,
    environment: &AppEnvironment,
) -> Result<(), Failure> {
    let caller = oxy_server_authz::Caller::from_user(user);
    if may_open_environment(db, &caller, app, environment).await {
        Ok(())
    } else {
        Err(not_found(environment))
    }
}

/// For a write by a sandbox agent token: the sandbox's row, locked, in a
/// transaction the caller keeps open until the write is done — and `404` if
/// that row is not a live sandbox the token created. `None` for every other
/// credential, with no read.
pub(super) async fn hold_own(
    db: &DatabaseConnection,
    app: &apps::Model,
    user: &AuthenticatedUser,
    environment: &AppEnvironment,
) -> Result<Option<DatabaseTransaction>, Failure> {
    let Some(token) = token_of(user) else {
        return Ok(None);
    };
    let failed = |e: sea_orm::DbErr| {
        tracing::error!(app_id = %app.id, "sandbox lock failed: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "environment lookup failed".to_string(),
        )
    };
    let txn = db.begin().await.map_err(failed)?;
    if lock_own(&txn, app.id, environment, token)
        .await
        .map_err(failed)?
    {
        Ok(Some(txn))
    } else {
        Err(not_found(environment))
    }
}

/// Refuse a sandbox agent token a secret value shaped like an Oxy credential.
/// Every other caller stores what they send, as before.
pub(super) fn refuse_credential(user: &AuthenticatedUser, value: &str) -> Result<(), Failure> {
    if token_of(user).is_some() && holds_oxy_credential(value) {
        return Err((
            StatusCode::BAD_REQUEST,
            "credential_shaped_value: a sandbox agent token cannot store a value shaped like an \
             Oxy credential (an API token or key, a publish token, or a session token)"
                .to_string(),
        ));
    }
    Ok(())
}

/// Whether `value` is, or carries, something shaped like an Oxy credential:
///
/// - a new-format token, by its prefix (`oxy_pat_`, `oxy_sat_`, `oxy_ci_`,
///   `oxy_sbx_`);
/// - a legacy `oxy_<32 hex>` key;
/// - an `oxypublish_<64 hex>` publish token;
/// - a JWT: three base64url segments, the first a JSON header (`eyJ…`).
///
/// Looked for in every run of token characters, so `Bearer <token>` and a
/// JSON blob holding one are caught as the bare value is.
fn holds_oxy_credential(value: &str) -> bool {
    value
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
        .any(|piece| oxy_auth::token::parse_format(piece).is_some() || is_jwt(piece))
}

fn is_jwt(piece: &str) -> bool {
    let segments: Vec<&str> = piece.split('.').collect();
    let base64url = |s: &&str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    };
    segments.len() == 3 && segments.iter().all(base64url) && segments[0].starts_with("eyJ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §6.4's four shapes, bare and carried inside a larger value.
    #[test]
    fn every_oxy_credential_shape_is_recognised() {
        let token = oxy_auth::token::generate_sandbox_agent().plaintext;
        let personal = oxy_auth::token::generate_personal().plaintext;
        let legacy = format!("oxy_{}", "0123456789abcdef".repeat(2));
        let publish = format!("oxypublish_{}", "0123456789abcdef".repeat(4));
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.c2lnbmF0dXJl";
        for shaped in [token.as_str(), &personal, &legacy, &publish, jwt] {
            assert!(holds_oxy_credential(shaped), "{shaped}");
            assert!(holds_oxy_credential(&format!("Bearer {shaped}")), "bearer");
            assert!(
                holds_oxy_credential(&format!("{{\"key\":\"{shaped}\"}}")),
                "json"
            );
            assert!(holds_oxy_credential(&format!("  {shaped}\n")), "padded");
        }
        // A new-format prefix is enough: a token with a damaged checksum is
        // still a token someone meant to store.
        assert!(holds_oxy_credential("oxy_sbx_notarealtoken"));
    }

    /// Ordinary secrets are not mistaken for one: a third party's key, a
    /// hostname with three labels, a connection string, a dotted version.
    #[test]
    fn ordinary_secret_values_are_not_credentials() {
        for plain in [
            "thirdparty_live_4eC39HqLyjWDarjtT1zdp7dc",
            "api.example.com",
            "postgres://user:pw@db.internal.example.com:5432/app",
            "1.2.3",
            "oxy",
            "oxy_staging_bucket",
            "whsec_abc.def.ghi",
            "",
        ] {
            assert!(!holds_oxy_credential(plain), "{plain}");
        }
    }
}
