//! `oxyc events resync <app> <function>` (app events design §7.3, "Retention
//! horizon"): the way out of every state a table subscription can get stuck
//! in.
//!
//! For each table the function watches in the environment it moves the cursor
//! to the newest snapshot, so what changed since is skipped, not replayed; it
//! clears the cursor's `behind_horizon` or `broken` state and the
//! subscription's circuit-breaker pause. With `backfill` it then queues one
//! synthetic event, `data.backfill: true` with no rows, to that function
//! alone, so its handler runs once and re-derives what it keeps from the whole
//! table. `dry_run` reports what would move and changes nothing.

use entity::{app_event_subscriptions as subs, apps};
use oxy_custom_apps::events::table::TableRef;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use serde::Serialize;

use super::breaker;
use super::cursors::{self, force_place};
use super::publish::{PublishError, publish_backfill};
use super::reader::{ReadError, reader};

/// One table a resync moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Resynced {
    pub table: String,
    pub from: Option<i64>,
    pub to: i64,
    /// The cursor's state before.
    pub state: String,
}

/// What a resync did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub function: String,
    pub environment: String,
    pub tables: Vec<Resynced>,
    pub backfill: bool,
    /// Backfill events queued (one per table).
    pub queued: usize,
    pub dry_run: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ResyncError {
    /// The function watches no table in this environment.
    #[error("`{function}` watches no table in `{environment}`")]
    NothingWatched {
        function: String,
        environment: String,
    },
    /// The watch is declared but its cursor is not made yet (the build has
    /// not been reconciled).
    #[error("`{0}` has no cursor yet; it is made when the build that declares it is live")]
    NoCursor(String),
    /// The workspace has no Airhouse, so there is no table to move a cursor on.
    #[error("the workspace has no Airhouse")]
    NoAirhouse,
    #[error("{0}")]
    Failed(String),
}

impl From<DbErr> for ResyncError {
    fn from(e: DbErr) -> Self {
        Self::Failed(e.to_string())
    }
}

/// The other apps whose subscriptions share a cursor with `function` of `app`
/// in `environment`: a cursor is per (workspace, environment, table), so a
/// resync moves it for all of them, and they would silently skip what changed.
pub async fn sharers(
    db: &DatabaseConnection,
    app: &apps::Model,
    function: &str,
    environment: &str,
) -> Result<Vec<apps::Model>, DbErr> {
    let mine: Vec<String> = subs::Entity::find()
        .filter(subs::Column::AppId.eq(app.id))
        .filter(subs::Column::Environment.eq(environment))
        .filter(subs::Column::FunctionName.eq(function))
        .all(db)
        .await?
        .into_iter()
        .filter(|s| s.event_type.starts_with("table:"))
        .map(|s| s.event_type)
        .collect();
    let others = subs::Entity::find()
        .filter(subs::Column::WorkspaceId.eq(app.project_id))
        .filter(subs::Column::Environment.eq(environment))
        .filter(subs::Column::EventType.is_in(mine))
        .filter(subs::Column::AppId.ne(app.id))
        .all(db)
        .await?;
    let ids: std::collections::BTreeSet<_> = others.iter().map(|s| s.app_id).collect();
    apps::Entity::find()
        .filter(apps::Column::Id.is_in(ids))
        .all(db)
        .await
}

/// Resync `function` of `app` in `environment`.
pub async fn resync(
    db: &DatabaseConnection,
    app: &apps::Model,
    function: &str,
    environment: &str,
    backfill: bool,
    dry_run: bool,
) -> Result<Report, ResyncError> {
    let watched: Vec<TableRef> = subs::Entity::find()
        .filter(subs::Column::AppId.eq(app.id))
        .filter(subs::Column::Environment.eq(environment))
        .filter(subs::Column::FunctionName.eq(function))
        .all(db)
        .await?
        .into_iter()
        .filter_map(|s| TableRef::from_event_type(&s.event_type))
        .collect();
    if watched.is_empty() {
        return Err(ResyncError::NothingWatched {
            function: function.to_string(),
            environment: environment.to_string(),
        });
    }
    let reader = reader();
    let head = match reader.snapshots(app.project_id).await {
        Ok(range) => range.newest,
        Err(ReadError::NotConfigured) => {
            return Err(ResyncError::NoAirhouse);
        }
        Err(e) => return Err(ResyncError::Failed(e.to_string())),
    };
    let mut report = Report {
        function: function.to_string(),
        environment: environment.to_string(),
        tables: Vec::new(),
        backfill,
        queued: 0,
        dry_run,
    };
    // Before anything is queued: a backfill delivered to a paused subscription
    // would be held and never run.
    if !dry_run {
        breaker::resume(db, app.id, function, environment).await?;
    }
    for table in watched {
        let cursor = cursors::of_environment(db, app.org_id, environment)
            .await?
            .into_iter()
            .find(|c| c.workspace_id == app.project_id && c.table_ref == table.spec())
            .ok_or_else(|| ResyncError::NoCursor(table.spec()))?;
        report.tables.push(Resynced {
            table: table.spec(),
            from: cursor.snapshot_id,
            to: head,
            state: cursor.state.clone(),
        });
        if dry_run {
            continue;
        }
        force_place(db, cursor.id, head).await?;
        if backfill {
            let listener = (app.id, function.to_string());
            match publish_backfill(db, &cursor, &table, head, listener).await {
                Ok(_) => report.queued += 1,
                Err(PublishError::NoStore) => {}
                Err(e) => return Err(ResyncError::Failed(e.to_string())),
            }
        }
    }
    Ok(report)
}
