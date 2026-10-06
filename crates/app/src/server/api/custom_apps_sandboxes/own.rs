//! Hold a write to a sandbox **the sandbox agent token created**, decided on
//! the row while it is locked.
//!
//! A request by such a token is admitted against the sandbox as it was when
//! the request arrived (`may_open_environment`). A write that lands later is
//! not covered by that: in between, the token's sandbox may have finished its
//! teardown and a colleague created another under the same name. So a write a
//! token makes by name — a pointer move, a secret set or delete — takes the
//! sandbox's row lock first and looks again. While the lock is held the row
//! cannot be marked deleting, and a delete cannot free the name.
//!
//! A delete does the same on the lock it already takes (`delete`).

use entity::app_environments;
use oxy_app_core::custom_app_environment::{AppEnvironment, AppEnvironmentKind};
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QuerySelect};
use uuid::Uuid;

/// Lock the sandbox's row for the rest of `txn`, and answer whether it is a
/// live sandbox `token` created. `false` when the row is another creator's,
/// is being torn down, or is gone.
pub(crate) async fn lock_own<C: ConnectionTrait>(
    txn: &C,
    app_id: Uuid,
    environment: &AppEnvironment,
    token: Uuid,
) -> Result<bool, DbErr> {
    let row = app_environments::Entity::find_by_id((app_id, environment.name()))
        .filter(app_environments::Column::Kind.eq(AppEnvironmentKind::Dev.as_str()))
        .lock_exclusive()
        .one(txn)
        .await?;
    Ok(owns(row.as_ref(), token))
}

/// Whether `row` is a live sandbox `token` created.
fn owns(row: Option<&app_environments::Model>, token: Uuid) -> bool {
    row.is_some_and(|row| row.deleting_at.is_none() && row.created_by_token_id == Some(token))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    const TOKEN: Uuid = Uuid::from_u128(0x70);

    fn row(created_by_token_id: Option<Uuid>, deleting: bool) -> app_environments::Model {
        let now = Utc::now().fixed_offset();
        app_environments::Model {
            app_id: Uuid::from_u128(1),
            name: "dev-a".into(),
            kind: AppEnvironmentKind::Dev.as_str().into(),
            owner_user_id: None,
            build_id: None,
            updated_by: None,
            updated_at: now,
            created_at: now,
            deleting_at: deleting.then_some(now),
            oltp_schema: None,
            created_by_token_id,
        }
    }

    /// A write is held to the row it locks: live, and created by this token.
    /// A colleague's sandbox under the same name, another token's, one being
    /// torn down and no row at all are all not the token's.
    #[test]
    fn only_a_live_row_the_token_created_is_its_own() {
        assert!(owns(Some(&row(Some(TOKEN), false)), TOKEN));
        assert!(!owns(Some(&row(Some(TOKEN), true)), TOKEN));
        assert!(!owns(Some(&row(None, false)), TOKEN));
        assert!(!owns(Some(&row(Some(Uuid::from_u128(0x71)), false)), TOKEN));
        assert!(!owns(None, TOKEN));
    }
}
