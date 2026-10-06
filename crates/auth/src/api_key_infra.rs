//! Resolve the user an API key or token acts as.
//!
//! Header parsing and the lookup order moved to [`crate::token::dispatch`], the
//! one dispatch every entry point goes through.

use entity::prelude::Users;
use entity::users::{self, UserStatus};
use oxy_shared::errors::OxyError;
use sea_orm::{ConnectionTrait, EntityTrait};
use uuid::Uuid;

use crate::types::Identity;

/// The identity of the user a validated key or token belongs to.
///
/// `Identity` is the *provider-shaped* identity — the thing a user is looked
/// up by — so its email is not optional. A user with no mailbox therefore
/// cannot carry an API key, and that is the right answer rather than a gap:
/// API keys are a developer credential, and a frontline worker enrolled by
/// PIN has no path to one. Refusing here beats minting an identity with an
/// empty address that later reaches SES.
pub async fn identity_for_key_owner<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
) -> Result<Identity, OxyError> {
    let user = key_owner(db, user_id).await?;

    let Some(email) = user.email else {
        tracing::warn!(
            user_id = %user_id,
            "API key belongs to a user with no email address; refusing"
        );
        return Err(OxyError::AuthenticationError(
            "API keys require an account with an email address".to_string(),
        ));
    };
    Ok(Identity {
        // A key already resolved to a row, so name it. Re-resolving by address
        // would be a second lookup that can only agree or be wrong.
        user_id: Some(user.id),
        picture: user.picture,
        email,
        name: Some(user.name),
    })
}

async fn key_owner<C: ConnectionTrait>(db: &C, user_id: Uuid) -> Result<users::Model, OxyError> {
    Users::find_by_id(user_id)
        .one(db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to fetch user for API key: {}", e);
            OxyError::AuthenticationError("Failed to authenticate user".to_string())
        })?
        .ok_or_else(|| {
            tracing::error!("User not found for validated API key: {}", user_id);
            OxyError::AuthenticationError("User not found".to_string())
        })
        .and_then(active)
}

/// The user, unless their account was deactivated. Checked where a key or
/// token is admitted, so it holds on every entry point: the status check in
/// `auth_middleware` covers `/api` alone, and the custom-app paths (`/fn`,
/// `/logs`) authenticate inside their handlers.
fn active(user: users::Model) -> Result<users::Model, OxyError> {
    if user.status == UserStatus::Active {
        return Ok(user);
    }
    tracing::warn!(
        user_id = %user.id,
        status = user.status.as_str(),
        "key or token belongs to an inactive user; refusing"
    );
    Err(OxyError::AuthenticationError(
        "This account is no longer active".to_string(),
    ))
}

/// Refuse unless `user_id` is still an active user.
///
/// What a **cached** credential is asked on every request: the cache holds the
/// token, not its owner's standing, so a deactivated owner would otherwise
/// keep up to 30 s of access on every pod. One primary-key read.
pub async fn require_active_owner<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
) -> Result<(), OxyError> {
    key_owner(db, user_id).await.map(|_| ())
}

/// The identity of a **service account** a validated `oxy_sat_` token acts as.
///
/// The one principal that carries a token with no mailbox, and only because
/// the caller has already found its `service_accounts` row: the refusal in
/// [`identity_for_key_owner`] stands for everyone else, a frontline worker
/// included. The identity names the account by id, so nothing resolves it by
/// address, and its address is empty — it can match no email-keyed standing,
/// invitation or provider login.
///
/// An account whose `users` row *has* an address is refused: that row could
/// be signed in to and could hold email-keyed standing, which is exactly what
/// a service account must never be able to do.
pub async fn identity_for_service_account<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
) -> Result<Identity, OxyError> {
    let user = key_owner(db, user_id).await?;
    if user.email.is_some() {
        tracing::error!(
            user_id = %user_id,
            "service account's user row has an email address; refusing"
        );
        return Err(OxyError::AuthenticationError("Invalid API key".to_string()));
    }
    Ok(Identity {
        user_id: Some(user.id),
        picture: None,
        email: String::new(),
        name: Some(user.name),
    })
}
