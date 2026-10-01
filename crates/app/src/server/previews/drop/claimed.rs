//! The drop's Postgres side: lock what this run claimed, read what the preview
//! recorded, and write back what the drop did — one transaction around the
//! Airhouse DDL.

use std::collections::{HashMap, HashSet};

use airhouse::preview_sql::PreviewNamespace;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait,
};

use super::pass::{ClaimedSchema, drop_listed};
use super::{DropPayload, DropReport};
use crate::server::previews::ddl::SchemaDropper;
use crate::server::previews::registry::refuse;
use crate::server::previews::runs::TERMINAL_RUN_STATUSES;

/// The rows this run still holds a claim on, locked until the transaction
/// ends, in `schema_name` order (the registry's lock order).
const LOCK_CLAIMED_SQL: &str = "\
    SELECT schema_name, live_schema, (schema_created_at IS NOT NULL) AS created \
    FROM workspace_preview_schemas \
    WHERE workspace_id = $1 AND preview_key = $2 AND schema_name = ANY($3) \
      AND dropped_at IS NULL AND refused_at IS NULL AND drop_run_id = $4 \
    ORDER BY schema_name \
    FOR UPDATE";

/// Every relation name the preview recorded, per live schema, **in any state**
/// — `dropped` included. The host records a step's changes before it sends
/// the step's SQL (`registry::record_step`), so a `dropped` entry can name a
/// relation whose `DROP` or `RENAME` never ran (the step crashed first): that
/// relation is still the preview's, and must not make the schema look like
/// someone else's.
const RECORDED_SQL: &str = "\
    SELECT live_schema, table_name FROM workspace_preview_tables \
    WHERE workspace_id = $1 AND preview_key = $2";

/// This run reached a worker: count the attempt on every row it still claims.
/// Its own statement, committed before the drop's transaction, so a drop that
/// crashes mid-way still counts; a claim released stale or dead-lettered
/// before any worker ran it never does.
const COUNT_ATTEMPT_SQL: &str = "\
    UPDATE workspace_preview_schemas SET drop_attempts = drop_attempts + 1 \
    WHERE workspace_id = $1 AND preview_key = $2 AND schema_name = ANY($3) \
      AND dropped_at IS NULL AND refused_at IS NULL AND drop_run_id = $4";

/// Drop what `run_id` claimed for `payload`'s key, then write the outcome back.
/// One transaction, so the row locks hold across the Airhouse DDL.
pub async fn drop_claimed(
    db: &DatabaseConnection,
    dropper: &dyn SchemaDropper,
    run_id: &str,
    payload: &DropPayload,
) -> Result<DropReport, DbErr> {
    let ns = PreviewNamespace::from_key(&payload.preview_key)
        .map_err(|e| DbErr::Custom(format!("drop payload: {e}")))?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        COUNT_ATTEMPT_SQL,
        [
            payload.workspace_id.into(),
            payload.preview_key.clone().into(),
            payload.schemas.clone().into(),
            run_id.into(),
        ],
    ))
    .await?;
    let txn = db.begin().await?;
    let claimed = lock_claimed(&txn, run_id, payload).await?;
    let report = drop_listed(dropper, &ns, &claimed, &payload.schemas).await;
    record(&txn, payload, &claimed, &report).await?;
    txn.commit().await?;
    Ok(report)
}

async fn lock_claimed<C: ConnectionTrait>(
    db: &C,
    run_id: &str,
    payload: &DropPayload,
) -> Result<HashMap<String, ClaimedSchema>, DbErr> {
    let (ws, key) = (payload.workspace_id, payload.preview_key.clone());
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            LOCK_CLAIMED_SQL,
            [
                ws.into(),
                key.clone().into(),
                payload.schemas.clone().into(),
                run_id.into(),
            ],
        ))
        .await?;
    let recorded_rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            RECORDED_SQL,
            [ws.into(), key.into()],
        ))
        .await?;
    let mut recorded: HashMap<String, HashSet<String>> = HashMap::new();
    for row in recorded_rows {
        let table: String = row.try_get("", "table_name")?;
        recorded
            .entry(row.try_get("", "live_schema")?)
            .or_default()
            .insert(table.to_ascii_lowercase());
    }
    let mut claimed = HashMap::new();
    for row in rows {
        let live_schema: String = row.try_get("", "live_schema")?;
        let schema = ClaimedSchema {
            created: row.try_get("", "created")?,
            recorded: recorded.remove(&live_schema).unwrap_or_default(),
            live_schema,
        };
        claimed.insert(row.try_get("", "schema_name")?, schema);
    }
    Ok(claimed)
}

/// Postgres after the DDL:
/// - dropped schemas, and ones the preview never created, are closed
///   (`dropped_at`);
/// - schemas holding relations the preview did not record, and claimed names
///   that are not well formed, are refused (never claimed again);
/// - the shadow-map rows of every schema closed or refused go (none of the
///   relations they name exist any more);
/// - once no live schema of the key is left, its Airway sample state and
///   finished leases go too (`$3` is the key's pipeline prefix,
///   `preview:<key>:`, matched with `left()`: `_` is a `LIKE` wildcard).
async fn record<C: ConnectionTrait>(
    db: &C,
    payload: &DropPayload,
    claimed: &HashMap<String, ClaimedSchema>,
    report: &DropReport,
) -> Result<(), DbErr> {
    let closed: Vec<String> = report
        .dropped
        .iter()
        .map(|(s, _)| s.clone())
        .chain(report.never_created.iter().cloned())
        .collect();
    let mut refused: Vec<(String, String)> = report
        .orphaned
        .iter()
        .map(|(s, _, others)| {
            let reason = format!(
                "it holds relations the preview did not create ({}), so it was left in place",
                others.join(", ")
            );
            (s.clone(), reason)
        })
        .collect();
    refused.extend(
        report
            .refused
            .iter()
            .filter(|(s, _)| claimed.contains_key(s))
            .cloned(),
    );
    let ws = payload.workspace_id;
    let key = &payload.preview_key;
    exec(
        db,
        "UPDATE workspace_preview_schemas SET dropped_at = now() \
         WHERE workspace_id = $1 AND preview_key = $2 AND schema_name = ANY($3)",
        vec![ws.into(), key.clone().into(), closed.clone().into()],
    )
    .await?;
    for (schema, reason) in &refused {
        refuse(db, ws, schema, reason).await?;
    }
    let gone_live: Vec<String> = closed
        .iter()
        .chain(refused.iter().map(|(s, _)| s))
        .filter_map(|s| claimed.get(s).map(|c| c.live_schema.clone()))
        .collect();
    exec(
        db,
        "DELETE FROM workspace_preview_tables \
         WHERE workspace_id = $1 AND preview_key = $2 AND live_schema = ANY($3)",
        vec![ws.into(), key.clone().into(), gone_live.into()],
    )
    .await?;
    cleanup_airway(db, payload).await
}

async fn cleanup_airway<C: ConnectionTrait>(db: &C, payload: &DropPayload) -> Result<(), DbErr> {
    let none_left = "NOT EXISTS (SELECT 1 FROM workspace_preview_schemas s \
         WHERE s.workspace_id = $1 AND s.preview_key = $2 \
           AND s.dropped_at IS NULL AND s.refused_at IS NULL)";
    let statements = [
        format!(
            "DELETE FROM airway_workspace_pipeline_state \
             WHERE workspace_id = $1 AND left(pipeline_name, length($3)) = $3 AND {none_left}"
        ),
        format!(
            "DELETE FROM airway_pipeline_leases l \
             WHERE l.workspace_id = $1 AND left(l.pipeline_name, length($3)) = $3 \
               AND (l.expires_at < now() OR EXISTS (SELECT 1 FROM agentic_runs r \
                    WHERE r.id = l.run_id AND r.task_status IN {TERMINAL_RUN_STATUSES})) \
               AND {none_left}"
        ),
    ];
    let prefix = format!("preview:{}:", payload.preview_key);
    for sql in statements {
        exec(
            db,
            &sql,
            vec![
                payload.workspace_id.into(),
                payload.preview_key.clone().into(),
                prefix.clone().into(),
            ],
        )
        .await?;
    }
    Ok(())
}

async fn exec<C: ConnectionTrait>(
    db: &C,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .map(|_| ())
}
