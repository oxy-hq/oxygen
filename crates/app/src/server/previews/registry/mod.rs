//! The preview schema registry (`workspace_preview_schemas`) and shadow map
//! (`workspace_preview_tables`, [`shadow`]): which Airhouse schemas a preview
//! may have, until when, and which relations it created in them. Plain
//! Postgres; safe on any pod.
//!
//! **Registry first, and never adopt.** A preview schema is created only by
//! [`ensure_schema`]: it writes the schema's row, and only then sends a
//! **strict** `CREATE SCHEMA` (no `IF NOT EXISTS`), recording
//! `schema_created_at` when that succeeds. So every schema a preview created
//! has a row saying so, and the TTL drop (`previews::maintenance`,
//! `previews::drop`) drops only those. A schema of that name that was already
//! there (a customer's `preview_<key>__marketing`, made by hand) fails the
//! create: its row is **refused** and the write is refused with it, for good.
//! A customer schema called `preview_notes` never gets a row at all.
//!
//! **The clock.** A row expires [`schema_ttl`] after its preview last wrote.
//! What a preview run does, in order (phase 2b S8/S9 wire these):
//! 1. run start → [`touch_key`] (also releases a drop that was claimed but has
//!    not run, so the run keeps its schemas);
//! 2. per step, [`load_shadow`] once and keep the map; run
//!    `airhouse::preview_sql::rewrite` with it;
//! 3. for each `Prelude::EnsureSchema { live, .. }` the rewrite returns →
//!    [`ensure_schema`] with `live` (through `ddl_airhouse::AirhousePreviewDdl`).
//!    This **replaces** sending `Prelude::statement` for that prelude: a
//!    preview schema created any other way has no row saying the preview made
//!    it, and is never dropped;
//! 4. probe the live tables each `Prelude::CopyOnWrite` names and build the
//!    step's `copy_plan`s (reads only; nothing sent to the preview yet);
//! 5. **record before sending**: [`record_step`]`(before, &rewrite, &copies)`
//!    writes every shadow-map change the step will make, and returns the map
//!    after it. Every relation the step creates must be recorded, and recorded
//!    **before** its DDL is sent: the drop drops only recorded relations and
//!    leaves a schema holding anything else in place, so a crash between the
//!    DDL and a record written afterwards would leak the schema. The other
//!    order is harmless — a recorded relation that never came to exist is
//!    simply not there to drop;
//! 6. send each copy statement, then the rewritten SQL (each statement
//!    `verify`-ed with the workspace catalog). A step that fails here has
//!    recorded relations it may not have made; a later step reading one fails
//!    as it would on a missing table, which ends the run;
//! 7. run finish → [`touch_key`].
//!
//! Deleting the preview (`service::delete`: [`expire_key`], plus cancelling its
//! queued runs) makes its schemas due at the next sweep.
//!
//! **Lock order.** Anything that locks these rows in one transaction takes
//! registry rows (`workspace_preview_schemas`, in `schema_name` order) before
//! shadow-map rows (`workspace_preview_tables`). [`ensure_schema`] locks its
//! one row across `CREATE SCHEMA`, and the drop locks its claimed rows across
//! its DDL (up to 30s a statement), so a write to that key can wait on either.
//!
//! **A refusal that was the preview's own schema.** A `CREATE SCHEMA` whose
//! response is lost (timeout) but which ran on the server looks, on retry,
//! like a schema that already existed: the row is refused and the schema is
//! left, never adopted. To recover, check the schema holds nothing but what the
//! preview recorded (`workspace_preview_tables` for its key and live schema),
//! then either drop it by hand and set `dropped_at = now()`, or clear
//! `refused_at` / `refused_reason` and set `schema_created_at = now()` so the
//! TTL drop takes it (`internal-docs/airhouse-integration.md`).

mod shadow;

use std::time::Duration;

use airhouse::preview_sql::{PreviewNamespace, Refused};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, Statement,
    TransactionTrait,
};
use uuid::Uuid;

pub use shadow::{load_shadow, parse_state, record_step, shadow_changes, state_str, upsert_shadow};

use super::ddl::{PreviewDdlError, SchemaCreator, well_formed};

pub const TTL_ENV: &str = "OXY_PREVIEW_SCHEMA_TTL_HOURS";
const DEFAULT_TTL_HOURS: u64 = 72;

/// How long a preview's schemas outlive its last write:
/// `OXY_PREVIEW_SCHEMA_TTL_HOURS`, default 72, at least one hour.
pub fn schema_ttl() -> Duration {
    let hours = std::env::var(TTL_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_TTL_HOURS)
        .max(1);
    Duration::from_secs(hours * 3600)
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// Nothing may be written there: the live schema has no preview stand-in
    /// (`preview_…`, `…__…`, not `[a-z0-9_]`), or the schema is not the
    /// preview's (it already existed, or holds relations the preview did not
    /// create). The write must be refused, not retried.
    #[error(transparent)]
    Refused(#[from] Refused),
    #[error("database error: {0}")]
    Db(#[from] DbErr),
    /// The row is written; creating the schema failed. Retrying is safe.
    #[error("creating the preview schema: {0}")]
    Ddl(PreviewDdlError),
}

/// Register `ns`'s stand-in for `live_schema`, or re-arm its row: expiry
/// `ttl` from now, not dropped, no drop claimed. A row re-armed after a drop
/// starts over (new creator run, not created yet, no attempts). A refused row
/// is left as it is and returns nothing. Returns whether the preview already
/// created the schema.
const ARM_SQL: &str = "\
    INSERT INTO workspace_preview_schemas AS s \
        (workspace_id, schema_name, preview_key, live_schema, created_by_run_id, expires_at) \
    VALUES ($1, $2, $3, $4, $5, now() + ($6::bigint * interval '1 second')) \
    ON CONFLICT (workspace_id, schema_name) DO UPDATE SET \
        created_by_run_id = CASE WHEN s.dropped_at IS NULL THEN s.created_by_run_id \
                                 ELSE EXCLUDED.created_by_run_id END, \
        created_at = CASE WHEN s.dropped_at IS NULL THEN s.created_at ELSE now() END, \
        schema_created_at = CASE WHEN s.dropped_at IS NULL THEN s.schema_created_at END, \
        drop_attempts = CASE WHEN s.dropped_at IS NULL THEN s.drop_attempts ELSE 0 END, \
        last_written_at = now(), expires_at = EXCLUDED.expires_at, \
        dropped_at = NULL, drop_run_id = NULL, drop_claimed_at = NULL \
    WHERE s.refused_at IS NULL \
    RETURNING (schema_created_at IS NOT NULL) AS created";

/// The only way a preview schema comes to exist. In order:
/// 1. the name must be one the DDL port accepts — no row is written otherwise;
/// 2. the registry row is written (and committed: `db` is a connection, never
///    a transaction a caller could roll back after the schema exists);
/// 3. when the row says the preview has not created the schema yet, the row is
///    locked (`FOR UPDATE`) and re-read, a strict `CREATE SCHEMA` is sent
///    through `ddl`, and `schema_created_at` recorded — all under that lock.
///
/// The lock is what makes two concurrent first writes safe: the second waits,
/// re-reads the row, finds the schema created and sends nothing. Without it,
/// its strict create would fail against the first caller's fresh schema and
/// refuse the preview's own schema.
///
/// A schema that already exists when the preview first creates it is someone
/// else's: its row is refused and so is this write (`RegistryError::Refused`),
/// now and every time after. A failed create leaves the row uncreated, and a
/// retry tries again. Returns the schema name.
pub async fn ensure_schema(
    db: &DatabaseConnection,
    ddl: &dyn SchemaCreator,
    workspace_id: Uuid,
    ns: &PreviewNamespace,
    live_schema: &str,
    run_id: &str,
    ttl: Duration,
) -> Result<String, RegistryError> {
    let schema = ns.schema_for(live_schema)?;
    well_formed(ns, &schema)?;
    let live = live_schema.to_ascii_lowercase();
    if arm_row(db, workspace_id, ns, &schema, &live, run_id, ttl).await? {
        return Ok(schema);
    }
    create_locked(db, ddl, workspace_id, &schema).await?;
    Ok(schema)
}

/// The armed row, locked and re-read: whether the preview created the schema
/// (perhaps a concurrent caller, just now) and whether it is refused.
const LOCK_ROW_SQL: &str = "\
    SELECT (schema_created_at IS NOT NULL) AS created, (refused_at IS NOT NULL) AS refused \
    FROM workspace_preview_schemas \
    WHERE workspace_id = $1 AND schema_name = $2 AND dropped_at IS NULL \
    FOR UPDATE";

/// Refuse a row only while the preview has not created its schema: a refusal
/// can never land on a schema the preview did create.
const REFUSE_UNCREATED_SQL: &str = "\
    UPDATE workspace_preview_schemas \
    SET refused_at = now(), refused_reason = $3, drop_run_id = NULL, drop_claimed_at = NULL \
    WHERE workspace_id = $1 AND schema_name = $2 AND refused_at IS NULL \
      AND schema_created_at IS NULL";

/// Step 3 of [`ensure_schema`], under the row's lock.
async fn create_locked(
    db: &DatabaseConnection,
    ddl: &dyn SchemaCreator,
    workspace_id: Uuid,
    schema: &str,
) -> Result<(), RegistryError> {
    let key = || vec![workspace_id.into(), schema.into()];
    let txn = db.begin().await?;
    let row = txn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            LOCK_ROW_SQL,
            key(),
        ))
        .await?;
    let Some(row) = row else {
        txn.rollback().await?;
        return Err(RegistryError::Ddl(PreviewDdlError::Backend(format!(
            "{schema:?} was dropped while it was being created; retry the write"
        ))));
    };
    if row.try_get::<bool>("", "created")? {
        txn.commit().await?;
        return Ok(());
    }
    if row.try_get::<bool>("", "refused")? {
        txn.rollback().await?;
        return Err(Refused(refusal(db, workspace_id, schema).await?).into());
    }
    create_and_record(txn, ddl, workspace_id, schema).await
}

/// The strict create, and its outcome on the locked row, committed together.
async fn create_and_record(
    txn: DatabaseTransaction,
    ddl: &dyn SchemaCreator,
    workspace_id: Uuid,
    schema: &str,
) -> Result<(), RegistryError> {
    let key = || vec![workspace_id.into(), schema.into()];
    match ddl.create_schema(schema).await {
        Ok(()) => {
            let sql = "UPDATE workspace_preview_schemas SET schema_created_at = now() \
                       WHERE workspace_id = $1 AND schema_name = $2";
            exec(&txn, sql, key()).await?;
            txn.commit().await?;
            Ok(())
        }
        Err(PreviewDdlError::AlreadyExists(why)) => {
            tracing::warn!(%workspace_id, %schema, "previews: refusing a preview schema that \
                already existed; it is not the preview's to write into or drop");
            let mut values = key();
            values.push(why.clone().into());
            exec(&txn, REFUSE_UNCREATED_SQL, values).await?;
            txn.commit().await?;
            Err(Refused(format!("{why}; the preview will not write into it")).into())
        }
        Err(other) => {
            txn.rollback().await?;
            Err(match other {
                PreviewDdlError::Refused(refused) => refused.into(),
                other => RegistryError::Ddl(other),
            })
        }
    }
}

/// [`ARM_SQL`]: `true` when the preview already created the schema.
async fn arm_row(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    ns: &PreviewNamespace,
    schema: &str,
    live: &str,
    run_id: &str,
    ttl: Duration,
) -> Result<bool, RegistryError> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            ARM_SQL,
            [
                workspace_id.into(),
                schema.into(),
                ns.key().into(),
                live.into(),
                run_id.into(),
                ttl_secs(ttl).into(),
            ],
        ))
        .await?;
    match row {
        Some(row) => Ok(row.try_get::<bool>("", "created")?),
        None => Err(Refused(refusal(db, workspace_id, schema).await?).into()),
    }
}

/// Why a refused row was refused, for the error the write gets.
async fn refusal(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    schema: &str,
) -> Result<String, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT refused_reason FROM workspace_preview_schemas \
             WHERE workspace_id = $1 AND schema_name = $2",
            [workspace_id.into(), schema.into()],
        ))
        .await?;
    let reason: Option<String> = row
        .map(|r| r.try_get("", "refused_reason"))
        .transpose()?
        .flatten();
    Ok(format!(
        "{schema:?} is not the preview's to write into: {}",
        reason.as_deref().unwrap_or("refused")
    ))
}

/// Mark a schema as not the preview's to touch any more: never written into,
/// claimed or dropped again. The drop's refusal, for a schema the preview
/// created that also holds relations it did not; [`ensure_schema`]'s own
/// refusal goes through [`REFUSE_UNCREATED_SQL`] instead.
pub(crate) async fn refuse<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    schema: &str,
    reason: &str,
) -> Result<(), DbErr> {
    exec(
        db,
        "UPDATE workspace_preview_schemas \
         SET refused_at = now(), refused_reason = $3, drop_run_id = NULL, drop_claimed_at = NULL \
         WHERE workspace_id = $1 AND schema_name = $2 AND refused_at IS NULL",
        vec![workspace_id.into(), schema.into(), reason.into()],
    )
    .await
    .map(|_| ())
}

/// The preview wrote (or a run started or finished): push every live schema of
/// the key `ttl` into the future, and release a drop that was claimed but has
/// not reached the schema yet (`previews::drop` re-checks its claim under a
/// row lock, so it then skips the schema). Returns the rows touched.
pub async fn touch_key<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    preview_key: &str,
    ttl: Duration,
) -> Result<u64, DbErr> {
    // Rows locked in `schema_name` order (the registry's lock order), so a
    // touch never deadlocks against a claim or a drop of the same key.
    exec(
        db,
        "UPDATE workspace_preview_schemas t \
         SET last_written_at = now(), expires_at = now() + ($3::bigint * interval '1 second'), \
             drop_run_id = NULL, drop_claimed_at = NULL \
         WHERE (t.workspace_id, t.schema_name) IN ( \
             SELECT s.workspace_id, s.schema_name FROM workspace_preview_schemas s \
             WHERE s.workspace_id = $1 AND s.preview_key = $2 AND s.dropped_at IS NULL \
               AND s.refused_at IS NULL \
             ORDER BY s.schema_name FOR UPDATE)",
        vec![
            workspace_id.into(),
            preview_key.into(),
            ttl_secs(ttl).into(),
        ],
    )
    .await
}

/// The preview was deleted: its schemas are due now, and the next sweep
/// queues their drop. Returns the rows expired.
pub async fn expire_key<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    preview_key: &str,
) -> Result<u64, DbErr> {
    exec(
        db,
        "UPDATE workspace_preview_schemas SET expires_at = now() \
         WHERE workspace_id = $1 AND preview_key = $2 AND dropped_at IS NULL \
           AND refused_at IS NULL AND expires_at > now()",
        vec![workspace_id.into(), preview_key.into()],
    )
    .await
}

fn ttl_secs(ttl: Duration) -> i64 {
    i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX)
}

async fn exec<C: ConnectionTrait>(
    db: &C,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<u64, DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .map(|r| r.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn the_ttl_defaults_to_three_days_and_is_at_least_an_hour() {
        // SAFETY: serialised with every other env-mutating test in this binary.
        unsafe { std::env::remove_var(TTL_ENV) };
        assert_eq!(schema_ttl(), Duration::from_secs(72 * 3600));
        unsafe { std::env::set_var(TTL_ENV, "0") };
        assert_eq!(schema_ttl(), Duration::from_secs(3600));
        unsafe { std::env::set_var(TTL_ENV, "not a number") };
        assert_eq!(schema_ttl(), Duration::from_secs(72 * 3600));
        unsafe { std::env::remove_var(TTL_ENV) };
    }
}
