use entity::prelude::Users;
use entity::users;
use oxy_platform::db::{DbFailure, establish_connection};
use oxy_platform::filters::UserQueryFilterExt;
use oxy_shared::errors::OxyError;
use sea_orm::{ActiveValue, DbErr, EntityTrait, Set, prelude::*};
use uuid::Uuid;

use crate::types::{AuthenticatedUser, Identity};
use entity::users::UserStatus;

/// Email address for the built-in local guest user (no-auth local mode).
/// This user is always granted Owner role so local installs work out of the box.
pub const LOCAL_GUEST_EMAIL: &str = "<local-user@example.com>";

/// Turn a failed user lookup into an `OxyError` that still says which side of
/// the wire broke.
///
/// This is the last place the `DbErr` exists — every caller sees a string. The
/// custom-app gate renders one of these as a 500 whose body carries a code, and
/// without the split here the answer would be the same for a reset connection
/// and for bad SQL. That ambiguity is what made a DNS problem read as an
/// application bug for an afternoon.
fn user_query_err(e: DbErr) -> OxyError {
    let msg = format!("Failed to query user: {e}");
    match DbFailure::classify(&e) {
        DbFailure::Unreachable => OxyError::DatabaseUnreachable(msg),
        DbFailure::Query => OxyError::DBError(msg),
    }
}

pub struct UserService;

impl UserService {
    /// Look up an existing user by the email in the identity. Does not create.
    /// Used by non-mutating endpoints (e.g. `GET /user`) where an incidental user
    /// row should not be minted just because someone hit the endpoint.
    pub async fn find_user_by_identity(
        identity: &Identity,
    ) -> Result<Option<AuthenticatedUser>, OxyError> {
        let connection = establish_connection().await?;
        // By id when the credential carries one. A session JWT always does, and
        // for a frontline worker it is the only thing that resolves — their
        // `users.email` is NULL, so the address lookup below matches nothing.
        //
        // This is also the stronger lookup for everyone else: an id cannot
        // collide the way a display string can.
        if let Some(user_id) = identity.user_id {
            let user = Users::find_by_id(user_id)
                .one(&connection)
                .await
                .map_err(user_query_err)?;
            return Ok(user.map(|u| u.into()));
        }
        let user = Users::find()
            .filter_by_email(&identity.email)
            .one(&connection)
            .await
            .map_err(user_query_err)?;
        Ok(user.map(|u| u.into()))
    }

    pub async fn get_or_create_user(identity: &Identity) -> Result<AuthenticatedUser, OxyError> {
        let connection = establish_connection().await?;

        // A credential naming a user id resolves to that user, full stop. It
        // must NEVER fall through to the create path below: minting a second
        // row for an id that already exists is how one human becomes two, and
        // for a frontline worker the create path would insert a user with a
        // NULL email and no credential — unreachable, and impossible to notice.
        if let Some(user_id) = identity.user_id {
            return Users::find_by_id(user_id)
                .one(&connection)
                .await
                .map_err(|e| OxyError::DBError(format!("Failed to query user: {e}")))?
                .map(|u| u.into())
                .ok_or_else(|| {
                    OxyError::AuthenticationError(format!(
                        "token names user {user_id}, which does not exist"
                    ))
                });
        }

        // First, try to find existing user
        if let Some(existing_user) = Users::find()
            .filter_by_email(&identity.email)
            .one(&connection)
            .await
            .map_err(|e| OxyError::DBError(format!("Failed to query user: {e}")))?
        {
            return Ok(existing_user.into());
        }

        // User not found, create new user.

        let new_user = users::ActiveModel {
            id: Set(Uuid::new_v4()),
            email: Set(Some(identity.email.clone())),
            name: Set(identity
                .name
                .clone()
                .unwrap_or_else(|| identity.email.clone())),
            picture: Set(identity.picture.clone()),
            email_verified: Set(true),
            magic_link_token: ActiveValue::not_set(),
            magic_link_token_expires_at: ActiveValue::not_set(),
            status: Set(UserStatus::Active),
            created_at: ActiveValue::not_set(), // Will use database default
            last_login_at: ActiveValue::not_set(), // Will use database default
        };

        match new_user.insert(&connection).await {
            Ok(user) => {
                tracing::info!("Created new user: {:?} ({})", user.email, user.id);
                Ok(user.into())
            }
            Err(e) if is_unique_violation(&e) => {
                // Race condition: another request created the user concurrently.
                // Fetch the existing user.
                Users::find()
                    .filter_by_email(&identity.email)
                    .one(&connection)
                    .await
                    .map_err(|e| OxyError::DBError(format!("Failed to query user: {e}")))?
                    .map(|u| u.into())
                    .ok_or_else(|| {
                        OxyError::DBError(format!(
                            "User '{}' not found after unique constraint violation",
                            identity.email
                        ))
                    })
            }
            Err(e) => Err(OxyError::DBError(format!("Failed to create user: {e}"))),
        }
    }

    pub async fn update_user_profile(
        user_id: Uuid,
        name: Option<String>,
        picture: Option<String>,
    ) -> Result<AuthenticatedUser, OxyError> {
        let connection = establish_connection().await?;

        let user = Users::find_by_id(user_id)
            .one(&connection)
            .await
            .map_err(|e| OxyError::DBError(format!("Failed to query user: {e}")))?
            .ok_or_else(|| OxyError::DBError("User not found".to_string()))?;

        let mut user: users::ActiveModel = user.into();

        if let Some(name) = name {
            user.name = Set(name);
        }

        if let Some(picture) = picture {
            user.picture = Set(Some(picture));
        }

        let updated_user = user
            .update(&connection)
            .await
            .map_err(|e| OxyError::DBError(format!("Failed to update user: {e}")))?;

        Ok(updated_user.into())
    }

    pub async fn list_all_users() -> Result<Vec<AuthenticatedUser>, OxyError> {
        let connection = establish_connection().await?;

        let users = Users::find()
            .all(&connection)
            .await
            .map_err(|e| OxyError::DBError(format!("Failed to query users: {e}")))?;

        Ok(users.into_iter().map(|user| user.into()).collect())
    }

    pub async fn delete_user(user_id: Uuid) -> Result<(), OxyError> {
        let connection = establish_connection().await?;

        let user = Users::find_by_id(user_id)
            .filter_active()
            .one(&connection)
            .await
            .map_err(|e| OxyError::DBError(format!("Failed to query user: {e}")))?
            .ok_or_else(|| OxyError::DBError("User not found or already deleted".to_string()))?;

        let mut user: users::ActiveModel = user.into();
        user.status = Set(UserStatus::Deleted);

        user.update(&connection)
            .await
            .map_err(|e| OxyError::DBError(format!("Failed to delete user: {e}")))?;

        Ok(())
    }

    pub async fn update_user_status(user_id: Uuid, status: UserStatus) -> Result<(), OxyError> {
        let connection = establish_connection().await?;

        let user = Users::find_by_id(user_id)
            .one(&connection)
            .await
            .map_err(|e| OxyError::DBError(format!("Failed to query user: {e}")))?
            .ok_or_else(|| OxyError::DBError("User not found".to_string()))?;

        let mut user: users::ActiveModel = user.into();
        user.status = Set(status);

        user.update(&connection)
            .await
            .map_err(|e| OxyError::DBError(format!("Failed to update user status: {e}")))?;

        Ok(())
    }
}

/// Check if a database error is a unique constraint violation.
/// Uses Sea-ORM's structured `SqlErr` rather than string matching so the check
/// is portable across DB engines.
fn is_unique_violation(err: &DbErr) -> bool {
    matches!(
        err.sql_err(),
        Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_unique_violation_negative_on_non_sql_err() {
        // DbErr variants without a structured SqlErr payload must not be
        // treated as unique-constraint violations.
        let err = DbErr::Custom("some non-DB error".to_string());
        assert!(!is_unique_violation(&err));
    }
}
