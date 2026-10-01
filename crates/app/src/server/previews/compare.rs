//! Comparing what a transform build wrote with live (phase 2 P2, S10).
//!
//! When a `transform_build` run's `agentic_runs` row is `done`,
//! [`enqueue_compares`] (the maintenance loop, and the preview-run sweep)
//! queues one `compare` run under it: a `workspace_preview_runs` row, its
//! `agentic_runs` row and a `TaskSpec::Custom { kind: "preview_compare" }`,
//! in one transaction, with the build's row locked so two sweeps queue one.
//!
//! [`PreviewCompareExecutor`] runs it on the worker fleet: for every table the
//! build wrote — `workspace_preview_tables` rows naming the build as
//! `last_run_id`, dropped ones included — [`table::compare_table`] compares
//! the preview's copy with the live table through a Reader. The outcome is
//! the run's `TaskOutcome::Done` metadata ([`CompareReport`]): table and column
//! names and counts, never a value from a row (I10), and a failure in fixed
//! words ([`CompareError`], [`ReadFailed`]). To see rows, the Previews tab runs
//! the `EXCEPT ALL` bodies live, bounded; nothing is stored.
//!
//! Two things a difference can be besides the branch's change, said in
//! [`CompareReport::caveats`]: the live table's age (it was built earlier; the
//! build read today's inputs), and tables the preview already held before
//! the build (`options.held_before`): the build may have read those copies
//! instead of live — a table it only read as much as one it wrote, so the
//! caveat is said whenever the preview held anything, and a written one is
//! also flagged `preexisting`. The preview is not reset before a build.

mod enqueue;
mod executor;
mod reads;
mod table;
#[cfg(test)]
mod tests;
mod view;

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use airhouse::preview_sql::{RewriteOptions, ShadowMap};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, Statement};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use enqueue::enqueue_compares;
pub use executor::PreviewCompareExecutor;
pub use table::{
    DEFAULT_DIFF_MAX_ROWS, DIFF_MAX_ROWS_VAR, Limits, ReadFailed, Retyped, Sides, TableCompare,
    compare_table, diff_max_rows,
};
pub use view::{CompareView, for_run};

use crate::agentic_wiring::preview_airhouse::{PreviewAirhousePorts, connector_on};

/// The `TaskSpec::Custom` kind, and the compare run's `source_type`.
pub const PREVIEW_COMPARE_KIND: &str = "preview_compare";

/// Always said: live was built earlier than the build's reads.
pub const FRESHNESS_CAVEAT: &str = "live tables were built by production at their own time, and \
    the build read its inputs as they are now: a difference can be the inputs' age, not the \
    change";

/// Said when the preview held any table before the build started — one the
/// build only read as much as one it wrote (and flagged `preexisting`).
pub const PREEXISTING_CAVEAT: &str = "the preview already held tables before this build (an \
    earlier run or build wrote them), and the build may have read those copies instead of live; \
    a compared table marked preexisting is one of them";

/// A compare run's outcome: counts only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompareReport {
    /// The `transform_build` run compared.
    pub build_run_id: String,
    pub diff_max_rows: u64,
    /// What else a difference can mean (fixed sentences).
    #[serde(default)]
    pub caveats: Vec<String>,
    pub tables: Vec<TableCompare>,
}

impl CompareReport {
    pub fn answer(&self) -> String {
        let equal = self.tables.iter().filter(|t| t.equal).count();
        format!(
            "{} table(s) compared with live: {equal} equal, {} different or not compared",
            self.tables.len(),
            self.tables.len() - equal
        )
    }
}

/// Why a compare failed, in fixed words: what is stored never carries an
/// engine's or the database's own message.
#[derive(Debug, thiserror::Error)]
pub enum CompareError {
    #[error("there is no compare preview run {0}")]
    NotFound(String),
    #[error("the compare run names no transform build")]
    NoBuild,
    #[error("the run's preview key is not one a preview could have")]
    Namespace,
    #[error("{0}")]
    Config(String),
    #[error("the compare could not read the preview's records")]
    Database(#[source] sea_orm::DbErr),
}

/// Compare every table the run's build wrote. A table that cannot be read is
/// reported skipped, with a fixed reason; a database error fails the run.
pub async fn compare(
    db: &DatabaseConnection,
    airhouse: &dyn PreviewAirhousePorts,
    run_id: &str,
) -> Result<CompareReport, CompareError> {
    let find = |id: String| entity::workspace_preview_runs::Entity::find_by_id(id).one(db);
    let row = find(run_id.to_string())
        .await
        .map_err(CompareError::Database)?
        .filter(|r| r.kind == "compare")
        .ok_or_else(|| CompareError::NotFound(run_id.to_string()))?;
    enqueue::mark_running(db, run_id).await;
    let build_id = row.parent_run_id.clone().ok_or(CompareError::NoBuild)?;
    let build = find(build_id.clone())
        .await
        .map_err(CompareError::Database)?
        .ok_or(CompareError::NoBuild)?;
    let ns = crate::server::previews::namespace::for_preview(&row)
        .map_err(|_| CompareError::Namespace)?;
    let limits = Limits {
        diff_max_rows: diff_max_rows().map_err(CompareError::Config)?,
        ignore_columns: ignore_columns(&row.options),
        catalog: airhouse.catalog(),
    };
    let reader = connector_on(
        airhouse,
        row.workspace_id,
        ns.clone(),
        Arc::new(RwLock::new(ShadowMap::default())),
        RewriteOptions {
            catalog: limits.catalog.clone(),
            read_live_only: true,
        },
    );
    let held_before = held_before(&build.options);
    let mut tables = Vec::new();
    for (live, state) in built_tables(db, &row, &build_id).await? {
        let sides = Sides {
            preview_schema: ns
                .schema_for(&live.0)
                .map_err(|_| CompareError::Namespace)?,
            partial: state == "partial",
            dropped: state == "dropped",
            preexisting: held_before.contains(&live),
            live,
        };
        tables.push(
            compare_table(reader.as_ref(), &sides, &limits)
                .await
                .unwrap_or_else(|why| TableCompare::skipped(&sides, why)),
        );
    }
    Ok(CompareReport {
        build_run_id: build_id,
        diff_max_rows: limits.diff_max_rows,
        caveats: caveats(&tables, &held_before),
        tables,
    })
}

/// Freshness always; the preexisting caveat whenever the preview held any
/// table when the build started — whether the build then wrote it or only
/// read it (a read of a held table is redirected to the preview's copy).
fn caveats(tables: &[TableCompare], held_before: &HashSet<(String, String)>) -> Vec<String> {
    let mut caveats = vec![FRESHNESS_CAVEAT.to_string()];
    if !held_before.is_empty() || tables.iter().any(|t| t.preexisting) {
        caveats.push(PREEXISTING_CAVEAT.to_string());
    }
    caveats
}

/// `(live table, state)` for every table the build wrote, dropped included.
async fn built_tables(
    db: &DatabaseConnection,
    row: &entity::workspace_preview_runs::Model,
    build: &str,
) -> Result<Vec<((String, String), String)>, CompareError> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT live_schema, table_name, state FROM workspace_preview_tables \
             WHERE workspace_id = $1 AND preview_key = $2 AND last_run_id = $3 \
               AND state IN ('shadow', 'partial', 'dropped') ORDER BY live_schema, table_name",
            [
                row.workspace_id.into(),
                row.preview_key.clone().into(),
                build.into(),
            ],
        ))
        .await
        .map_err(CompareError::Database)?;
    rows.iter()
        .map(|r| {
            let get = |c: &str| r.try_get::<String>("", c).map_err(CompareError::Database);
            Ok(((get("live_schema")?, get("table_name")?), get("state")?))
        })
        .collect()
}

/// The run's `options.ignore_columns`, lowercase.
fn ignore_columns(options: &Value) -> Vec<String> {
    options
        .get("ignore_columns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_ascii_lowercase)
        .collect()
}

/// The tables the preview held when the build started
/// (`options.held_before`, written by `runs::advance`).
fn held_before(options: &Value) -> HashSet<(String, String)> {
    options
        .get("held_before")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|pair| {
            let pair = pair.as_array()?;
            Some((
                pair.first()?.as_str()?.to_string(),
                pair.get(1)?.as_str()?.to_string(),
            ))
        })
        .collect()
}
