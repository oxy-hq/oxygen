//! An environment's Airhouse overlay, after the fact: resetting it, and
//! reporting what it holds (`internal-docs/2026-11-02-custom-app-branches-p3-
//! plan.md` task 5). The writes that fill it are the host's
//! (`custom_apps_functions::host::airhouse_overlay`); the shadow registry that
//! says which tables it holds is `airhouse::preview_sql::registry`, under the
//! key [`overlay_key`].
//!
//! **A reset empties; it does not drop.** Every table the overlay holds is
//! `DELETE`d from in the sibling and the table stays: the app's migrations
//! made it, and a dropped one would have to be migrated again. Then the
//! environment's shadow rows go, so reads fall back to production, and the
//! pin is set (`snapshot`, to `at` or now) or cleared (any other mode).
//!
//! **Only the environment's own sibling is touched.** Its name is computed
//! from the app's slug and the environment ([`AirhouseHome`]), never taken
//! from input, and only registry rows of the app's own schema are emptied.
//!
//! Teardown forgets the shadow rows too (`teardown`), beside the sibling's
//! drop; `diff` is the `airhouse` key of `env diff`.

use airhouse::preview_sql::registry::{self, state_str};
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};
use tokio_postgres::SimpleQueryMessage;
use uuid::Uuid;

use crate::custom_apps_functions::env_policy::airhouse_overlay::overlay_key;
use crate::custom_apps_migrations::{AirhouseDrop, AirhouseHome, connect_as_app};
use crate::custom_apps_planes::AirhouseMode;
use oxy_app_core::custom_app_environment::AppEnvironment;

/// What a reset did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ResetOutcome {
    /// The sibling's tables whose rows were deleted, as `schema.table` of the
    /// app's own schema, sorted.
    pub tables_emptied: Vec<String>,
    pub shadow_rows_cleared: u64,
    /// The pin after the reset: `at` or now under `snapshot`, else `None`.
    pub pinned_at: Option<DateTime<Utc>>,
}

/// One table the overlay holds, as `env diff` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldTable {
    pub schema: String,
    pub table: String,
    /// `shadow` (a whole copy) or `partial` (copied empty, over the cap).
    pub state: String,
    /// `count(*)` in the sibling; `None` when it could not be counted.
    pub rows: Option<u64>,
}

/// Reset `environment`'s overlay. Connects to Airhouse only when the overlay
/// holds a table.
pub async fn reset_airhouse_overlay(
    db: &DatabaseConnection,
    run: AirhouseDrop<'_>,
    environment: &AppEnvironment,
    snapshot: bool,
    at: Option<DateTime<Utc>>,
) -> Result<ResetOutcome, String> {
    let overlay = held_tables(db, run, environment).await?;
    let client = overlay.connect(run).await?;
    let sibling = overlay.home.as_ref().zip(client.as_ref());
    reset_with(db, run, environment, (snapshot, at), &overlay.held, sibling).await
}

/// [`reset_airhouse_overlay`] over a connection the caller hands it — for a
/// test whose Airhouse is another Postgres.
#[doc(hidden)]
pub async fn reset_airhouse_overlay_over(
    db: &DatabaseConnection,
    run: AirhouseDrop<'_>,
    environment: &AppEnvironment,
    snapshot: bool,
    at: Option<DateTime<Utc>>,
    client: &tokio_postgres::Client,
) -> Result<ResetOutcome, String> {
    let overlay = held_tables(db, run, environment).await?;
    let sibling = overlay.home.as_ref().map(|home| (home, client));
    reset_with(db, run, environment, (snapshot, at), &overlay.held, sibling).await
}

async fn reset_with(
    db: &DatabaseConnection,
    run: AirhouseDrop<'_>,
    environment: &AppEnvironment,
    (snapshot, at): (bool, Option<DateTime<Utc>>),
    held: &[(String, String, String)],
    sibling: Option<(&AirhouseHome, &tokio_postgres::Client)>,
) -> Result<ResetOutcome, String> {
    let mut emptied = Vec::new();
    for (schema, table, _) in held {
        if let Some((home, client)) = sibling {
            let sql = format!("DELETE FROM {}.{}", quote(home.schema()), quote(table));
            client
                .simple_query(&sql)
                .await
                .map_err(|e| format!("{sql}: {e}"))?;
        }
        emptied.push(format!("{schema}.{table}"));
    }
    let key = overlay_key(run.app_id, environment);
    let shadow_rows_cleared = registry::clear_shadow(db, run.workspace_id, &key)
        .await
        .map_err(|e| format!("forgetting the overlay's tables: {e}"))?;
    let pinned_at = snapshot.then(|| at.unwrap_or_else(Utc::now));
    set_pin(db, run.app_id, environment, pinned_at)
        .await
        .map_err(|e| format!("setting the Airhouse pin: {e}"))?;
    Ok(ResetOutcome {
        tables_emptied: emptied,
        shadow_rows_cleared,
        pinned_at,
    })
}

/// `env diff`'s `airhouse` key for `environment`, counting each held table in
/// the sibling. Connects only when the overlay holds a table.
pub async fn airhouse_diff(
    db: &DatabaseConnection,
    run: AirhouseDrop<'_>,
    environment: &AppEnvironment,
    mode: AirhouseMode,
) -> Result<Value, String> {
    let overlay = held_tables(db, run, environment).await?;
    let client = overlay.connect(run).await?;
    let sibling = overlay.home.as_ref().zip(client.as_ref());
    diff_with(db, run, environment, mode, &overlay.held, sibling).await
}

/// [`airhouse_diff`] over a connection the caller hands it.
#[doc(hidden)]
pub async fn airhouse_diff_over(
    db: &DatabaseConnection,
    run: AirhouseDrop<'_>,
    environment: &AppEnvironment,
    mode: AirhouseMode,
    client: &tokio_postgres::Client,
) -> Result<Value, String> {
    let overlay = held_tables(db, run, environment).await?;
    let sibling = overlay.home.as_ref().map(|home| (home, client));
    diff_with(db, run, environment, mode, &overlay.held, sibling).await
}

async fn diff_with(
    db: &DatabaseConnection,
    run: AirhouseDrop<'_>,
    environment: &AppEnvironment,
    mode: AirhouseMode,
    held: &[(String, String, String)],
    sibling: Option<(&AirhouseHome, &tokio_postgres::Client)>,
) -> Result<Value, String> {
    let mut tables = Vec::new();
    for (schema, table, state) in held {
        let rows = match sibling {
            Some((home, client)) => count_rows(client, home.schema(), table).await,
            None => None,
        };
        tables.push(HeldTable {
            schema: schema.clone(),
            table: table.clone(),
            state: state.clone(),
            rows,
        });
    }
    let pinned_at = read_pin(db, run.app_id, environment)
        .await
        .map_err(|e| format!("reading the Airhouse pin: {e}"))?;
    Ok(diff_json(mode, pinned_at, tables))
}

/// The `airhouse` key of `env diff`: the mode, the pin, and the tables the
/// overlay holds, sorted by schema and table.
pub fn diff_json(
    mode: AirhouseMode,
    pinned_at: Option<DateTime<Utc>>,
    mut tables: Vec<HeldTable>,
) -> Value {
    tables.sort_by(|a, b| (&a.schema, &a.table).cmp(&(&b.schema, &b.table)));
    json!({
        "mode": mode.as_str(),
        "pinned_at": pinned_at.map(|at| at.to_rfc3339()),
        "tables": tables.iter().map(|t| json!({
            "schema": t.schema,
            "table": t.table,
            "state": t.state,
            "rows": t.rows,
        })).collect::<Vec<_>>(),
    })
}

/// An environment's sibling, and the tables the registry says its overlay
/// holds in the app's own schema, as `(schema, table, state)` sorted.
struct Overlay {
    /// `None` for an environment with no sibling: production, a name that
    /// derives none, or an app whose slug backs no Airhouse schema. It holds
    /// nothing, so a reset only forgets and re-pins, and is still done.
    home: Option<AirhouseHome>,
    held: Vec<(String, String, String)>,
}

impl Overlay {
    /// The app's connection to the sibling, only when there is a table in it
    /// to touch.
    async fn connect(
        &self,
        run: AirhouseDrop<'_>,
    ) -> Result<Option<tokio_postgres::Client>, String> {
        match &self.home {
            Some(home) if !self.held.is_empty() => {
                connect_as_app(run.workspace_id, run.app_slug, home.schema())
                    .await
                    .map(Some)
                    .map_err(|e| e.to_string())
            }
            _ => Ok(None),
        }
    }
}

async fn held_tables(
    db: &DatabaseConnection,
    run: AirhouseDrop<'_>,
    environment: &AppEnvironment,
) -> Result<Overlay, String> {
    let home = match AirhouseHome::for_environment(run.app_slug, environment) {
        Ok(Some(home)) => home,
        Ok(None) => {
            return Ok(Overlay {
                home: None,
                held: Vec::new(),
            });
        }
        Err(e) => {
            tracing::debug!(app_slug = run.app_slug, error = %e, "no Airhouse sibling to reset");
            return Ok(Overlay {
                home: None,
                held: Vec::new(),
            });
        }
    };
    let key = overlay_key(run.app_id, environment);
    let shadow = registry::load_shadow(db, run.workspace_id, &key)
        .await
        .map_err(|e| format!("reading the overlay's tables: {e}"))?;
    let mut held: Vec<(String, String, String)> = shadow
        .0
        .iter()
        // Only the app's own schema is ever emptied: the sibling is
        // `<schema>__<label>`, so a row of any other schema is not this
        // sibling's to touch.
        .filter(|((schema, _), _)| home.schema().starts_with(&format!("{schema}__")))
        .map(|((schema, table), state)| (schema.clone(), table.clone(), state_str(*state).into()))
        .collect();
    held.sort();
    Ok(Overlay {
        home: Some(home),
        held,
    })
}

async fn count_rows(client: &tokio_postgres::Client, schema: &str, table: &str) -> Option<u64> {
    let sql = format!(
        "SELECT count(*) AS n FROM {}.{}",
        quote(schema),
        quote(table)
    );
    let messages = client.simple_query(&sql).await.ok()?;
    messages.iter().find_map(|message| match message {
        SimpleQueryMessage::Row(row) => row.get(0)?.parse().ok(),
        _ => None,
    })
}

/// `app_environments.airhouse_pinned_at`. The column is not on the entity, so
/// it is read and written in raw SQL.
async fn read_pin(
    db: &DatabaseConnection,
    app_id: Uuid,
    environment: &AppEnvironment,
) -> Result<Option<DateTime<Utc>>, sea_orm::DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT airhouse_pinned_at FROM app_environments WHERE app_id = $1 AND name = $2",
            [app_id.into(), environment.name().into()],
        ))
        .await?;
    match row {
        Some(row) => row.try_get::<Option<DateTime<Utc>>>("", "airhouse_pinned_at"),
        None => Ok(None),
    }
}

pub(super) async fn set_pin<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
    environment: &AppEnvironment,
    pinned_at: Option<DateTime<Utc>>,
) -> Result<(), sea_orm::DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE app_environments SET airhouse_pinned_at = $3 WHERE app_id = $1 AND name = $2",
        [app_id.into(), environment.name().into(), pinned_at.into()],
    ))
    .await?;
    Ok(())
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

#[cfg(test)]
#[path = "airhouse_overlay_tests.rs"]
mod tests;
