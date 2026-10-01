use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, Statement};

/// `app_function_invocations.environment`, and the idempotency key widened to
/// include it.
///
/// Custom-app environments (`internal-docs/2026-09-10-custom-app-environments-
/// design.md` §3.4) run the same build in production and staging. The
/// idempotency key was `(app, function, user, key)`, so a production call
/// reusing a key already spent in staging would find staging's row and replay
/// staging's stored result: the production write silently skipped. The column
/// joins the key, and every existing row is production, which is what it was.
///
/// **Rollback-safe.** The previous deploy never writes the column, so its rows
/// take `DEFAULT 'production'`, and among rows of one environment the wider
/// index rejects exactly what the old one did. That deploy's lookups, without
/// an environment filter, read the same rows as before because nothing yet
/// writes any other environment. `migration_rollback_safety` flags the new
/// unique index and carries this argument as its declaration.
///
/// **Concurrently, outside a transaction**, for the reason
/// `m20260911_000002_function_failure_fingerprint_index` gives: the table takes
/// a row per invocation and nothing prunes it, so a plain `CREATE INDEX` would
/// block every function invocation for the length of the scan. The new index
/// is built before the old one is dropped, so there is never a moment with no
/// unique index behind the concurrent-first-call race `acquire_invocation`
/// resolves by it. A failed concurrent build leaves an `INVALID` index that
/// `IF NOT EXISTS` would skip forever, so a leftover one is dropped first.
#[derive(DeriveMigrationName)]
pub struct Migration;

const NEW_INDEX: &str = "uq_app_function_invocations_idempotency_env";
const OLD_INDEX: &str = "uq_app_function_invocations_idempotency";

async fn drop_if_invalid(manager: &SchemaManager<'_>, index: &str) -> Result<(), DbErr> {
    let db = manager.get_connection();
    let invalid = db
        .query_one_raw(Statement::from_string(
            manager.get_database_backend(),
            format!(
                "SELECT 1 FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid \
                 WHERE c.relname = '{index}' AND pg_table_is_visible(c.oid) \
                   AND NOT i.indisvalid"
            ),
        ))
        .await?;
    if invalid.is_some() {
        db.execute_unprepared(&format!("DROP INDEX CONCURRENTLY IF EXISTS {index}"))
            .await?;
    }
    Ok(())
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        Some(false)
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        // A constant default: Postgres records it in the catalog, no rewrite.
        db.execute_unprepared(
            "ALTER TABLE app_function_invocations \
             ADD COLUMN IF NOT EXISTS environment TEXT NOT NULL DEFAULT 'production'",
        )
        .await?;
        drop_if_invalid(manager, NEW_INDEX).await?;
        db.execute_unprepared(&format!(
            "CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS {NEW_INDEX} \
             ON app_function_invocations \
             (app_id, function_name, user_id, environment, idempotency_key)"
        ))
        .await?;
        db.execute_unprepared(&format!("DROP INDEX CONCURRENTLY IF EXISTS {OLD_INDEX}"))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        // A key used in two environments cannot survive the narrower index. The
        // non-production row loses its key (and so its replay), never the
        // production one.
        db.execute_unprepared(
            "UPDATE app_function_invocations SET idempotency_key = NULL \
             WHERE environment <> 'production' AND idempotency_key IS NOT NULL",
        )
        .await?;
        drop_if_invalid(manager, OLD_INDEX).await?;
        db.execute_unprepared(&format!(
            "CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS {OLD_INDEX} \
             ON app_function_invocations (app_id, function_name, user_id, idempotency_key)"
        ))
        .await?;
        db.execute_unprepared(&format!("DROP INDEX CONCURRENTLY IF EXISTS {NEW_INDEX}"))
            .await?;
        db.execute_unprepared(
            "ALTER TABLE app_function_invocations DROP COLUMN IF EXISTS environment",
        )
        .await?;
        Ok(())
    }
}
