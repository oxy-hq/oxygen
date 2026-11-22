//! quickbooks_oauth_states helpers: create + atomic single-use consume.
//!
//! Mirrors the Slack OAuth state service. The row carries no secret
//! material — only the var names the callback resolves/writes through the
//! workspace secret manager, plus the exact redirect_uri and return mode.

use chrono::{Duration, Utc};
use entity::quickbooks_oauth_states;
use oxy::database::client::establish_connection;
use oxy_shared::errors::OxyError;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

/// State rows live 15 minutes — long enough for a consent round-trip.
pub const STATE_TTL_MIN: i64 = 15;

#[derive(Debug, Clone)]
pub struct CreateState {
    /// Provider slug this consent is for. Verified at consume time against the
    /// provider named by the callback route, so a nonce minted for one vendor
    /// cannot be redeemed at another's callback.
    pub provider: String,
    pub project_id: Uuid,
    pub client_id: String,
    pub client_secret_var: String,
    pub refresh_token_var: String,
    pub redirect_uri: String,
    /// `popup` | `redirect`.
    pub mode: String,
    pub return_path: Option<String>,
    pub created_by: Option<Uuid>,
}

pub struct QuickbooksOauthStateService;

impl QuickbooksOauthStateService {
    /// Insert a new state row; returns the nonce to put in the Intuit URL.
    pub async fn create(input: CreateState) -> Result<String, OxyError> {
        let nonce = generate_nonce();
        let conn = establish_connection().await?;
        quickbooks_oauth_states::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            nonce: ActiveValue::Set(nonce.clone()),
            provider: ActiveValue::Set(input.provider),
            project_id: ActiveValue::Set(input.project_id),
            client_id: ActiveValue::Set(input.client_id),
            client_secret_var: ActiveValue::Set(input.client_secret_var),
            refresh_token_var: ActiveValue::Set(input.refresh_token_var),
            redirect_uri: ActiveValue::Set(input.redirect_uri),
            mode: ActiveValue::Set(input.mode),
            return_path: ActiveValue::Set(input.return_path),
            created_by: ActiveValue::Set(input.created_by),
            created_at: ActiveValue::NotSet,
            expires_at: ActiveValue::Set((Utc::now() + Duration::minutes(STATE_TTL_MIN)).into()),
            consumed_at: ActiveValue::Set(None),
        }
        .insert(&conn)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?;
        Ok(nonce)
    }

    /// Atomically consume a nonce for `provider`. A single `UPDATE … WHERE
    /// consumed_at IS NULL AND expires_at > now()` means two concurrent
    /// callbacks cannot both claim the same nonce (CSRF/replay guard).
    ///
    /// The provider is matched in the same `UPDATE`, not checked afterwards: a
    /// post-hoc check would have already burned the nonce, turning a
    /// wrong-callback probe into a denial-of-service against a legitimate
    /// consent still in flight.
    pub async fn consume(
        nonce: &str,
        provider: &str,
    ) -> Result<quickbooks_oauth_states::Model, OxyError> {
        let conn = establish_connection().await?;

        let update_result = quickbooks_oauth_states::Entity::update_many()
            .col_expr(
                quickbooks_oauth_states::Column::ConsumedAt,
                sea_orm::prelude::Expr::value(Utc::now()),
            )
            .filter(quickbooks_oauth_states::Column::Nonce.eq(nonce))
            .filter(quickbooks_oauth_states::Column::Provider.eq(provider))
            .filter(quickbooks_oauth_states::Column::ConsumedAt.is_null())
            .filter(
                quickbooks_oauth_states::Column::ExpiresAt
                    .gt::<DateTimeWithTimeZone>(Utc::now().into()),
            )
            .exec(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))?;

        if update_result.rows_affected > 0 {
            let row = quickbooks_oauth_states::Entity::find()
                .filter(quickbooks_oauth_states::Column::Nonce.eq(nonce))
                .one(&conn)
                .await
                .map_err(|e| OxyError::DBError(e.to_string()))?
                .ok_or_else(|| OxyError::DBError("state disappeared after update".into()))?;
            return Ok(row);
        }

        // Zero rows updated — classify why for an actionable error.
        let existing = quickbooks_oauth_states::Entity::find()
            .filter(quickbooks_oauth_states::Column::Nonce.eq(nonce))
            .one(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))?;
        match existing {
            None => Err(OxyError::ValidationError("unknown state nonce".into())),
            // Deliberately the same message as an unknown nonce: telling a
            // caller "that nonce is real, just for another provider" confirms a
            // valid nonce to someone who guessed one.
            Some(r) if r.provider != provider => {
                Err(OxyError::ValidationError("unknown state nonce".into()))
            }
            Some(r) if r.consumed_at.is_some() => {
                Err(OxyError::ValidationError("state already consumed".into()))
            }
            Some(_) => Err(OxyError::ValidationError("state expired".into())),
        }
    }
}

fn generate_nonce() -> String {
    let buf: [u8; 32] = rand::random();
    hex::encode(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_is_64_hex_chars() {
        let nonce = generate_nonce();
        assert_eq!(nonce.len(), 64);
        assert!(nonce.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn nonce_is_unique() {
        assert_ne!(generate_nonce(), generate_nonce());
    }
}
