//! App events, platform side (app events design §6.9): the stored event, the
//! subscriptions that fan it out, one delivery per listener function, each
//! delivery's attempts, and the schema revisions an event is checked against.
//!
//! - `app_event_schemas`: one row per distinct schema of an app's event type;
//!   `revision` counts up, `sha256` keeps a republished identical schema from
//!   adding a revision.
//! - `app_events`: a relayed event's routing metadata. `id` is a UUIDv5 the
//!   app computes, so there is no default and a second relay of the same event
//!   conflicts instead of duplicating. The body is org data and never lands
//!   here: it stays in the org's OLTP outbox log (`oxy_events.outbox`), named
//!   by `(writer_role, id)`; `data_bytes` is its size.
//! - `app_event_subscriptions`: a listener function's subscription to an event
//!   type in one environment, reconciled from the live build's manifest.
//! - `app_event_deliveries`: one per (event, listener function).
//! - `app_event_delivery_attempts`: one per try of a delivery.
//! - `app_event_outbox_marks`: per (org, environment), when a publish last
//!   went into the org's outbox, when a relay pass last left it empty, and
//!   since when work has been waiting there, so the relay sweep finds waiting
//!   outboxes, and the ones waiting too long, without connecting to the
//!   org's store.
//!
//! `org_id`, `workspace_id` and `build_id` are loose UUIDs (domain-boundaries
//! P3). Every `app_id` and `source_app_id` references `apps` with `ON DELETE
//! CASCADE`, so deleting an app (or its org) deletes its event rows; deliveries
//! and attempts cascade from their event and delivery.
//!
//! Additive and revert-safe: new tables a binary one release back never reads.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS app_event_schemas (
    id UUID PRIMARY KEY,
    app_id UUID NOT NULL REFERENCES apps (id) ON DELETE CASCADE,
    event_name TEXT NOT NULL,
    revision INTEGER NOT NULL,
    schema JSONB NOT NULL,
    sha256 TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT app_event_schemas_revision_key UNIQUE (app_id, event_name, revision),
    CONSTRAINT app_event_schemas_sha256_key UNIQUE (app_id, event_name, sha256)
);

CREATE TABLE IF NOT EXISTS app_events (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    environment TEXT NOT NULL,
    source_app_id UUID NOT NULL REFERENCES apps (id) ON DELETE CASCADE,
    type TEXT NOT NULL,
    key TEXT NOT NULL,
    data_bytes INTEGER NOT NULL,
    writer_role TEXT NOT NULL,
    schema_revision_id UUID,
    actor TEXT,
    causation_id UUID,
    root_id UUID NOT NULL,
    depth INTEGER NOT NULL,
    invocation_id UUID,
    delay_seconds INTEGER NOT NULL DEFAULT 0,
    published_at TIMESTAMPTZ NOT NULL,
    relayed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    relay_error TEXT
);
CREATE INDEX IF NOT EXISTS app_events_org_env_type_published_idx
    ON app_events (org_id, environment, type, published_at);
CREATE INDEX IF NOT EXISTS app_events_published_at_idx ON app_events (published_at);
CREATE INDEX IF NOT EXISTS app_events_root_id_idx ON app_events (root_id);

CREATE TABLE IF NOT EXISTS app_event_subscriptions (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    environment TEXT NOT NULL,
    app_id UUID NOT NULL REFERENCES apps (id) ON DELETE CASCADE,
    function_name TEXT NOT NULL,
    event_type TEXT NOT NULL,
    build_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT app_event_subscriptions_listener_key
        UNIQUE (environment, app_id, function_name, event_type)
);
CREATE INDEX IF NOT EXISTS app_event_subscriptions_org_env_type_idx
    ON app_event_subscriptions (org_id, environment, event_type);

CREATE TABLE IF NOT EXISTS app_event_deliveries (
    id UUID PRIMARY KEY,
    event_id UUID NOT NULL REFERENCES app_events (id) ON DELETE CASCADE,
    app_id UUID NOT NULL REFERENCES apps (id) ON DELETE CASCADE,
    function_name TEXT NOT NULL,
    environment TEXT NOT NULL,
    status TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    run_id TEXT,
    invocation_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT app_event_deliveries_status_check
        CHECK (status IN ('pending', 'acked', 'dead', 'held', 'discarded', 'expired')),
    CONSTRAINT app_event_deliveries_listener_key UNIQUE (event_id, app_id, function_name)
);
CREATE INDEX IF NOT EXISTS app_event_deliveries_status_created_idx
    ON app_event_deliveries (status, created_at);

CREATE TABLE IF NOT EXISTS app_event_delivery_attempts (
    id UUID PRIMARY KEY,
    delivery_id UUID NOT NULL REFERENCES app_event_deliveries (id) ON DELETE CASCADE,
    attempt INTEGER NOT NULL,
    started_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ,
    outcome TEXT,
    error TEXT,
    invocation_id UUID,
    CONSTRAINT app_event_delivery_attempts_outcome_check
        CHECK (outcome IS NULL
            OR outcome IN ('ok', 'error', 'timeout', 'crash', 'non_retryable')),
    CONSTRAINT app_event_delivery_attempts_attempt_key UNIQUE (delivery_id, attempt)
);

CREATE TABLE IF NOT EXISTS app_event_outbox_marks (
    org_id UUID NOT NULL,
    environment TEXT NOT NULL,
    published_at TIMESTAMPTZ NOT NULL,
    drained_at TIMESTAMPTZ,
    pending_since TIMESTAMPTZ,
    PRIMARY KEY (org_id, environment)
);
"#;

// Reverse dependency order: attempts, deliveries, then the tables they hang off.
const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS app_event_outbox_marks;
DROP TABLE IF EXISTS app_event_delivery_attempts;
DROP TABLE IF EXISTS app_event_deliveries;
DROP TABLE IF EXISTS app_event_subscriptions;
DROP TABLE IF EXISTS app_events;
DROP TABLE IF EXISTS app_event_schemas;
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
