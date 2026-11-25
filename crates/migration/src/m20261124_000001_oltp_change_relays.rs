//! OLTP change stream relay, platform side (change stream design §5.3):
//! the control plane's marks, lease and holds.
//!
//! - `oltp_change_relays`: one row per (org, environment). `nudged_at` is when
//!   the org's store last logged a change, `pending_since` since when changes
//!   have waited, `drain_started_at` when a relay pass that left nothing
//!   waiting began. They let the sweep find waiting stores without connecting
//!   to one. `lease_holder`, `lease_epoch` and `lease_expires_at` are the
//!   single-writer lease: the epoch counts up on every acquisition.
//!   `failures`, `failed_at` and `retry_after` back a failing store off: a
//!   mark is not due before `retry_after` unless nudged since `failed_at`.
//!   `stale_alarmed_at` is when the sweep last logged the mark stale, so a
//!   backed-off mark alarms once per backoff window.
//! - `oltp_change_holds`: a table (or, with `table_name = ''`, a whole
//!   writer's stream) the relay must stop shipping, with the reason.
//!
//! Both reference `organizations` with `ON DELETE CASCADE`, so deleting an org
//! deletes its rows. Additive and revert-safe: new tables a binary one release
//! back never reads.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS oltp_change_relays (
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    environment TEXT NOT NULL,
    nudged_at TIMESTAMPTZ,
    pending_since TIMESTAMPTZ,
    drain_started_at TIMESTAMPTZ,
    lease_holder TEXT,
    lease_epoch BIGINT NOT NULL DEFAULT 0,
    lease_expires_at TIMESTAMPTZ,
    failures INT NOT NULL DEFAULT 0,
    failed_at TIMESTAMPTZ,
    retry_after TIMESTAMPTZ,
    stale_alarmed_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, environment)
);

CREATE TABLE IF NOT EXISTS oltp_change_holds (
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    environment TEXT NOT NULL,
    writer TEXT NOT NULL,
    table_name TEXT NOT NULL,
    reason TEXT NOT NULL,
    held_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, environment, writer, table_name)
);
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS oltp_change_holds;
DROP TABLE IF EXISTS oltp_change_relays;
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
