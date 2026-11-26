//! Holds (OLTP change stream design §5.3): the tables, or whole writers, the
//! relay must stop shipping, each with the reason it was held. A hold is kept
//! until someone removes it, and keeps the reason it was first made with.

use std::collections::{BTreeMap, BTreeSet};

use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};
use uuid::Uuid;

/// Why a hold was made, which decides what may lift it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldKind {
    /// A table the relay refused to ship.
    Table,
    /// A writer whose oldest transaction is past the relay's ceiling.
    Ceiling,
    /// A writer whose store was found restored.
    Restore,
    /// A writer with no app or no Airhouse to ship to.
    Target,
}

impl HoldKind {
    /// The `kind` column's value.
    pub fn as_str(self) -> &'static str {
        match self {
            HoldKind::Table => "table",
            HoldKind::Ceiling => "ceiling",
            HoldKind::Restore => "restore",
            HoldKind::Target => "target",
        }
    }

    fn parse(text: &str) -> HoldKind {
        match text {
            "ceiling" => HoldKind::Ceiling,
            "restore" => HoldKind::Restore,
            "target" => HoldKind::Target,
            _ => HoldKind::Table,
        }
    }

    /// A hold row's kind: its `kind`, except a writer's whole-stream hold an
    /// older binary wrote, whose `kind` is the column's default (`table`):
    /// that one is classified by its `reason`, as the migration's backfill
    /// classified the rows before the column ([`KIND_SQL`]).
    pub fn of_row(table: &str, kind: &str, reason: &str) -> HoldKind {
        let kind = HoldKind::parse(kind);
        if !table.is_empty() || kind != HoldKind::Table {
            return kind;
        }
        if reason.starts_with(RESTORE_REASON) {
            HoldKind::Restore
        } else if reason.starts_with(CEILING_REASON) && reason.contains(" rows exceeds") {
            HoldKind::Ceiling
        } else {
            HoldKind::Target
        }
    }
}

/// How PR 2's restore hold reasons begin.
const RESTORE_REASON: &str = "restore detected (";
/// How PR 2's ceiling hold reasons begin; `" rows exceeds"` follows the count.
const CEILING_REASON: &str = "a transaction of ";

/// [`HoldKind::of_row`] in SQL, over a row of `oltp_change_holds`.
const KIND_SQL: &str = "CASE WHEN table_name <> '' OR kind <> 'table' THEN kind \
     WHEN reason LIKE 'restore detected (%' THEN 'restore' \
     WHEN reason LIKE 'a transaction of % rows exceeds%' THEN 'ceiling' \
     ELSE 'target' END";

/// An (org, environment)'s holds, read once per pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Holds {
    /// Writers whose whole stream is held, with why.
    writers: BTreeMap<String, HoldKind>,
    /// Held tables, by writer.
    tables: BTreeMap<String, BTreeSet<String>>,
}

impl Holds {
    /// Is `w`'s whole stream held?
    pub fn writer(&self, w: &str) -> bool {
        self.writers.contains_key(w)
    }

    /// Why `w`'s whole stream is held, if it is.
    pub fn writer_kind(&self, w: &str) -> Option<HoldKind> {
        self.writers.get(w).copied()
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
            "SELECT writer, table_name, kind, reason FROM oltp_change_holds \
              WHERE org_id = $1 AND environment = $2",
            [org_id.into(), environment.into()],
        ))
        .await?;
    let mut holds = Holds::default();
    for row in &rows {
        let writer: String = row.try_get("", "writer")?;
        let table: String = row.try_get("", "table_name")?;
        if table.is_empty() {
            let kind: String = row.try_get("", "kind")?;
            let reason: String = row.try_get("", "reason")?;
            holds
                .writers
                .insert(writer, HoldKind::of_row(&table, &kind, &reason));
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
    pub kind: HoldKind,
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
        "INSERT INTO oltp_change_holds (org_id, environment, writer, table_name, reason, kind) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (org_id, environment, writer, table_name) DO NOTHING",
        [
            key.org_id.into(),
            key.environment.into(),
            key.writer.into(),
            key.table.unwrap_or("").into(),
            reason.into(),
            key.kind.as_str().into(),
        ],
    ))
    .await
    .map(|r| r.rows_affected() == 1)
}

/// Which holds to remove: of an (org, environment), optionally one `writer`
/// and some `tables` of it, and only of the `kinds` named.
#[derive(Debug, Clone, Copy)]
pub struct Release<'a> {
    pub org_id: Uuid,
    pub environment: &'a str,
    pub writer: Option<&'a str>,
    pub tables: Option<&'a [String]>,
    pub kinds: &'a [HoldKind],
}

/// Remove the holds `r` names, each by its kind as [`HoldKind::of_row`]
/// reads it. Returns the (writer, table_name) of each one removed; `''` is a
/// writer's whole stream.
pub async fn release(
    db: &impl ConnectionTrait,
    r: Release<'_>,
) -> Result<Vec<(String, String)>, DbErr> {
    let kinds: Vec<String> = r.kinds.iter().map(|k| k.as_str().to_owned()).collect();
    let tables: Option<Vec<String>> = r.tables.map(<[String]>::to_vec);
    let sql = format!(
        "DELETE FROM oltp_change_holds \
          WHERE org_id = $1 AND environment = $2 AND ({KIND_SQL}) = ANY($3) \
            AND ($4::text IS NULL OR writer = $4) \
            AND ($5::text[] IS NULL OR table_name = ANY($5)) \
      RETURNING writer, table_name"
    );
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [
                r.org_id.into(),
                r.environment.into(),
                kinds.into(),
                r.writer.map(str::to_owned).into(),
                tables.into(),
            ],
        ))
        .await?;
    rows.iter()
        .map(|row| Ok((row.try_get("", "writer")?, row.try_get("", "table_name")?)))
        .collect()
}

#[cfg(test)]
mod tests;
