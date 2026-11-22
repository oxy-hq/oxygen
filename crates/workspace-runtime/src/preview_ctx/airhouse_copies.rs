//! Copy-on-write for a preview Airhouse step: planning the copies a rewrite
//! asks for, and noticing a copy the shadow map records that is not there.
//!
//! The second is what a crash leaves behind. A step records its copies before
//! it sends them (`registry::record_step`); if the send never happens, the map
//! says the preview holds `S.t` while `"preview_<key>__S"."t"` does not exist,
//! and the next write in place — which copies only what the map says is not
//! held — would meet a missing table. [`missing_copies`] finds such tables so
//! the step is rewritten as if the preview held nothing for them, and the copy
//! is planned again — strictly ([`CopyPlan::strict`]): the listing runs on a
//! Reader that can lag the Writer, and a copy made on its word must fail
//! rather than replace a table the preview has been writing.

use std::collections::HashSet;

use agentic_connector::DatabaseConnector;
use agentic_core::result::CellValue;
use airhouse::preview_sql::{
    CopyPlan, PreviewNamespace, Rewrite, ShadowMap, ShadowState, cow_max_rows,
};

/// The step's copy-on-writes: each live table probed and counted through the
/// run's connector (reads, on a Reader), then planned against the cap. A copy
/// of a table in `again` (planned again after a listing missed it) is strict.
pub(super) async fn copy_plans(
    conn: &dyn DatabaseConnector,
    rewrite: &Rewrite,
    catalog: Option<&str>,
    again: &HashSet<(String, String)>,
) -> Result<Vec<CopyPlan>, String> {
    let cap = cow_max_rows()?;
    let mut plans = Vec::new();
    for prelude in &rewrite.preludes {
        let (Some(probe), Some(rows)) = (
            prelude.live_table_probe(catalog),
            prelude.live_rows(catalog),
        ) else {
            continue;
        };
        let exists = count(conn, &probe).await? > 0;
        let live_rows = if exists { count(conn, &rows).await? } else { 0 };
        let plan = prelude.copy_plan(catalog, exists, live_rows, cap);
        plans.extend(plan.map(|p| {
            if again.contains(&p.live) {
                p.strict()
            } else {
                p
            }
        }));
    }
    Ok(plans)
}

/// Tables the step writes that `before` records as held (not dropped) but
/// whose preview relation does not exist: one listing per preview schema.
pub(super) async fn missing_copies(
    conn: &dyn DatabaseConnector,
    ns: &PreviewNamespace,
    before: &ShadowMap,
    rewrite: &Rewrite,
) -> Result<Vec<(String, String)>, String> {
    let held: Vec<&(String, String)> = rewrite
        .writes
        .iter()
        .filter(|live| {
            before
                .state(live)
                .is_some_and(|s| s != ShadowState::Dropped)
        })
        .collect();
    let mut schemas: Vec<&str> = held.iter().map(|(s, _)| s.as_str()).collect();
    schemas.sort();
    schemas.dedup();
    let mut missing = Vec::new();
    for live_schema in schemas {
        let schema = ns.schema_for(live_schema).map_err(|e| e.to_string())?;
        let present = relations_in(conn, &schema).await?;
        missing.extend(
            held.iter()
                .filter(|(s, t)| s == live_schema && !present.contains(t))
                .map(|live| (*live).clone()),
        );
    }
    Ok(missing)
}

/// The relations in `schema`, lowercase. Filtered by schema alone: a preview
/// schema's name is unique to the preview, whatever catalog it is in.
async fn relations_in(conn: &dyn DatabaseConnector, schema: &str) -> Result<Vec<String>, String> {
    let sql = format!(
        "SELECT lower(table_name) FROM information_schema.tables WHERE lower(table_schema) = '{}'",
        schema.replace('\'', "''")
    );
    let result = conn
        .execute_query(&sql, 100_000)
        .await
        .map_err(|e| format!("listing the preview's tables in {schema}: {e}"))?;
    Ok(result
        .result
        .rows
        .iter()
        .filter_map(|row| match row.0.first() {
            Some(CellValue::Text(name)) => Some(name.clone()),
            _ => None,
        })
        .collect())
}

/// The single count a probe or `count(*)` returns.
async fn count(conn: &dyn DatabaseConnector, sql: &str) -> Result<u64, String> {
    let result = conn
        .execute_query(sql, 1)
        .await
        .map_err(|e| format!("preparing a copy-on-write: {e}"))?;
    let cell = result
        .result
        .rows
        .first()
        .and_then(|row| row.0.first())
        .ok_or_else(|| format!("preparing a copy-on-write: `{sql}` returned no row"))?;
    match cell {
        CellValue::Number(n) if *n >= 0.0 => Ok(*n as u64),
        CellValue::Text(t) => t
            .trim()
            .parse()
            .map_err(|_| format!("preparing a copy-on-write: `{sql}` returned {t:?}")),
        other => Err(format!(
            "preparing a copy-on-write: `{sql}` returned {other:?}"
        )),
    }
}
