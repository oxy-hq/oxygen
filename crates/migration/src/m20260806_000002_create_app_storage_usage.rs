//! Storage accounting for the custom-app asset silo.
//!
//! Until now nothing measured `ctx.storage`: no row per object, no rollup, and
//! therefore no answer to "how many GB does this org hold?" short of walking S3.
//! See `internal-docs/customer-apps-functions.md` ("Quotas, usage & the admin Storage tab").
//!
//! **Two tables, not a per-object index.** Object rows were considered and
//! rejected: `list_objects_v2` already returns key/size/last-modified for the
//! browse surface, and a row minted at presign time is a *claim* (the browser
//! uploads straight to S3, so oxy never observes the completion) rather than a
//! fact. A rollup computed *from* S3 is correct by construction no matter who
//! wrote the bytes. Both tables therefore scale with **apps**, not objects.
//!
//! `app_storage_usage` is current state (quota decisions read this).
//! `app_storage_usage_samples` is the time series that averages into GB-month
//! for billing and differences into the growth rate that predicts the bill.

use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum AppStorageUsage {
    Table,
    AppId,
    OrgId,
    Bytes,
    ObjectCount,
    UntaggedBytes,
    UntaggedObjectCount,
    PrefixBreakdown,
    MeasuredAt,
    MeasureStatus,
    MeasureDetail,
}

#[derive(DeriveIden)]
enum AppStorageUsageSamples {
    Table,
    AppId,
    MeasuredAt,
    Bytes,
    ObjectCount,
}

/// Referenced for the cascade FKs only.
#[derive(DeriveIden)]
enum Apps {
    Table,
    Id,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AppStorageUsage::Table)
                    .if_not_exists()
                    // One row per app — the app IS the key, so no surrogate id.
                    .col(uuid(AppStorageUsage::AppId).not_null().primary_key())
                    // Denormalized from `apps.org_id`: every quota question is
                    // asked org-wide, and joining `apps` on each check would put
                    // a join on the upload hot path for a column that never
                    // changes for a given app.
                    .col(uuid(AppStorageUsage::OrgId).not_null())
                    .col(big_integer(AppStorageUsage::Bytes).not_null().default(0))
                    .col(
                        big_integer(AppStorageUsage::ObjectCount)
                            .not_null()
                            .default(0),
                    )
                    // Bytes carrying no `oxy-ttl` tag — i.e. growth with no
                    // ceiling. The leading indicator of "which app is going to
                    // surprise us", available before the invoice.
                    .col(
                        big_integer(AppStorageUsage::UntaggedBytes)
                            .not_null()
                            .default(0),
                    )
                    .col(
                        big_integer(AppStorageUsage::UntaggedObjectCount)
                            .not_null()
                            .default(0),
                    )
                    // `{ "uploads/": {"bytes":…, "objects":…}, … }` — the
                    // top-level-segment split the UI shows. Computed during the
                    // walk we are already paying for; recomputing it per page
                    // load would mean re-walking the whole silo.
                    .col(json_binary_null(AppStorageUsage::PrefixBreakdown))
                    .col(timestamp_with_time_zone(AppStorageUsage::MeasuredAt).not_null())
                    // `ok` | `partial` | `failed`. A truncated walk MUST NOT be
                    // recorded as a smaller number: a quota that quietly drops
                    // because a LIST failed is a quota that fails open at the
                    // worst possible moment.
                    .col(
                        string(AppStorageUsage::MeasureStatus)
                            .not_null()
                            .default("ok"),
                    )
                    .col(string_null(AppStorageUsage::MeasureDetail))
                    // Cascade is load-bearing, not hygiene. `delete_one` removes
                    // the S3 silo and the `apps` row, and the sweeper only
                    // measures apps that still exist — so an orphaned row is
                    // never re-measured to zero, and `status_for_org` keeps
                    // summing its bytes forever. Without this, an org that fills
                    // its quota and deletes the app stays hard-blocked for good.
                    // The entity's `on_delete = "Cascade"` is ORM metadata and
                    // does NOT create this constraint.
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_app_storage_usage_app_id")
                            .from(AppStorageUsage::Table, AppStorageUsage::AppId)
                            .to(Apps::Table, Apps::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Fleet ranking and every org-level quota check filter by org.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_app_storage_usage_org_id")
                    .table(AppStorageUsage::Table)
                    .col(AppStorageUsage::OrgId)
                    .to_owned(),
            )
            .await?;

        // "Which app should the sweeper measure next" is an ORDER BY on this.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_app_storage_usage_measured_at")
                    .table(AppStorageUsage::Table)
                    .col(AppStorageUsage::MeasuredAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(AppStorageUsageSamples::Table)
                    .if_not_exists()
                    .col(uuid(AppStorageUsageSamples::AppId).not_null())
                    .col(timestamp_with_time_zone(AppStorageUsageSamples::MeasuredAt).not_null())
                    .col(big_integer(AppStorageUsageSamples::Bytes).not_null())
                    .col(big_integer(AppStorageUsageSamples::ObjectCount).not_null())
                    // (app, time) is the natural key — a second sample for the
                    // same instant is the same measurement, so collide rather
                    // than accumulate duplicates that would skew the GB-month
                    // mean toward whichever hour got sampled twice.
                    .primary_key(
                        Index::create()
                            .col(AppStorageUsageSamples::AppId)
                            .col(AppStorageUsageSamples::MeasuredAt),
                    )
                    // Same reasoning as the rollup, plus: samples are the billing
                    // input, so a deleted app's history must not keep metering.
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_app_storage_usage_samples_app_id")
                            .from(AppStorageUsageSamples::Table, AppStorageUsageSamples::AppId)
                            .to(Apps::Table, Apps::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Billing reads a period slice; the retention prune deletes an age
        // slice. Both are this index.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_app_storage_usage_samples_measured_at")
                    .table(AppStorageUsageSamples::Table)
                    .col(AppStorageUsageSamples::MeasuredAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(AppStorageUsageSamples::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(AppStorageUsage::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}
