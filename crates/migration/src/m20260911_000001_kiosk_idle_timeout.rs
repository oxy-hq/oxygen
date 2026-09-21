//! A kiosk carries how long it may sit idle before the shift session closes.
//!
//! The shift session is twelve hours because a closing shift is the long case
//! (`crates/app/src/server/api/frontline.rs`). That is the ceiling, not the
//! working rule: a tablet on a counter is left signed in as whoever last
//! touched it, so every submission for the rest of the day is attributed to
//! them. The number that fixes it is per-tablet — a drive-thru iPad nobody
//! stands at wants a shorter one than a back-office device.
//!
//! **Nullable, and the platform only carries the number.** NULL means "the
//! default" (`frontline_devices::DEFAULT_IDLE_TIMEOUT_SECONDS` — 30 minutes
//! today, 5 when this column was added), which is what lets every existing row
//! acquire the behaviour without a backfill and without the column having to be
//! rewritten when the default changes. That has since happened once, and no row
//! moved. The countdown itself lives in the custom app — nothing server-side
//! watches a clock for this.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(OrgKioskDevices::Table)
                    .add_column_if_not_exists(
                        ColumnDef::new(OrgKioskDevices::IdleTimeoutSeconds)
                            .integer()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(OrgKioskDevices::Table)
                    .drop_column(OrgKioskDevices::IdleTimeoutSeconds)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum OrgKioskDevices {
    Table,
    IdleTimeoutSeconds,
}
