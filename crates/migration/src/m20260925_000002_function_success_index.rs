use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, Statement};

/// The index behind "has this function answered since?", built without
/// locking `app_function_invocations`.
///
/// The pager's persistent route (`custom_apps_functions::failure_alert`) asks
/// whether a function succeeded in a time range, and the deploy preflight asks
/// whether it did this week. A success there is `status = 'success'` with no
/// fingerprint. The fingerprint index is partial on the opposite predicate, and
/// the `(app_id, function_name)` index has no time, so without this the
/// interesting answer — "no, not since" — scans the function's whole history,
/// which nothing prunes.
///
/// Concurrently and outside a transaction, and a leftover `INVALID` index from
/// a failed build is dropped first: the same reasoning as
/// `m20260911_000002_function_failure_fingerprint_index`.
#[derive(DeriveMigrationName)]
pub struct Migration;

const INDEX: &str = "idx_app_function_invocations_success";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        Some(false)
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        let invalid = db
            .query_one_raw(Statement::from_string(
                manager.get_database_backend(),
                format!(
                    "SELECT 1 FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid \
                     WHERE c.relname = '{INDEX}' AND pg_table_is_visible(c.oid) \
                       AND NOT i.indisvalid"
                ),
            ))
            .await?;
        if invalid.is_some() {
            db.execute_unprepared(&format!("DROP INDEX CONCURRENTLY IF EXISTS {INDEX}"))
                .await?;
        }
        db.execute_unprepared(&format!(
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS {INDEX} \
             ON app_function_invocations (app_id, function_name, created_at) \
             WHERE status = 'success' AND failure_fingerprint IS NULL"
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
