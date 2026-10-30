use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, Statement};

/// A partial index on `app_function_invocations (environment, created_at DESC,
/// id DESC)` over the invocations that count as failures, for the staff feed
/// `GET /api/customer-apps/function-failures`.
///
/// The feed asks "every failed production invocation, newest first, keyset on
/// `(created_at, id)`" across every app. None of the existing indexes answer
/// that: each leads with `app_id`, so an unbounded caller made Postgres read
/// every production row of every app in the window — successes, which are
/// nearly all of them, included — and sort. This index holds only failures and
/// is in the feed's order, so a page is `LIMIT n` entries of it. The predicate
/// is [`FAILURE_PREDICATE`], which the feed states verbatim, so the planner can
/// prove the index applies.
///
/// **Concurrently, outside a transaction**, for the reason
/// `m20261001_000002_function_invocation_environment_index` gives: the table
/// takes a row per invocation. A failed concurrent build leaves an `INVALID`
/// index that `IF NOT EXISTS` would skip forever, so a leftover one is dropped
/// first.
///
/// **Rollback-safe**: an index the previous deploy ignores.
#[derive(DeriveMigrationName)]
pub struct Migration;

const INDEX: &str = "idx_app_function_invocations_failure_feed";

/// The rows the index holds — the pager's failure rule
/// (`counted_as_failure`), as the feed's query spells it.
pub const FAILURE_PREDICATE: &str =
    "failure_fingerprint IS NOT NULL OR status IN ('error', 'timeout')";

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
                 ON app_function_invocations (environment, created_at DESC, id DESC) \
                 WHERE {FAILURE_PREDICATE}"
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
