//! slack_oauth_states helpers: create, consume, expire.

use chrono::{Duration, Utc};
use entity::prelude::SlackOauthStates;
use entity::slack_oauth_states;
use oxy::database::client::establish_connection;
use oxy_shared::errors::OxyError;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

pub const STATE_TTL_MIN: i64 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateKind {
    Install,
    UserLink,
}

impl StateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::UserLink => "user_link",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreateInstallState {
    pub org_id: Uuid,
    pub oxy_user_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct CreateUserLinkState {
    pub slack_team_id: String,
    pub slack_user_id: String,
    /// Channel where the user originally sent the unlinked message. Stored
    /// so the confirm handler can post "✅ You're connected!" back there.
    pub slack_channel_id: Option<String>,
    /// Thread timestamp to target for the post-connection confirmation.
    pub slack_thread_ts: Option<String>,
}

pub struct OauthStateService;

impl OauthStateService {
    /// Insert a new install state. Returns the nonce.
    pub async fn create_install(input: CreateInstallState) -> Result<String, OxyError> {
        let nonce = generate_nonce();
        let conn = establish_connection().await?;
        slack_oauth_states::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            kind: ActiveValue::Set(StateKind::Install.as_str().into()),
            nonce: ActiveValue::Set(nonce.clone()),
            org_id: ActiveValue::Set(Some(input.org_id)),
            oxy_user_id: ActiveValue::Set(Some(input.oxy_user_id)),
            slack_team_id: ActiveValue::Set(None),
            slack_user_id: ActiveValue::Set(None),
            slack_channel_id: ActiveValue::Set(None),
            slack_thread_ts: ActiveValue::Set(None),
            created_at: ActiveValue::NotSet,
            expires_at: ActiveValue::Set((Utc::now() + Duration::minutes(STATE_TTL_MIN)).into()),
            consumed_at: ActiveValue::Set(None),
        }
        .insert(&conn)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?;
        Ok(nonce)
    }

    pub async fn create_user_link(input: CreateUserLinkState) -> Result<String, OxyError> {
        let nonce = generate_nonce();
        let conn = establish_connection().await?;
        slack_oauth_states::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            kind: ActiveValue::Set(StateKind::UserLink.as_str().into()),
            nonce: ActiveValue::Set(nonce.clone()),
            org_id: ActiveValue::Set(None),
            oxy_user_id: ActiveValue::Set(None),
            slack_team_id: ActiveValue::Set(Some(input.slack_team_id)),
            slack_user_id: ActiveValue::Set(Some(input.slack_user_id)),
            slack_channel_id: ActiveValue::Set(input.slack_channel_id),
            slack_thread_ts: ActiveValue::Set(input.slack_thread_ts),
            created_at: ActiveValue::NotSet,
            expires_at: ActiveValue::Set((Utc::now() + Duration::minutes(STATE_TTL_MIN)).into()),
            consumed_at: ActiveValue::Set(None),
        }
        .insert(&conn)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?;
        Ok(nonce)
    }

    /// Atomically consume a nonce of the given kind. Returns the row if
    /// found, not expired, not already consumed.
    ///
    /// Uses a single UPDATE ... WHERE consumed_at IS NULL so that two
    /// concurrent requests cannot both claim the same nonce (TOCTOU fix).
    pub async fn consume(
        nonce: &str,
        expected_kind: StateKind,
    ) -> Result<slack_oauth_states::Model, OxyError> {
        let conn = establish_connection().await?;

        // Atomic claim: only updates if unconsumed, unexpired, and correct kind.
        let update_result = SlackOauthStates::update_many()
            .col_expr(
                slack_oauth_states::Column::ConsumedAt,
                sea_orm::prelude::Expr::value(Utc::now()),
            )
            .filter(slack_oauth_states::Column::Nonce.eq(nonce))
            .filter(slack_oauth_states::Column::Kind.eq(expected_kind.as_str()))
            .filter(slack_oauth_states::Column::ConsumedAt.is_null())
            .filter(
                slack_oauth_states::Column::ExpiresAt.gt::<DateTimeWithTimeZone>(Utc::now().into()),
            )
            .exec(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))?;

        if update_result.rows_affected > 0 {
            // We claimed it — re-fetch the now-consumed row.
            let row = SlackOauthStates::find()
                .filter(slack_oauth_states::Column::Nonce.eq(nonce))
                .one(&conn)
                .await
                .map_err(|e| OxyError::DBError(e.to_string()))?
                .ok_or_else(|| OxyError::DBError("state disappeared after update".into()))?;
            return Ok(row);
        }

        // UPDATE hit zero rows — classify the failure with a read-only lookup.
        let existing = SlackOauthStates::find()
            .filter(slack_oauth_states::Column::Nonce.eq(nonce))
            .one(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))?;

        match existing {
            None => Err(OxyError::ValidationError("unknown state nonce".into())),
            Some(r) if r.kind != expected_kind.as_str() => {
                Err(OxyError::ValidationError("state kind mismatch".into()))
            }
            Some(r) if r.consumed_at.is_some() => {
                Err(OxyError::ValidationError("state already consumed".into()))
            }
            Some(_) => Err(OxyError::ValidationError("state expired".into())),
        }
    }
}

/// Delete oauth states older than 7 days. Conservative — state TTL is 15 min,
/// so anything older is long-dead. Safe to run hourly (or on startup).
pub async fn sweep_expired() -> Result<u64, OxyError> {
    let conn = establish_connection().await?;
    let cutoff: DateTimeWithTimeZone = (Utc::now() - Duration::days(7)).into();
    let res = SlackOauthStates::delete_many()
        .filter(slack_oauth_states::Column::ExpiresAt.lt(cutoff))
        .exec(&conn)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?;
    Ok(res.rows_affected)
}

fn generate_nonce() -> String {
    let buf: [u8; 32] = rand::random();
    hex::encode(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_kind_install_as_str() {
        assert_eq!(StateKind::Install.as_str(), "install");
    }

    #[test]
    fn state_kind_user_link_as_str() {
        assert_eq!(StateKind::UserLink.as_str(), "user_link");
    }

    #[test]
    fn nonce_is_64_hex_chars() {
        let nonce = generate_nonce();
        assert_eq!(nonce.len(), 64, "32 bytes = 64 hex chars");
        assert!(nonce.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn nonce_is_unique() {
        let a = generate_nonce();
        let b = generate_nonce();
        assert_ne!(a, b, "nonces should be unique");
    }
}
