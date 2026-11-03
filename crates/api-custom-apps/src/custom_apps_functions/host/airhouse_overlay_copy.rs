//! The copy that precedes an environment's first write to a table under the
//! `branch` and `snapshot` modes (plan task 4).
//!
//! Before `INSERT INTO app_w.t …` lands in the sibling, the sibling's `t`
//! must hold what production's holds, or the app would read back "its own
//! rows only" and a read-modify-write would lose production's. So the first
//! write to `t` copies it, once, under a Postgres advisory lock keyed by the
//! table: two functions racing on one environment's first write do not both
//! copy, and the loser sees the winner's shadow row and writes.
//!
//! **The shadow row is recorded after the copy, never before.** A crash
//! between the two leaves a copied table the registry does not mention, so
//! reads stay on production and the next write copies again. That is safe
//! because every copy starts over: `CREATE OR REPLACE` when the sibling has
//! no such table, and a refill that begins with `DELETE` when migrations
//! already made it (the refill keeps the sibling's columns, plan finding 4).
//!
//! The copy runs on the sibling-scoped Writer, so it reads production live,
//! not at the pin (plan finding 2).

use agentic_core::result::CellValue;
use airhouse::preview_sql::{CopyPlan, Prelude, ShadowState, cow_max_rows, registry};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};

use super::*;

/// The advisory-lock text for one table's first copy. Hashed by Postgres
/// (`hashtextextended`), so it need not fit a bigint here; it names the
/// workspace, the environment's key and the live table, and nothing else.
pub(super) fn copy_lock_key(workspace: Uuid, key: &str, live: &(String, String)) -> String {
    format!("airhouse-overlay:{workspace}:{key}:{}.{}", live.0, live.1)
}

impl ProjectFunctionHost {
    /// Make sure every table `preludes` would write has its copy in
    /// `sibling`. A prelude that creates a schema is skipped: the rewrite
    /// emits one for every first write, but the sibling exists already (the
    /// app's migrations made it, and app SQL cannot create a schema), so a
    /// missing one fails the write as it would in `write-only`.
    pub(super) async fn ensure_overlay_copies(
        &self,
        sibling: &str,
        preludes: &[Prelude],
    ) -> Result<(), String> {
        let (workspace, key) = self.overlay_registry_key();
        let connector = self.airhouse_connector(sibling).await?;
        for prelude in preludes {
            if let Prelude::CopyOnWrite { live, .. } = prelude {
                self.copy_once(workspace, &key, &*connector, prelude, live)
                    .await?;
            }
        }
        Ok(())
    }

    async fn copy_once(
        &self,
        workspace: Uuid,
        key: &str,
        connector: &dyn DatabaseConnector,
        prelude: &Prelude,
        live: &(String, String),
    ) -> Result<(), String> {
        let registry_err = |e: sea_orm::DbErr| format!("Airhouse overlay registry: {e}");
        let txn = self.db.begin().await.map_err(registry_err)?;
        txn.query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
            [copy_lock_key(workspace, key, live).into()],
        ))
        .await
        .map_err(registry_err)?;
        let held = registry::load_shadow(&txn, workspace, key)
            .await
            .map_err(registry_err)?;
        let already = held
            .state(live)
            .is_some_and(|state| state != ShadowState::Dropped);
        if !already && let Some(plans) = plan_copy(connector, prelude).await? {
            for plan in &plans {
                with_db_timeout("airhouse copy", async {
                    connector
                        .execute_statement(&plan.statement)
                        .await
                        .map_err(|e| format!("copying {}.{} failed: {e}", live.0, live.1))
                })
                .await?;
            }
            if let Some(last) = plans.last() {
                let run_id = self.identity.invocation_id.to_string();
                registry::upsert_shadow(
                    &txn,
                    workspace,
                    key,
                    &run_id,
                    &[(live.clone(), last.state)],
                )
                .await
                .map_err(registry_err)?;
            }
        }
        txn.commit().await.map_err(registry_err)
    }
}

/// The statements that bring the sibling's table up to production's, in
/// order. `None` when production has no such table: the write then meets the
/// missing table as production would. A cap that is not a number is an error,
/// never a default.
async fn plan_copy(
    connector: &dyn DatabaseConnector,
    prelude: &Prelude,
) -> Result<Option<Vec<CopyPlan>>, String> {
    let catalog = airhouse::ducklake_catalog();
    let cat = catalog.as_deref();
    let probe = |sql: Option<String>| sql.ok_or_else(|| "not a copy-on-write".to_string());
    if count(connector, &probe(prelude.live_table_probe(cat))?).await? == 0 {
        return Ok(None);
    }
    let rows = count(connector, &probe(prelude.live_rows(cat))?).await?;
    let cap = cow_max_rows()?;
    if count(connector, &probe(prelude.preview_table_probe(cat))?).await? == 0 {
        return Ok(prelude
            .copy_plan(cat, true, rows, cap)
            .map(|plan| vec![plan]));
    }
    let columns = column_names(connector, &probe(prelude.shared_columns_probe(cat))?).await?;
    Ok(prelude.refill_plans(cat, rows, cap, &columns))
}

/// The single count a probe or `count(*)` returns.
async fn count(connector: &dyn DatabaseConnector, sql: &str) -> Result<u64, String> {
    let fail = |why: String| format!("preparing an Airhouse overlay copy: {why}");
    let result = connector
        .execute_query(sql, 1)
        .await
        .map_err(|e| fail(e.to_string()))?;
    let cell = result
        .result
        .rows
        .first()
        .and_then(|row| row.0.first())
        .ok_or_else(|| fail(format!("`{sql}` returned no row")))?;
    match cell {
        CellValue::Number(n) if *n >= 0.0 => Ok(*n as u64),
        CellValue::Text(t) => t
            .trim()
            .parse()
            .map_err(|_| fail(format!("`{sql}` returned {t:?}"))),
        other => Err(fail(format!("`{sql}` returned {other:?}"))),
    }
}

/// The first column of every row, as text.
async fn column_names(connector: &dyn DatabaseConnector, sql: &str) -> Result<Vec<String>, String> {
    let result = connector
        .execute_query(sql, 10_000)
        .await
        .map_err(|e| format!("preparing an Airhouse overlay copy: {e}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_names_the_workspace_environment_and_table() {
        let ws = Uuid::nil();
        let live = ("app_w".to_string(), "orders".to_string());
        assert_eq!(
            copy_lock_key(ws, "env:a:dev-x", &live),
            format!("airhouse-overlay:{ws}:env:a:dev-x:app_w.orders")
        );
        let other = ("app_w".to_string(), "items".to_string());
        assert_ne!(
            copy_lock_key(ws, "env:a:dev-x", &live),
            copy_lock_key(ws, "env:a:dev-x", &other)
        );
    }
}
