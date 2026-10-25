//! The Postgres side of a pass: where it is, and the next batch of rows after it.

use chrono::{DateTime, Utc};
use entity::{apps, custom_app_event, custom_app_usage_backfill, custom_app_view_event};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect, Statement,
};
use uuid::Uuid;

use super::{EVENT, VIEW};
use crate::usage_airhouse::UsageFact;

/// Where the last batch ended. `kind` is `None` before anything was handed
/// over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Position {
    pub(super) kind: Option<&'static str>,
    pub(super) app_id: Uuid,
    pub(super) at: DateTime<Utc>,
    pub(super) id: Uuid,
}

impl Default for Position {
    fn default() -> Self {
        Self {
            kind: None,
            app_id: Uuid::nil(),
            at: DateTime::UNIX_EPOCH,
            id: Uuid::nil(),
        }
    }
}

impl Position {
    /// Before the first row of `kind`: the nil app and the epoch sort before
    /// every real row.
    pub(super) fn start_of(kind: &'static str) -> Self {
        Self {
            kind: Some(kind),
            ..Self::default()
        }
    }

    pub(super) fn of(row: &custom_app_usage_backfill::Model) -> Self {
        let kind = match row.cursor_kind.as_deref() {
            Some(VIEW) => Some(VIEW),
            Some(EVENT) => Some(EVENT),
            _ => None,
        };
        match (kind, row.cursor_app_id, row.cursor_at, row.cursor_id) {
            (Some(kind), Some(app_id), Some(at), Some(id)) => Self {
                kind: Some(kind),
                app_id,
                at: at.with_timezone(&Utc),
                id,
            },
            _ => Self::default(),
        }
    }
}

pub(super) async fn save(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    handed: u64,
    at: &Position,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE custom_app_usage_backfill \
         SET rows_copied = rows_copied + $2, cursor_kind = $3, cursor_app_id = $4, \
             cursor_at = $5, cursor_id = $6, updated_at = now() \
         WHERE workspace_id = $1",
        [
            workspace_id.into(),
            (handed as i64).into(),
            at.kind.map(str::to_string).into(),
            at.app_id.into(),
            at.at.into(),
            at.id.into(),
        ],
    ))
    .await?;
    Ok(())
}

/// The workspace's apps with the org each belongs to, by app id.
pub(super) async fn app_homes(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<Vec<(Uuid, Uuid)>, DbErr> {
    apps::Entity::find()
        .select_only()
        .columns([apps::Column::Id, apps::Column::OrgId])
        .filter(apps::Column::ProjectId.eq(workspace_id))
        .order_by_asc(apps::Column::Id)
        .into_tuple::<(Uuid, Uuid)>()
        .all(db)
        .await
}

pub(super) fn org_of(homes: &[(Uuid, Uuid)], app_id: Uuid) -> Option<Uuid> {
    homes
        .iter()
        .find(|(app, _)| *app == app_id)
        .map(|(_, o)| *o)
}

pub(super) async fn view_batch(
    db: &DatabaseConnection,
    homes: &[(Uuid, Uuid)],
    after: &Position,
    limit: u64,
    recorded_at: DateTime<Utc>,
) -> Result<(Vec<UsageFact>, Option<Position>), DbErr> {
    if homes.is_empty() {
        return Ok((Vec::new(), None));
    }
    let rows = custom_app_view_event::Entity::find()
        .from_raw_sql(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT * FROM custom_app_view_event \
             WHERE app_id = ANY($1) AND (app_id, viewed_at, id) > ($2, $3, $4) \
             ORDER BY app_id, viewed_at, id LIMIT $5",
            [
                homes.iter().map(|(app, _)| *app).collect::<Vec<_>>().into(),
                after.app_id.into(),
                after.at.into(),
                after.id.into(),
                (limit as i64).into(),
            ],
        ))
        .all(db)
        .await?;
    let last = rows.last().map(|r| Position {
        kind: Some(VIEW),
        app_id: r.app_id,
        at: r.viewed_at.with_timezone(&Utc),
        id: r.id,
    });
    let facts = rows
        .iter()
        .filter_map(|r| {
            org_of(homes, r.app_id).map(|org| UsageFact::from_view_model(r, org, recorded_at))
        })
        .collect();
    Ok((facts, last))
}

pub(super) async fn event_batch(
    db: &DatabaseConnection,
    homes: &[(Uuid, Uuid)],
    after: &Position,
    limit: u64,
    recorded_at: DateTime<Utc>,
) -> Result<(Vec<UsageFact>, Option<Position>), DbErr> {
    if homes.is_empty() {
        return Ok((Vec::new(), None));
    }
    let rows = custom_app_event::Entity::find()
        .from_raw_sql(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT * FROM custom_app_event \
             WHERE app_id = ANY($1) AND (app_id, occurred_at, id) > ($2, $3, $4) \
             ORDER BY app_id, occurred_at, id LIMIT $5",
            [
                homes.iter().map(|(app, _)| *app).collect::<Vec<_>>().into(),
                after.app_id.into(),
                after.at.into(),
                after.id.into(),
                (limit as i64).into(),
            ],
        ))
        .all(db)
        .await?;
    let last = rows.last().map(|r| Position {
        kind: Some(EVENT),
        app_id: r.app_id,
        at: r.occurred_at.with_timezone(&Utc),
        id: r.id,
    });
    let facts = rows
        .iter()
        .filter_map(|r| {
            org_of(homes, r.app_id).map(|org| UsageFact::from_event_model(r, org, recorded_at))
        })
        .collect();
    Ok((facts, last))
}

/// Up to `limit` of the workspace's rows of `kind` after `after` (a time and
/// an id), in time order, where the last one is, and how many rows were read. A parked-row pass reads
/// from the oldest parked row on, every app of the workspace at once: after a
/// workspace stops writing Postgres, the only rows there are parked ones.
pub(super) async fn parked_batch(
    db: &DatabaseConnection,
    kind: &'static str,
    homes: &[(Uuid, Uuid)],
    after: (DateTime<Utc>, Uuid),
    limit: u64,
    recorded_at: DateTime<Utc>,
) -> Result<(Vec<UsageFact>, Option<(DateTime<Utc>, Uuid)>, usize), DbErr> {
    if homes.is_empty() {
        return Ok((Vec::new(), None, 0));
    }
    let values = || {
        [
            homes.iter().map(|(app, _)| *app).collect::<Vec<_>>().into(),
            after.0.into(),
            after.1.into(),
            (limit as i64).into(),
        ]
    };
    if kind == VIEW {
        let rows = custom_app_view_event::Entity::find()
            .from_raw_sql(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT * FROM custom_app_view_event \
                 WHERE app_id = ANY($1) AND (viewed_at, id) > ($2, $3) \
                 ORDER BY viewed_at, id LIMIT $4",
                values(),
            ))
            .all(db)
            .await?;
        let last = rows.last().map(|r| (r.viewed_at.with_timezone(&Utc), r.id));
        let facts = rows
            .iter()
            .filter_map(|r| {
                org_of(homes, r.app_id).map(|org| UsageFact::from_view_model(r, org, recorded_at))
            })
            .collect();
        return Ok((facts, last, rows.len()));
    }
    let rows = custom_app_event::Entity::find()
        .from_raw_sql(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT * FROM custom_app_event \
             WHERE app_id = ANY($1) AND (occurred_at, id) > ($2, $3) \
             ORDER BY occurred_at, id LIMIT $4",
            values(),
        ))
        .all(db)
        .await?;
    let last = rows
        .last()
        .map(|r| (r.occurred_at.with_timezone(&Utc), r.id));
    let facts = rows
        .iter()
        .filter_map(|r| {
            org_of(homes, r.app_id).map(|org| UsageFact::from_event_model(r, org, recorded_at))
        })
        .collect();
    Ok((facts, last, rows.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_or_partial_cursor_starts_from_the_beginning() {
        let row = custom_app_usage_backfill::Model {
            workspace_id: Uuid::nil(),
            started_at: Utc::now().fixed_offset(),
            finished_at: None,
            rows_copied: 0,
            cursor_kind: Some("view".into()),
            cursor_app_id: None,
            cursor_at: None,
            cursor_id: None,
            updated_at: Utc::now().fixed_offset(),
            parked_since: None,
        };
        assert_eq!(Position::of(&row), Position::default());
        let row = custom_app_usage_backfill::Model {
            cursor_kind: Some("something-else".into()),
            ..row
        };
        assert_eq!(Position::of(&row).kind, None);
    }

    #[test]
    fn the_start_of_a_kind_sorts_before_every_row() {
        let start = Position::start_of(EVENT);
        assert_eq!(start.kind, Some(EVENT));
        assert_eq!(start.app_id, Uuid::nil());
        assert_eq!(start.at, DateTime::UNIX_EPOCH);
    }
}
