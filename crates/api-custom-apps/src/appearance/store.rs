//! The person's mode in `user_ui_preferences`: read, write. One row per
//! person, written lazily; no row (or a null column) is "not chosen".

use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement};
use uuid::Uuid;

use super::ColorMode;

/// The person's stored mode, or `None` when they have not chosen. A stored
/// value that is not one of the three (the column's `CHECK` keeps them out; a
/// hand-edited row could still differ) reads as not chosen rather than failing
/// every page that asks.
pub async fn load<C: ConnectionTrait>(db: &C, user_id: Uuid) -> Result<Option<ColorMode>, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT color_mode FROM user_ui_preferences WHERE user_id = $1",
            [user_id.into()],
        ))
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let stored: Option<String> = row.try_get("", "color_mode")?;
    Ok(stored.as_deref().and_then(ColorMode::parse))
}

/// Store `mode` as the person's choice. One statement, so two tabs saving at
/// once leave one of them.
pub async fn save<C: ConnectionTrait>(db: &C, user_id: Uuid, mode: ColorMode) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO user_ui_preferences (user_id, color_mode, updated_at) \
         VALUES ($1, $2, now()) \
         ON CONFLICT (user_id) DO UPDATE SET color_mode = EXCLUDED.color_mode, \
           updated_at = now()",
        [user_id.into(), mode.as_str().into()],
    ))
    .await?;
    Ok(())
}
