//! Reading an app's attempts back, the way they are kept.
//!
//! Retention keeps the newest [`KEEP_ATTEMPTS`] of **each** outcome
//! (`record::prune`), so the read takes the newest `limit` of each and merges
//! them newest first. One window across both outcomes would let a CI job
//! failing in a loop hide the stage timings of every build that landed, and a
//! run of good publishes hide the last failures — the two things the console
//! reads this for.

use entity::app_publish_attempts::{self, OUTCOME_FAILED, OUTCOME_SUCCESS};
use sea_orm::prelude::Uuid;
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};

use super::KEEP_ATTEMPTS;

/// How many attempts of each outcome a read answers when it asks for no
/// number: everything retention keeps.
pub const DEFAULT_LIST_LIMIT: u64 = KEEP_ATTEMPTS;
/// The most attempts of each outcome one read answers. No more are kept.
pub const MAX_LIST_LIMIT: u64 = KEEP_ATTEMPTS;

/// The `limit` a read asked for, held to `1..=MAX_LIST_LIMIT`.
pub fn clamp_limit(asked: Option<u64>) -> u64 {
    asked.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, MAX_LIST_LIMIT)
}

/// The app's newest `limit` attempts of each outcome, newest first.
pub async fn recent(
    db: &DatabaseConnection,
    app_id: Uuid,
    limit: u64,
) -> Result<Vec<app_publish_attempts::Model>, DbErr> {
    let landed = newest(db, app_id, OUTCOME_SUCCESS, limit).await?;
    let failed = newest(db, app_id, OUTCOME_FAILED, limit).await?;
    Ok(newest_first(landed, failed))
}

async fn newest(
    db: &DatabaseConnection,
    app_id: Uuid,
    outcome: &str,
    limit: u64,
) -> Result<Vec<app_publish_attempts::Model>, DbErr> {
    app_publish_attempts::Entity::find()
        .filter(app_publish_attempts::Column::AppId.eq(app_id))
        .filter(app_publish_attempts::Column::Outcome.eq(outcome))
        .order_by_desc(app_publish_attempts::Column::StartedAt)
        .limit(limit)
        .all(db)
        .await
}

/// Two windows merged into one list, newest first.
pub(super) fn newest_first(
    a: Vec<app_publish_attempts::Model>,
    b: Vec<app_publish_attempts::Model>,
) -> Vec<app_publish_attempts::Model> {
    let mut all: Vec<_> = a.into_iter().chain(b).collect();
    all.sort_by(|x, y| y.started_at.cmp(&x.started_at));
    all
}

#[cfg(test)]
#[path = "list_tests.rs"]
mod tests;
