//! `server_keys` — secrets this deployment generates for itself, one row each.
//!
//! The first row is `session`: the root of the key that signs browser-session
//! JWTs (`oxy_auth::session_key`). Until now that key was a constant in the
//! source, the same on every deployment and readable by anyone with the
//! repository — so a session for any user could be signed anywhere.
//!
//! Why a table and not an environment variable: every instance of a deployment
//! must sign and verify with the same key, and Postgres is the one thing they
//! share by construction. A variable that one pod is missing, or a key file
//! each pod fabricates for itself, fails as "signed in on one request and out
//! on the next". The row is created by whichever instance asks first
//! (`INSERT … ON CONFLICT DO NOTHING`); this migration only makes the table.
//!
//! Expand-only: a new table nothing else references. A binary one release back
//! never reads it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS server_keys (
    name TEXT PRIMARY KEY,
    secret BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS server_keys;
"#;

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
