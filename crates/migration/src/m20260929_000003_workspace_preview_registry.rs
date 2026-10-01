//! The rest of the workspace-previews control plane (phase 2b; the runs table
//! landed in `m20260929_000002_workspace_preview_runs`):
//!
//! - `workspace_preview_schemas` — the registry of every Airhouse schema a
//!   preview may create, `preview_<key>__<live schema>`, and when it expires.
//!   A row is written **before** its schema exists, and the TTL sweeper
//!   (`server::previews::maintenance`) only ever drops a schema that has one,
//!   so a customer's own schema that happens to be called `preview_…` is never
//!   touched. `ck_preview_schema_name` ties the name to its key and live schema,
//!   so a row cannot claim a name its key does not own. `schema_created_at` is
//!   set only when the preview's own strict `CREATE SCHEMA` succeeded: a schema
//!   the preview did not create is never dropped. A row whose schema turned out
//!   to be someone else's — it already existed, or it holds relations the
//!   preview did not record — is `refused_at` (with `refused_reason`) and left
//!   alone for good. `drop_claimed_at` and `drop_attempts` let the sweep
//!   release a stuck claim and give up after repeated failures.
//! - `workspace_preview_tables` — which live tables a preview holds a copy of
//!   (its shadow map), so reads resolve to the copy.
//! - `workspace_preview_sources` — per-pipeline sandbox overrides for an Airway
//!   sample of a rotate-on-use source: variable names and realm ids, never a
//!   secret. At most one pipeline per workspace may rotate a given refresh
//!   token (`one_rotator`), because the provider voids the old one on issue.
//!
//! Control plane only: names, ids and timestamps. Preview rows stay in the
//! workspace's Airhouse. Purely additive: three new tables nothing older reads
//! or writes.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS workspace_preview_schemas (
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    schema_name TEXT NOT NULL,
    preview_key TEXT NOT NULL,
    live_schema TEXT NOT NULL,
    created_by_run_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_written_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    schema_created_at TIMESTAMPTZ,
    drop_run_id TEXT,
    drop_claimed_at TIMESTAMPTZ,
    drop_attempts INTEGER NOT NULL DEFAULT 0,
    dropped_at TIMESTAMPTZ,
    refused_at TIMESTAMPTZ,
    refused_reason TEXT,
    PRIMARY KEY (workspace_id, schema_name),
    CONSTRAINT ck_preview_schema_name CHECK (schema_name = 'preview_' || preview_key || '__' || live_schema)
);
CREATE INDEX IF NOT EXISTS idx_workspace_preview_schemas_expiry ON workspace_preview_schemas (expires_at)
    WHERE dropped_at IS NULL;

CREATE TABLE IF NOT EXISTS workspace_preview_tables (
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    preview_key TEXT NOT NULL,
    live_schema TEXT NOT NULL,
    table_name TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('shadow','partial','sample','dropped')),
    last_run_id TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, preview_key, live_schema, table_name)
);

CREATE TABLE IF NOT EXISTS workspace_preview_sources (
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    pipeline_name TEXT NOT NULL,
    environment TEXT NOT NULL CHECK (environment IN ('sandbox')),
    overrides JSONB NOT NULL,
    updated_by UUID REFERENCES users(id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, pipeline_name)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_workspace_preview_sources_one_rotator
    ON workspace_preview_sources (workspace_id, (overrides->>'refresh_token_var'))
    WHERE overrides ? 'refresh_token_var';
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
            .execute_unprepared(
                "DROP TABLE IF EXISTS workspace_preview_sources; \
                 DROP TABLE IF EXISTS workspace_preview_tables; \
                 DROP TABLE IF EXISTS workspace_preview_schemas;",
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::UP_SQL;

    fn flat() -> String {
        UP_SQL.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The TTL drop trusts a registry row's name because this CHECK makes the
    /// name a function of the key and live schema: a row for key `k` can only
    /// ever name `preview_k__<S>`, never a customer schema or another
    /// preview's. The drop executor re-checks the name's shape as well; this is
    /// the database's half.
    #[test]
    fn workspace_preview_schemas_check_ties_name_to_key() {
        let sql = flat();
        let table = sql
            .split("CREATE TABLE IF NOT EXISTS workspace_preview_schemas (")
            .nth(1)
            .and_then(|rest| rest.split(");").next())
            .expect("workspace_preview_schemas is created");
        assert!(
            table.contains(
                "CONSTRAINT ck_preview_schema_name CHECK \
                 (schema_name = 'preview_' || preview_key || '__' || live_schema)"
            ),
            "the name CHECK is missing or changed: {table}"
        );
        for column in [
            "schema_name TEXT NOT NULL",
            "preview_key TEXT NOT NULL",
            "live_schema TEXT NOT NULL",
            "expires_at TIMESTAMPTZ NOT NULL",
        ] {
            assert!(
                table.contains(column),
                "{column} must be NOT NULL: a NULL passes a CHECK"
            );
        }
    }

    /// The columns the registry's safety rests on: "did the preview create
    /// this schema", "is it someone else's", and the claim bookkeeping that
    /// lets the sweep release a stuck drop and stop after repeated failures.
    #[test]
    fn workspace_preview_schemas_tracks_creation_refusal_and_claims() {
        let sql = flat();
        for column in [
            "schema_created_at TIMESTAMPTZ,",
            "refused_at TIMESTAMPTZ,",
            "refused_reason TEXT,",
            "drop_claimed_at TIMESTAMPTZ,",
            "drop_attempts INTEGER NOT NULL DEFAULT 0,",
        ] {
            assert!(sql.contains(column), "missing {column}");
        }
    }

    /// The sweep reads expired, undropped rows through this partial index.
    #[test]
    fn workspace_preview_schemas_expiry_index_covers_only_live_rows() {
        assert!(flat().contains(
            "CREATE INDEX IF NOT EXISTS idx_workspace_preview_schemas_expiry \
             ON workspace_preview_schemas (expires_at) WHERE dropped_at IS NULL;"
        ));
    }

    /// Shadow-map states are exactly the airhouse `ShadowState` variants.
    #[test]
    fn workspace_preview_tables_state_check_names_every_shadow_state() {
        assert!(
            flat().contains("CHECK (state IN ('shadow','partial','sample','dropped'))"),
            "state CHECK drifted from airhouse::preview_sql::ShadowState"
        );
    }
}
