//! The TTL sweep for preview schemas: a boot-time loop, shaped like
//! `compile_maintenance`, that finds keys whose schemas expired and queues
//! their drop. It only writes rows and enqueues — the drop itself is a
//! `preview_schema_drop` task (`previews::drop`) on the worker fleet — and it
//! is idempotent across replicas: a key's rows are claimed with a guarded
//! `UPDATE`, so two sweeps never queue the same schema twice.
//!
//! The same loop queues the compare of every finished transform build
//! ([`enqueue_compares`], `previews::compare`), as the preview-run sweep on the
//! driver's tick does; either finds each build once.
//!
//! Each pass:
//! 1. **Release** claims whose drop will not finish: its run ended failed,
//!    cancelled or timed out, or is gone; its queue task was dead-lettered; or
//!    the claim is older than [`STALE_CLAIM`]. A released schema is retried
//!    until the drop has *run* [`MAX_DROP_ATTEMPTS`] times (the executor
//!    counts them; a claim released before any worker ran it costs nothing),
//!    then left with a warning.
//! 2. **Find** up to 50 keys with a due schema ([`find_expired_keys`]).
//! 3. **Claim** each key's due schemas under a fresh run id and, in the same
//!    transaction, seed the run and queue its task ([`claim_key`]). The claim
//!    re-checks every guard of the find.
//!
//! A schema is due when it expired, is neither dropped nor refused, is not
//! claimed, has not used up its attempts, and no preview run of its key is
//! still working. A run is working when it is not a change check (`analyze`
//! never touches a preview schema), is `queued` or `running`, and either:
//! - its `agentic_runs` row is not terminal (a crashed run's row stays
//!   `running` for ever, and must not keep the key); or
//! - it has no `agentic_runs` row yet and is `queued`: a preview run's row is
//!   only seeded when the queue starts it (`runs::advance`). Bounded by the
//!   run ceiling (`OXY_PREVIEW_RUN_MAX_MINUTES`) on its age, so a queue that
//!   never moves (runs disabled) does not keep the key for ever; a run that
//!   starts after its schemas went simply creates them again.

use std::time::Duration;

use agentic_core::delegation::TaskSpec;
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait,
};
use uuid::Uuid;

pub use super::compare::enqueue_compares;
use super::drop::{DropPayload, PREVIEW_SCHEMA_DROP_KIND};
use super::runs::TERMINAL_RUN_STATUSES;

pub const INTERVAL_ENV: &str = "OXY_PREVIEW_MAINTENANCE_INTERVAL_SECS";
const DEFAULT_INTERVAL_SECS: u64 = 300;
/// Keys per pass; the next pass continues.
const KEYS_PER_PASS: i64 = 50;
/// Drops a worker actually ran for a schema (the executor counts them) before
/// the sweep gives up on it (logged; reset `drop_attempts` to retry).
pub const MAX_DROP_ATTEMPTS: i32 = 5;
/// A claim older than this is released even if its run looks alive: a drop
/// takes seconds, so one this old is lost (a task that never got a worker).
/// Releasing is safe while it runs: the drop holds its rows locked, the
/// release skips locked rows, and a drop whose claim was released skips the
/// schema.
pub const STALE_CLAIM: Duration = Duration::from_secs(6 * 3600);

#[derive(Clone, Copy, Debug)]
pub struct MaintenanceConfig {
    pub interval: Duration,
}

impl MaintenanceConfig {
    /// `OXY_PREVIEW_MAINTENANCE_INTERVAL_SECS`, default 300, at least 30.
    pub fn from_env() -> Self {
        let secs = std::env::var(INTERVAL_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_INTERVAL_SECS)
            .max(30);
        Self {
            interval: Duration::from_secs(secs),
        }
    }
}

/// One queued drop: the run that claimed `schemas` of `preview_key`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropClaim {
    pub run_id: String,
    pub workspace_id: Uuid,
    pub preview_key: String,
    pub schemas: Vec<String>,
}

/// Spawn the detached sweep loop. Pure DB work, so it runs on any node
/// regardless of `--no-workers`; the queued drops run wherever the worker
/// fleet does.
pub fn spawn(config: MaintenanceConfig) {
    tokio::spawn(async move {
        let db = match oxy::database::client::establish_connection().await {
            Ok(db) => db,
            Err(e) => {
                tracing::warn!(
                    ?e,
                    "preview_maintenance: DB connect failed; loop not started"
                );
                return;
            }
        };
        tracing::info!(
            interval_secs = config.interval.as_secs(),
            "preview_maintenance: started"
        );
        let mut tick = tokio::time::interval(config.interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await; // skip the startup storm, as compile_maintenance does
        loop {
            tick.tick().await;
            if let Err(e) = sweep_expired(&db, Utc::now()).await {
                tracing::warn!(error = %e, "preview_maintenance: sweep failed");
            }
            if let Err(e) = enqueue_compares(&db).await {
                tracing::warn!(error = %e, "preview_maintenance: queueing compares failed");
            }
        }
    });
}

/// One pass: release lost claims, then claim and queue the drop of every key
/// (up to 50) with a schema due before `now`.
pub async fn sweep_expired(
    db: &DatabaseConnection,
    now: DateTime<Utc>,
) -> Result<Vec<DropClaim>, DbErr> {
    release_lost_claims(db).await?;
    let mut claims = Vec::new();
    for (workspace_id, preview_key) in find_expired_keys(db, now).await? {
        if let Some(claim) = claim_key(db, workspace_id, &preview_key, now).await? {
            claims.push(claim);
        }
    }
    Ok(claims)
}

/// Claims whose drop will not finish: the run failed, was cancelled, timed out
/// or is gone; the queue dead-lettered its task; or the claim is older than
/// `$1` seconds. Rows a running drop holds locked are skipped, not waited on.
const RELEASE_SQL: &str = "\
    UPDATE workspace_preview_schemas s SET drop_run_id = NULL, drop_claimed_at = NULL \
    WHERE (s.workspace_id, s.schema_name) IN ( \
        SELECT c.workspace_id, c.schema_name FROM workspace_preview_schemas c \
        WHERE c.dropped_at IS NULL AND c.refused_at IS NULL AND c.drop_run_id IS NOT NULL \
          AND (EXISTS (SELECT 1 FROM agentic_runs r WHERE r.id = c.drop_run_id \
                       AND r.task_status IN ('failed','cancelled','timed_out')) \
               OR NOT EXISTS (SELECT 1 FROM agentic_runs r WHERE r.id = c.drop_run_id) \
               OR EXISTS (SELECT 1 FROM agentic_task_queue q \
                          WHERE q.task_id = c.drop_run_id AND q.queue_status = 'dead') \
               OR c.drop_claimed_at < now() - ($1::bigint * interval '1 second')) \
        ORDER BY c.workspace_id, c.schema_name \
        FOR UPDATE SKIP LOCKED) \
    RETURNING s.workspace_id, s.preview_key, s.schema_name, s.drop_attempts";

/// Let go of claims whose drop will not finish ([`RELEASE_SQL`]), warning for
/// each schema that has now used up its attempts.
async fn release_lost_claims(db: &DatabaseConnection) -> Result<(), DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            RELEASE_SQL,
            [(STALE_CLAIM.as_secs() as i64).into()],
        ))
        .await?;
    for row in &rows {
        let attempts: i32 = row.try_get("", "drop_attempts")?;
        if attempts < MAX_DROP_ATTEMPTS {
            continue;
        }
        let workspace_id: Uuid = row.try_get("", "workspace_id")?;
        let preview_key: String = row.try_get("", "preview_key")?;
        let schema: String = row.try_get("", "schema_name")?;
        tracing::warn!(%workspace_id, %preview_key, %schema, attempts,
            "preview_maintenance: giving up on dropping a preview schema after repeated \
             failures; reset drop_attempts to retry");
    }
    if !rows.is_empty() {
        tracing::info!(
            released = rows.len(),
            "preview_maintenance: released preview schemas whose drop did not finish"
        );
    }
    Ok(())
}

/// Every guard that makes a row (`s`) due, with the clock as `now_param`.
/// Shared by the find and the claim, so a row that stopped being due between
/// the two (a run started on its key, a write re-armed it) is not claimed.
///
/// Expiry is judged against `now_param` (the sweep's clock); a queued run's
/// age against the database's own `now()`, since it bounds real waiting.
fn due(now_param: &str) -> String {
    let ceiling = super::runs::max_minutes();
    format!(
        "s.dropped_at IS NULL AND s.refused_at IS NULL AND s.drop_run_id IS NULL \
         AND s.drop_attempts < {MAX_DROP_ATTEMPTS} AND s.expires_at < {now_param} \
         AND NOT EXISTS (SELECT 1 FROM workspace_preview_runs p \
                         LEFT JOIN agentic_runs r ON r.id = p.run_id \
                         WHERE p.workspace_id = s.workspace_id \
                           AND p.preview_key = s.preview_key \
                           AND p.state <> 'finished' AND p.kind <> 'analyze' \
                           AND CASE WHEN r.id IS NULL \
                                    THEN p.state = 'queued' \
                                         AND p.created_at > now() - make_interval(mins => {ceiling}) \
                                    ELSE r.task_status IS NULL \
                                         OR r.task_status NOT IN {TERMINAL_RUN_STATUSES} \
                               END)"
    )
}

/// Keys with a schema due before `now`, up to 50.
pub async fn find_expired_keys(
    db: &DatabaseConnection,
    now: DateTime<Utc>,
) -> Result<Vec<(Uuid, String)>, DbErr> {
    let sql = format!(
        "SELECT s.workspace_id, s.preview_key FROM workspace_preview_schemas s \
         WHERE {} GROUP BY 1, 2 LIMIT $2",
        due("$1")
    );
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [now.into(), KEYS_PER_PASS.into()],
        ))
        .await?;
    rows.iter()
        .map(|r| {
            Ok((
                r.try_get("", "workspace_id")?,
                r.try_get("", "preview_key")?,
            ))
        })
        .collect()
}

/// Claim the key's due schemas for a new drop run (locking them in
/// `schema_name` order, skipping any a writer holds), seed the run and queue
/// its task, all or nothing. `None` when nothing of the key is due any more.
pub async fn claim_key(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    preview_key: &str,
    now: DateTime<Utc>,
) -> Result<Option<DropClaim>, DbErr> {
    let run_id = Uuid::new_v4().to_string();
    let txn = db.begin().await?;
    let sql = format!(
        "UPDATE workspace_preview_schemas t \
         SET drop_run_id = $3, drop_claimed_at = now() \
         WHERE (t.workspace_id, t.schema_name) IN ( \
             SELECT s.workspace_id, s.schema_name FROM workspace_preview_schemas s \
             WHERE s.workspace_id = $1 AND s.preview_key = $2 AND {} \
             ORDER BY s.schema_name FOR UPDATE SKIP LOCKED) \
         RETURNING t.schema_name",
        due("$4")
    );
    let rows = txn
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [
                workspace_id.into(),
                preview_key.into(),
                run_id.clone().into(),
                now.into(),
            ],
        ))
        .await?;
    let mut schemas = rows
        .iter()
        .map(|r| r.try_get::<String>("", "schema_name"))
        .collect::<Result<Vec<_>, _>>()?;
    if schemas.is_empty() {
        txn.rollback().await?;
        return Ok(None);
    }
    schemas.sort();
    let payload = DropPayload {
        workspace_id,
        preview_key: preview_key.to_string(),
        schemas: schemas.clone(),
    };
    seed_and_enqueue(&txn, &run_id, &payload).await?;
    txn.commit().await?;
    tracing::info!(%workspace_id, %preview_key, %run_id, schemas = schemas.len(),
        "preview_maintenance: queued the drop of expired preview schemas");
    Ok(Some(DropClaim {
        run_id,
        workspace_id,
        preview_key: preview_key.to_string(),
        schemas,
    }))
}

/// The drop's `agentic_runs` row and its `TaskSpec::Custom`. No retry policy:
/// a failed drop is released and re-queued by the next sweep, under a new run.
async fn seed_and_enqueue<C: ConnectionTrait>(
    db: &C,
    run_id: &str,
    payload: &DropPayload,
) -> Result<(), DbErr> {
    let payload_json =
        serde_json::to_value(payload).map_err(|e| DbErr::Custom(format!("drop payload: {e}")))?;
    agentic_runtime::crud::insert_run(
        db,
        run_id,
        &format!("Drop expired preview schemas: {}", payload.preview_key),
        None,
        PREVIEW_SCHEMA_DROP_KIND,
        Some(payload_json.clone()),
        payload.workspace_id,
    )
    .await?;
    agentic_runtime::crud::enqueue_task(
        db,
        run_id,
        run_id,
        None,
        &TaskSpec::Custom {
            kind: PREVIEW_SCHEMA_DROP_KIND.to_string(),
            payload: payload_json,
        },
        None,
        TaskScope::Global,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn the_interval_defaults_to_five_minutes_and_is_at_least_thirty_seconds() {
        // SAFETY: serialised with every other env-mutating test in this binary.
        unsafe { std::env::remove_var(INTERVAL_ENV) };
        assert_eq!(
            MaintenanceConfig::from_env().interval,
            Duration::from_secs(300)
        );
        unsafe { std::env::set_var(INTERVAL_ENV, "1") };
        assert_eq!(
            MaintenanceConfig::from_env().interval,
            Duration::from_secs(30)
        );
        unsafe { std::env::remove_var(INTERVAL_ENV) };
    }
}
