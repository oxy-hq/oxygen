//! The `jti` single-use store: a token is spent the moment it verifies, at
//! either exchange, so one JWT mints at most one credential.

use chrono::{DateTime, Duration, Utc};
use entity::oidc_used_jti;
use entity::prelude::OidcUsedJti;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter,
    SqlErr,
};

use super::verify::OidcError;

/// How long a spent `jti` is remembered. Generous: the token itself expires in
/// minutes, and the sweep removes the row once this passes.
pub const REMEMBER_FOR: Duration = Duration::hours(1);

/// Record the `jti` as spent. The primary key is what makes it single-use: a
/// second insert of the same value is the replay.
///
/// Fails closed on any other database error — a token we cannot mark used is a
/// token we do not accept.
pub async fn burn<C: ConnectionTrait>(db: &C, jti: &str) -> Result<(), OidcError> {
    let row = oidc_used_jti::ActiveModel {
        jti: ActiveValue::Set(jti.to_string()),
        expires_at: ActiveValue::Set((Utc::now() + REMEMBER_FOR).fixed_offset()),
    };
    match row.insert(db).await {
        Ok(_) => Ok(()),
        Err(e) => Err(burn_error(e)),
    }
}

/// The driver's own classification of the failure, never its message text: a
/// unique violation is the replay, and nothing else is.
fn burn_error(e: DbErr) -> OidcError {
    match e.sql_err() {
        Some(SqlErr::UniqueConstraintViolation(_)) => OidcError::Replayed,
        _ => OidcError::Db(e.to_string()),
    }
}

/// Delete every spent `jti` whose `expires_at` has passed. Idempotent, so any
/// number of replicas running it is harmless.
pub async fn sweep_expired<C: ConnectionTrait>(db: &C, now: DateTime<Utc>) -> Result<u64, DbErr> {
    Ok(OidcUsedJti::delete_many()
        .filter(oidc_used_jti::Column::ExpiresAt.lt(now.fixed_offset()))
        .exec(db)
        .await?
        .rows_affected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_that_merely_mentions_a_duplicate_is_not_a_replay() {
        // The replay is decided by the driver's error class. A failure whose
        // text happens to contain the words fails closed as a database error.
        let e = DbErr::Custom("connection reset: duplicate unique packet".into());
        assert!(matches!(burn_error(e), OidcError::Db(_)));
    }
}
