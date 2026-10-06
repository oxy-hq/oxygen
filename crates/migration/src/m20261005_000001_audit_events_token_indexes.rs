use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, Statement};

/// The two partial indexes behind "what happened to this API token, and what
/// was done with it?" (`oxy_app_core::audit::token_events`), built without
/// locking `audit_events`.
///
/// - `idx_audit_events_token_id`: rows a token performed
///   (`metadata->>'token_id'`), newest first.
/// - `idx_audit_events_api_token_target`: the token's own lifecycle rows
///   (`target_type = 'api_token'`), newest first.
///
/// Partial, so a write-heavy table pays only for the rows a token question can
/// match. Index DDL is not a row change: the append-only trigger and the
/// per-org hash chain are untouched.
///
/// **Concurrently, outside a transaction**, for the reason
/// `m20260911_000002_function_failure_fingerprint_index` gives. `audit_events`
/// takes a row per audited request and is append-only, so a plain `CREATE
/// INDEX` (a `SHARE` lock for the whole scan, held to the end of the
/// migration's transaction) would stall every audited request for the length
/// of both builds. That is why these are not in
/// `m20261001_000002_api_token_usage`, whose table is new and empty: a
/// migration is either transactional or not, never both.
///
/// One statement per call. Postgres runs a multi-statement string as one
/// implicit transaction, which `CONCURRENTLY` refuses.
///
/// A failed concurrent build leaves an `INVALID` index that `IF NOT EXISTS`
/// would skip forever, so a leftover one is dropped first.
///
/// **Rollback-safe**: indexes the previous deploy ignores.
#[derive(DeriveMigrationName)]
pub struct Migration;

/// Each index's name, and what follows `ON`.
const INDEXES: &[(&str, &str)] = &[
    (
        "idx_audit_events_token_id",
        "audit_events ((metadata->>'token_id'), seq DESC) \
         WHERE (metadata->>'token_id') IS NOT NULL",
    ),
    (
        "idx_audit_events_api_token_target",
        "audit_events (target_id, seq DESC) WHERE target_type = 'api_token'",
    ),
];

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
        for (index, on) in INDEXES {
            drop_if_invalid(manager, index).await?;
            manager
                .get_connection()
                .execute_unprepared(&format!(
                    "CREATE INDEX CONCURRENTLY IF NOT EXISTS {index} ON {on}"
                ))
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (index, _) in INDEXES.iter().rev() {
            manager
                .get_connection()
                .execute_unprepared(&format!("DROP INDEX CONCURRENTLY IF EXISTS {index}"))
                .await?;
        }
        Ok(())
    }
}
