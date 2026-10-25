//! Where custom-app usage is read from: the Activity tab and the weekly usage
//! report ask here, and one rule picks the store.
//!
//! # The rule
//!
//! A workspace whose `custom_app_usage_backfill` row says `finished_at` reads
//! its own Airhouse table `oxy_custom_app_usage`; every other workspace — no
//! row, a pass still running, no Airhouse at all — reads Oxy's Postgres
//! (`custom_app_view_event`, `custom_app_event`). [`stores`] is that rule and
//! the only place it is written: every function below starts from it.
//!
//! Postgres still receives every write, so it is also the fallback. An Airhouse
//! that cannot be read (down, slow past [`READ_TIMEOUT`](crate::usage_airhouse::READ_TIMEOUT),
//! the tenant gone) is a `warn!` and the same question put to Postgres, not an
//! error on the tab or a missing report. What that fallback cannot do is repair
//! a fact the best-effort Airhouse append dropped after the backfill finished;
//! until a reconciliation exists, the two stores can differ by those rows.
//!
//! # Two queries for one question
//!
//! Each read exists once per store. They are kept side by side, and the tests
//! (`custom_app_usage_reads`, the report's `usage_report_airhouse`) put the same
//! history in both and require the same numbers.
//!
//! * **Airhouse is DuckLake**: no index, primary key, unique constraint or
//!   foreign key in the table or in a query, so nothing makes `event_id`
//!   unique. Every Airhouse read keeps one row per id (`QUALIFY row_number()
//!   OVER (PARTITION BY event_id ORDER BY recorded_at DESC) = 1`), because two
//!   replicas racing the same id can both pass the insert's check.
//! * **Literals, not parameters**: DuckLake's pgwire has no `$n`. Every value in
//!   the SQL is built from a uuid, an integer or an escaped string.
//! * **Text in, text out**: a cell comes back as text, so a timestamp is
//!   selected as `epoch_us(…)` and read as microseconds.
//! * **Microsecond precision**: Postgres and DuckDB keep microseconds, a clock
//!   reading carries nanoseconds. Every window edge goes through
//!   [`micros`](crate::usage_airhouse::micros) before it is compared in either
//!   store, so a row exactly on an edge is on the same side of it in both.
//! * **Staging and sandboxes are never usage**: every query keeps
//!   `environment = 'production'`.

mod activity;
mod report;
mod views;
mod vitals;

pub use activity::*;
pub use report::*;
pub use views::*;
pub use vitals::*;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use chrono::{DateTime, FixedOffset, Utc};
use oxy_cameras::airhouse::escape::sql_str;
use sea_orm::{
    DatabaseBackend, DatabaseConnection, DbErr, EntityTrait, FromQueryResult, QuerySelect,
    Statement,
};
use uuid::Uuid;

use crate::usage_airhouse::{
    self as sink, FAILURE_BACKOFF, READ_TIMEOUT, SinkError, SqlRows, TABLE, ts_literal,
};

pub(super) const PRODUCTION: &str = "production";

/// The store that answers for a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageStore {
    Postgres,
    Airhouse,
}

/// The store that answers for each of `workspace_ids`: Airhouse for those whose
/// backfill row says finished, Postgres for the rest. Every id asked about is
/// in the map.
pub async fn stores(
    db: &DatabaseConnection,
    workspace_ids: &[Uuid],
) -> Result<HashMap<Uuid, UsageStore>, DbErr> {
    #[derive(FromQueryResult)]
    struct Row {
        workspace_id: Uuid,
    }
    let mut out: HashMap<Uuid, UsageStore> = workspace_ids
        .iter()
        .map(|id| (*id, UsageStore::Postgres))
        .collect();
    if workspace_ids.is_empty() {
        return Ok(out);
    }
    let finished = Row::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT workspace_id FROM custom_app_usage_backfill \
         WHERE workspace_id = ANY($1) AND finished_at IS NOT NULL",
        [workspace_ids.to_vec().into()],
    ))
    .all(db)
    .await?;
    for row in finished {
        // A workspace whose Airhouse just failed to answer is left on Postgres
        // for a while, so one hung Airhouse does not make every request wait
        // out the read timeout first.
        if !in_read_backoff(row.workspace_id) {
            out.insert(row.workspace_id, UsageStore::Airhouse);
        }
    }
    Ok(out)
}

/// Workspace -> when its Airhouse may be asked again after a failed read.
static READ_BACKOFF: LazyLock<Mutex<HashMap<Uuid, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn in_read_backoff(workspace_id: Uuid) -> bool {
    let mut map = READ_BACKOFF.lock().unwrap_or_else(|e| e.into_inner());
    match map.get(&workspace_id) {
        Some(until) if *until > Instant::now() => true,
        Some(_) => {
            map.remove(&workspace_id);
            false
        }
        None => false,
    }
}

fn start_read_backoff(workspace_id: Uuid) {
    READ_BACKOFF
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(workspace_id, Instant::now() + FAILURE_BACKOFF);
}

/// The workspace an app belongs to and the store that answers for it. `None`
/// for an app that does not exist (its reads come back empty from Postgres).
async fn home_of(
    db: &DatabaseConnection,
    app_id: Uuid,
) -> Result<Option<(Uuid, UsageStore)>, DbErr> {
    let workspace_id = entity::apps::Entity::find_by_id(app_id)
        .select_only()
        .column(entity::apps::Column::ProjectId)
        .into_tuple::<Uuid>()
        .one(db)
        .await?;
    let Some(workspace_id) = workspace_id else {
        return Ok(None);
    };
    let store = stores(db, &[workspace_id])
        .await?
        .get(&workspace_id)
        .copied()
        .unwrap_or(UsageStore::Postgres);
    Ok(Some((workspace_id, store)))
}

/// `Some(workspace)` when Airhouse should answer for `app_id`.
pub(super) async fn airhouse_workspace_of(
    db: &DatabaseConnection,
    app_id: Uuid,
) -> Result<Option<Uuid>, DbErr> {
    Ok(match home_of(db, app_id).await? {
        Some((workspace_id, UsageStore::Airhouse)) => Some(workspace_id),
        _ => None,
    })
}

/// An Airhouse read's result, or `None` after saying why it is not used.
pub(super) fn or_fall_back<T>(
    workspace_id: Uuid,
    what: &str,
    result: Result<T, SinkError>,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(e) => {
            start_read_backoff(workspace_id);
            tracing::warn!(
                %workspace_id,
                "custom-app usage: Airhouse could not answer {what} ({e:?}); \
                 reading Postgres instead"
            );
            None
        }
    }
}

pub(super) async fn query(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    sql: &str,
) -> Result<SqlRows, SinkError> {
    let sink = sink::sink();
    tokio::time::timeout(READ_TIMEOUT, sink.query(db, workspace_id, sql))
        .await
        .unwrap_or_else(|_| {
            Err(SinkError::Failed(format!(
                "a read timed out after {}s",
                READ_TIMEOUT.as_secs()
            )))
        })
}

/// A relation of the production facts of `apps` — one row per event id, either
/// kind — occurring at or after `since` and before `until` (each optional).
pub(super) fn facts(
    apps: &[Uuid],
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> String {
    let list = apps
        .iter()
        .map(|a| sql_str(&a.to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    let since = since
        .map(|t| format!(" AND occurred_at >= {}", ts_literal(t)))
        .unwrap_or_default();
    let until = until
        .map(|t| format!(" AND occurred_at < {}", ts_literal(t)))
        .unwrap_or_default();
    format!(
        "SELECT * FROM {TABLE} WHERE app_id IN ({list}) AND environment = '{PRODUCTION}'\
         {since}{until} \
         QUALIFY row_number() OVER (PARTITION BY event_id ORDER BY recorded_at DESC) = 1"
    )
}

/// The apps of each Airhouse workspace, and the apps left to Postgres.
pub(super) struct Split {
    pub(super) airhouse: HashMap<Uuid, Vec<Uuid>>,
    pub(super) postgres: Vec<Uuid>,
}

pub(super) async fn split(db: &DatabaseConnection, apps: &[ReportApp]) -> Result<Split, DbErr> {
    let workspaces: Vec<Uuid> = apps
        .iter()
        .map(|a| a.workspace_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let stores = stores(db, &workspaces).await?;
    let mut out = Split {
        airhouse: HashMap::new(),
        postgres: Vec::new(),
    };
    for app in apps {
        match stores.get(&app.workspace_id) {
            Some(UsageStore::Airhouse) => out
                .airhouse
                .entry(app.workspace_id)
                .or_default()
                .push(app.app_id),
            _ => out.postgres.push(app.app_id),
        }
    }
    Ok(out)
}

/// Rows about `apps`, each app answered by its workspace's store: `airhouse`
/// once per Airhouse workspace with that workspace's apps, all at once, then
/// `postgres` once with every other app and the apps of any Airhouse that
/// could not answer. `what` names the read in the fallback's log line.
pub(super) async fn per_store<T, A, AF, P, PF>(
    db: &DatabaseConnection,
    apps: &[ReportApp],
    what: &str,
    airhouse: A,
    postgres: P,
) -> Result<Vec<T>, DbErr>
where
    A: Fn(Uuid, Vec<Uuid>) -> AF,
    AF: Future<Output = Result<Vec<T>, SinkError>>,
    P: FnOnce(Vec<Uuid>) -> PF,
    PF: Future<Output = Result<Vec<T>, DbErr>>,
{
    if apps.is_empty() {
        return Ok(Vec::new());
    }
    let Split {
        airhouse: by_workspace,
        postgres: mut left,
    } = split(db, apps).await?;
    let answers = futures::future::join_all(by_workspace.iter().map(|(workspace_id, app_ids)| {
        let read = airhouse(*workspace_id, app_ids.clone());
        async move { (*workspace_id, read.await) }
    }))
    .await;
    let mut out = Vec::new();
    for (workspace_id, result) in answers {
        match or_fall_back(workspace_id, what, result) {
            Some(found) => out.extend(found),
            None => left.extend(by_workspace[&workspace_id].iter().copied()),
        }
    }
    if !left.is_empty() {
        out.extend(postgres(left).await?);
    }
    Ok(out)
}

pub(super) fn at(t: DateTime<Utc>) -> DateTime<FixedOffset> {
    t.fixed_offset()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn the_deduplicating_relation_names_no_key_or_index() {
        let app = Uuid::from_u128(5);
        let sql = facts(
            &[app],
            Some(Utc.timestamp_opt(1_790_000_000, 123_456_789).unwrap()),
            None,
        );
        let upper = sql.to_ascii_uppercase();
        for forbidden in [
            "PRIMARY KEY",
            "UNIQUE",
            "INDEX",
            "REFERENCES",
            "FOREIGN KEY",
        ] {
            assert!(!upper.contains(forbidden), "{forbidden} in {sql}");
        }
        assert!(sql.contains("QUALIFY row_number() OVER (PARTITION BY event_id"));
        assert!(sql.contains("environment = 'production'"));
        assert!(sql.contains(&format!("'{app}'")));
        // The edge is cut to microseconds, the precision both stores keep.
        assert!(sql.contains(".123456+00:00'"), "{sql}");
        assert!(!sql.contains(".123456789"), "{sql}");
    }

    /// The files allowed to read the Postgres usage tables, relative to
    /// `crates/`: the reads that choose a store, the backfill that copies the
    /// history to Airhouse, and the seed fixture checking its own sentinel row.
    const READERS: [&str; 3] = [
        "custom-apps/src/usage_reads/",
        "custom-apps/src/usage_backfill/",
        "api-custom-apps/src/seed_fixture/",
    ];

    /// A read of a Postgres usage table in `line`: SQL naming one after `FROM`
    /// or `JOIN`, or a SeaORM `find` on its entity.
    fn reads_a_usage_table(line: &str) -> Option<&'static str> {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        for table in ["custom_app_view_event", "custom_app_event"] {
            let named = |prefix: &str| {
                let needle = format!("{prefix}{table}");
                line.match_indices(&needle).any(|(i, m)| {
                    !line[i + m.len()..]
                        .starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
                })
            };
            if named("FROM ") || named("JOIN ") || line.contains(&format!("{table}::Entity::find"))
            {
                return Some(table);
            }
        }
        None
    }

    fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() {
                if name != "tests" && name != "target" && name != "node_modules" {
                    rust_files(&path, out);
                }
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn the_scanner_sees_sql_and_entity_reads_and_nothing_else() {
        assert_eq!(
            reads_a_usage_table("SELECT 1 FROM custom_app_view_event v"),
            Some("custom_app_view_event")
        );
        assert_eq!(
            reads_a_usage_table("  JOIN   custom_app_event e ON"),
            Some("custom_app_event")
        );
        assert_eq!(
            reads_a_usage_table("let rows = custom_app_event::Entity::find()"),
            Some("custom_app_event")
        );
        assert_eq!(reads_a_usage_table("FROM custom_app_events"), None);
        assert_eq!(reads_a_usage_table("custom_app_event::ActiveModel {"), None);
        assert_eq!(
            reads_a_usage_table("INSERT INTO custom_app_view_event"),
            None
        );
    }

    /// Every read of `custom_app_view_event` and `custom_app_event` goes
    /// through [`stores`], so a workspace whose usage lives in Airhouse is
    /// never read from a Postgres table that stopped receiving its rows. A new
    /// reader belongs in this module; this test fails where one appears
    /// anywhere else outside a `tests/` directory.
    #[test]
    fn no_reader_outside_usage_reads_queries_the_postgres_usage_tables() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let mut files = Vec::new();
        rust_files(&crates, &mut files);
        assert!(files.len() > 100, "the scan found the workspace's crates");
        let mut found = Vec::new();
        for file in files {
            let relative = file
                .strip_prefix(&crates)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if READERS.iter().any(|allowed| relative.starts_with(allowed)) {
                continue;
            }
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            for (n, line) in text.lines().enumerate() {
                if let Some(table) = reads_a_usage_table(line) {
                    found.push(format!("{relative}:{} reads {table}", n + 1));
                }
            }
        }
        assert!(
            found.is_empty(),
            "read the usage tables through `oxy_custom_apps::usage_reads`, which \
             picks each workspace's store:\n{}",
            found.join("\n")
        );
    }
}
