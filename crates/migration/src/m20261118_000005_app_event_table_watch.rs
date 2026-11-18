use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Table changes as events (app events design §7), platform side.
///
/// - `app_event_table_cursors`: one row per (workspace, environment, watched
///   table): the last DuckLake snapshot delivered, the newest the last sweep
///   saw, the table's columns as the last sweep read them (the contract a
///   listener relies on) and the cursor's state (`active`, `behind_horizon`,
///   `broken`). The cursor moves in the same platform transaction as the event
///   row and its deliveries, so a snapshot is delivered exactly when the
///   cursor has moved past it. Metadata only: the rows a snapshot changed stay
///   in the org's store, in the event's body.
/// - `app_events.mirrored_at`: when the event was copied into the workspace's
///   Airhouse `oxy_event_log` (null until then), so the mirror pass finds what
///   it still owes without reading the log.
/// - `app_event_subscriptions.paused_at` / `paused_reason` / `resumed_at`: the
///   per-subscription circuit breaker. A paused subscription's deliveries are
///   held; the breaker counts only deliveries published after `resumed_at`.
///
/// Additive. `app_events.source_app_id` was made nullable by the release before
/// this one (`m20261118_000004`), so a binary one release back reads the
/// null a table event carries.
const UP_SQL: &str = r#"
ALTER TABLE app_events ADD COLUMN IF NOT EXISTS mirrored_at TIMESTAMPTZ;
CREATE INDEX IF NOT EXISTS app_events_unmirrored_idx
    ON app_events (relayed_at) WHERE mirrored_at IS NULL;

ALTER TABLE app_event_subscriptions ADD COLUMN IF NOT EXISTS paused_at TIMESTAMPTZ;
ALTER TABLE app_event_subscriptions ADD COLUMN IF NOT EXISTS paused_reason TEXT;
ALTER TABLE app_event_subscriptions ADD COLUMN IF NOT EXISTS resumed_at TIMESTAMPTZ;

CREATE TABLE IF NOT EXISTS app_event_table_cursors (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    environment TEXT NOT NULL,
    table_ref TEXT NOT NULL,
    snapshot_id BIGINT,
    head_snapshot BIGINT,
    watched_schema TEXT,
    table_columns JSONB,
    table_id BIGINT,
    state TEXT NOT NULL DEFAULT 'active',
    state_detail TEXT,
    checked_at TIMESTAMPTZ,
    advanced_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT app_event_table_cursors_state_check
        CHECK (state IN ('active', 'behind_horizon', 'broken')),
    CONSTRAINT app_event_table_cursors_watch_key
        UNIQUE (workspace_id, environment, table_ref)
);
CREATE INDEX IF NOT EXISTS app_event_table_cursors_org_env_idx
    ON app_event_table_cursors (org_id, environment);
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS app_event_table_cursors;
ALTER TABLE app_event_subscriptions DROP COLUMN IF EXISTS resumed_at;
ALTER TABLE app_event_subscriptions DROP COLUMN IF EXISTS paused_reason;
ALTER TABLE app_event_subscriptions DROP COLUMN IF EXISTS paused_at;
DROP INDEX IF EXISTS app_events_unmirrored_idx;
ALTER TABLE app_events DROP COLUMN IF EXISTS mirrored_at;
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
