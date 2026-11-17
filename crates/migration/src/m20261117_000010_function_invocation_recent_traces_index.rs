use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, Statement};

/// An index on `app_function_invocations (app_id, function_name, environment,
/// created_at DESC, id DESC)`.
///
/// The console's app map reads each function's newest recorded runs in one
/// environment (`admin::apps::recent_traces`): one probe per function of
/// `app_id = $1 AND function_name = f.name AND environment = $3 AND created_at
/// >= $4 ORDER BY created_at DESC, id DESC LIMIT n`. Nothing prunes this
/// table, and `idx_app_function_invocations_app_fn (app_id, function_name)`
/// stops at the function, so without this each probe reads every invocation
/// that function ever had and sorts them. With it, a probe walks the
/// function's newest rows in that environment in order and stops at `n`.
///
/// **Concurrently, outside a transaction**, for the reason
/// `m20260911_000002_function_failure_fingerprint_index` gives: the table takes
/// a row per invocation, so a plain `CREATE INDEX` would block every function
/// call for the length of the build. A failed concurrent build leaves an
/// `INVALID` index that `IF NOT EXISTS` would skip forever, so a leftover one
/// is dropped first.
///
/// **Rollback-safe**: an index the previous deploy ignores.
#[derive(DeriveMigrationName)]
pub struct Migration;

const INDEX: &str = "idx_app_function_invocations_app_fn_env_created";

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
        drop_if_invalid(manager, INDEX).await?;
        manager
            .get_connection()
            .execute_unprepared(&format!(
                "CREATE INDEX CONCURRENTLY IF NOT EXISTS {INDEX} \
                 ON app_function_invocations \
                 (app_id, function_name, environment, created_at DESC, id DESC)"
            ))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(&format!("DROP INDEX CONCURRENTLY IF EXISTS {INDEX}"))
            .await?;
        Ok(())
    }
}
