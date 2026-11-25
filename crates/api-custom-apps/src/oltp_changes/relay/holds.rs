//! Holds (OLTP change stream design §5.3): the tables, or whole writers, the
//! relay must stop shipping, each with the reason it was held. A hold is kept
//! until someone removes it, and keeps the reason it was first made with.

use std::collections::{BTreeMap, BTreeSet};

use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};
use uuid::Uuid;

/// An (org, environment)'s holds, read once per pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Holds {
    /// Writers whose whole stream is held.
    writers: BTreeSet<String>,
    /// Held tables, by writer.
    tables: BTreeMap<String, BTreeSet<String>>,
}

impl Holds {
    /// Is `w`'s whole stream held?
    pub fn writer(&self, w: &str) -> bool {
        self.writers.contains(w)
    }

    /// The tables of `w` held on their own, sorted.
    pub fn tables(&self, w: &str) -> Vec<String> {
        self.tables
            .get(w)
            .map(|t| t.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// Read every hold of `org_id`'s `environment`. A `table_name` of `''` is the
/// writer's whole stream.
pub async fn read(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    environment: &str,
) -> Result<Holds, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT writer, table_name FROM oltp_change_holds \
              WHERE org_id = $1 AND environment = $2",
            [org_id.into(), environment.into()],
        ))
        .await?;
    let mut holds = Holds::default();
    for row in &rows {
        let writer: String = row.try_get("", "writer")?;
        let table: String = row.try_get("", "table_name")?;
        if table.is_empty() {
            holds.writers.insert(writer);
        } else {
            holds.tables.entry(writer).or_default().insert(table);
        }
    }
    Ok(holds)
}

/// What one hold is on, its row's key: a `table` of a `writer` in an
/// (org, environment), or the writer's whole stream when `table` is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoldKey<'a> {
    pub org_id: Uuid,
    pub environment: &'a str,
    pub writer: &'a str,
    pub table: Option<&'a str>,
}

/// Hold what `key` names. Returns whether the hold is new: an existing one
/// keeps its first reason, and the caller logs `oltp_changes_held` only for a
/// new one.
pub async fn hold(
    db: &impl ConnectionTrait,
    key: HoldKey<'_>,
    reason: &str,
) -> Result<bool, DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO oltp_change_holds (org_id, environment, writer, table_name, reason) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (org_id, environment, writer, table_name) DO NOTHING",
        [
            key.org_id.into(),
            key.environment.into(),
            key.writer.into(),
            key.table.unwrap_or("").into(),
            reason.into(),
        ],
    ))
    .await
    .map(|r| r.rows_affected() == 1)
}
