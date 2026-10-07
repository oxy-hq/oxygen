//! `app_builds.published_token_id` — the **sandbox agent token** that published
//! a build, to a sandbox of its own or as a draft to staging (sandbox agent
//! credential design, "Staging option", §10.5). NULL for every build a person
//! or a CI job published, and for every row that exists today.
//!
//! Two rules read it, and both need the build itself to remember — an audit
//! row is best-effort and pruned, and the draft pointer says nothing of who
//! moved it:
//!
//! - **production never falls back to it.** An app with no production build
//!   runs staging's build on the production path; not when a token published
//!   that build (`custom_apps_agent_built`);
//! - **a blind promote never ships it.** Promote, batch promote, promote
//!   latest and rollback refuse a build that carries it, until a person
//!   publishes the build under their own name. Promote latest takes an app's
//!   newest build and a rollback any retained one, so a token's sandbox build
//!   is marked as its draft is.
//!
//! Expand-only: one nullable column, no index, no constraint. A plain uuid and
//! **not** a foreign key, for the reason `app_environments.created_by_token_id`
//! is one: a constraint added to an existing table is a contraction the
//! rollback-safety guard refuses, and `ON DELETE SET NULL` would erase the
//! mark when the minter's row goes — a dangling id still says "a token
//! published this", which is all either rule asks.
//!
//! A binary one release back never reads the column and writes it as NULL. So
//! during a revert a build a token publishes to its sandbox goes unmarked, and
//! a build already marked is treated as any other — the state before this
//! column, for as long as the revert lasts. (It publishes no draft meanwhile:
//! an `app_staging` grant is a kind that binary refuses the token for.)

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = "ALTER TABLE app_builds ADD COLUMN IF NOT EXISTS published_token_id UUID;";

const DOWN_SQL: &str = "ALTER TABLE app_builds DROP COLUMN IF EXISTS published_token_id;";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DOWN_SQL)
            .await?;
        Ok(())
    }
}
