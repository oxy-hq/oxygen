//! Where a workspace's new usage rows are written.
//!
//! A workspace whose usage history has been copied to its Airhouse (its
//! `custom_app_usage_backfill` row has `finished_at`) is read from Airhouse
//! ([`usage_reads`](crate::usage_reads)), so its new page views and events are
//! written there **only**: the drain ([`usage_queue`](crate::usage_queue))
//! appends them and writes no Postgres row. Every other workspace is written
//! to Postgres as before, with the best-effort Airhouse copy behind it.
//!
//! **A failed append parks the rows.** When a workspace's append does not land
//! (an error, a timeout, the workspace in its failure backoff), the drain
//! writes those rows to Postgres instead, under the same ids, so none is lost,
//! and [`park`] records the oldest of them in `parked_since`. The backfill
//! sweep queues a pass for every finished workspace with `parked_since` set,
//! and that pass copies the workspace's Postgres rows from `parked_since` on
//! to Airhouse ([`usage_backfill`](crate::usage_backfill)). Until it does, the
//! parked rows are in Postgres and not in what the reads see.
//!
//! **What reads see while an Airhouse is down.** The reads fall back to
//! Postgres when Airhouse cannot answer; for a workspace that writes Airhouse
//! only, Postgres then holds the history up to its backfill and any parked
//! rows, not everything since. A stale answer for the length of the outage,
//! not an error.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement};
use uuid::Uuid;

use crate::usage_airhouse as sink;

/// The workspaces among `workspace_ids` that write their usage to Airhouse
/// only. Empty when no sink is enabled. `None` when the database could not say:
/// the caller then writes Postgres, which is always safe, and marks any row
/// whose Airhouse copy did not land with [`park_apps`], so a backfill pass
/// copies it if its workspace turns out to read Airhouse.
pub async fn airhouse_only(
    db: &DatabaseConnection,
    workspace_ids: &[Uuid],
) -> Option<HashSet<Uuid>> {
    if workspace_ids.is_empty() || !sink::enabled() {
        return Some(HashSet::new());
    }
    #[derive(FromQueryResult)]
    struct Row {
        workspace_id: Uuid,
    }
    match Row::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT workspace_id FROM custom_app_usage_backfill \
         WHERE workspace_id = ANY($1) AND finished_at IS NOT NULL",
        [workspace_ids.to_vec().into()],
    ))
    .all(db)
    .await
    {
        Ok(rows) => Some(rows.into_iter().map(|r| r.workspace_id).collect()),
        Err(e) => {
            tracing::warn!(
                "custom-app usage: could not read which workspaces write Airhouse only; \
                 writing Postgres: {e}"
            );
            None
        }
    }
}

/// How long [`airhouse_only_home`] keeps an answer. A workspace whose backfill
/// finishes keeps writing Postgres (with its Airhouse copy) for at most this
/// long, which is safe; nothing ever moves a workspace back.
const HOME_TTL: Duration = Duration::from_secs(30);
/// Apps [`airhouse_only_home`] remembers before it starts over.
const HOME_CACHE_MAX: usize = 10_000;

type Home = Option<(Uuid, Uuid)>;

static HOMES: LazyLock<Mutex<HashMap<Uuid, (Instant, Home)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The workspace and org of `app_id` when that workspace writes Airhouse only.
/// `None` for every other app, an app that no longer exists, and on a database
/// error. One query, remembered for [`HOME_TTL`]: `record_event` asks on every
/// SDK event.
pub async fn airhouse_only_home(db: &DatabaseConnection, app_id: Uuid) -> Home {
    if !sink::enabled() {
        return None;
    }
    let now = Instant::now();
    if let Some((at, home)) = HOMES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&app_id)
        .copied()
        && now.duration_since(at) < HOME_TTL
    {
        return home;
    }
    #[derive(FromQueryResult)]
    struct Row {
        workspace_id: Uuid,
        org_id: Uuid,
    }
    let found = Row::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT a.project_id AS workspace_id, a.org_id FROM apps a \
         JOIN custom_app_usage_backfill b \
           ON b.workspace_id = a.project_id AND b.finished_at IS NOT NULL \
         WHERE a.id = $1",
        [app_id.into()],
    ))
    .one(db)
    .await;
    match found {
        Ok(row) => {
            let home = row.map(|r| (r.workspace_id, r.org_id));
            let mut cache = HOMES.lock().unwrap_or_else(|e| e.into_inner());
            if cache.len() >= HOME_CACHE_MAX {
                cache.clear();
            }
            cache.insert(app_id, (now, home));
            home
        }
        Err(e) => {
            tracing::warn!(%app_id, "custom-app usage: could not read where the app's usage goes; writing Postgres: {e}");
            None
        }
    }
}

/// Record that rows were parked in Postgres for each workspace, the oldest at
/// the given time. `parked_since` keeps the older of what it holds and this.
/// The time is cut to microseconds: Postgres rounds a row's time to the
/// nearest one, so the cut is never later than the row.
/// `updated_at` moves too, which tells a backfill pass that started before now
/// that more rows were parked while it ran.
pub async fn park(db: &DatabaseConnection, oldest: &HashMap<Uuid, DateTime<Utc>>) {
    for (workspace_id, at) in oldest {
        let result = db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE custom_app_usage_backfill \
                 SET parked_since = LEAST(COALESCE(parked_since, $2), $2), updated_at = now() \
                 WHERE workspace_id = $1",
                [(*workspace_id).into(), sink::micros(*at).into()],
            ))
            .await;
        if let Err(e) = result {
            tracing::warn!(
                %workspace_id,
                "custom-app usage: rows were written to Postgres after a failed Airhouse \
                 append, but recording it failed, so no pass will copy them: {e}"
            );
        }
    }
}

/// [`park`] by app, for rows already in Postgres whose Airhouse copy did not
/// land and whose workspace the caller may not know. Marks only a workspace
/// whose backfill finished: any other still reads Postgres, where the rows are.
pub async fn park_apps(db: &DatabaseConnection, oldest: &HashMap<Uuid, DateTime<Utc>>) {
    if oldest.is_empty() {
        return;
    }
    let apps: Vec<Uuid> = oldest.keys().copied().collect();
    let times: Vec<DateTime<Utc>> = apps.iter().map(|a| sink::micros(oldest[a])).collect();
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE custom_app_usage_backfill b \
             SET parked_since = LEAST(COALESCE(b.parked_since, p.at), p.at), updated_at = now() \
             FROM (SELECT a.project_id AS workspace_id, min(x.at) AS at \
                   FROM unnest($1::uuid[], $2::timestamptz[]) AS x(app_id, at) \
                   JOIN apps a ON a.id = x.app_id GROUP BY a.project_id) p \
             WHERE b.workspace_id = p.workspace_id AND b.finished_at IS NOT NULL",
            [apps.into(), times.into()],
        ))
        .await;
    if let Err(e) = result {
        tracing::warn!(
            apps = oldest.len(),
            "custom-app usage: rows in Postgres whose Airhouse copy did not land could not be \
             marked, so no pass will copy them: {e}"
        );
    }
}
