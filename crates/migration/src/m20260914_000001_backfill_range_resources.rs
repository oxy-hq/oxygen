use sea_orm_migration::prelude::*;

/// Remember which resources a chunked backfill was scoped to.
///
/// # Why the column has to exist
///
/// `POST /agentic-airway/chunked-backfill` had no way to say *which* resources
/// to replay, while the single-window `/backfill` already did. That is not a
/// missing convenience — it makes the chunked path spend report jobs on data it
/// cannot fetch.
///
/// A backfill run uses a RUN-SCOPED state store, so every chunk starts with no
/// cursor. Windowed resources want that: it is what lets the pinned window take
/// effect instead of the live incremental position. But a SNAPSHOT resource
/// reads the same empty state, concludes it has never run, and pulls its
/// ordinary daily snapshot — on EVERY chunk. Amazon serves no historical form
/// of a snapshot report, so those pulls cannot even return the period being
/// backfilled. On a 13-week seller backfill that is ~39 report jobs spent for
/// nothing, against a per-report daily quota shared with the Seller Central UI,
/// which then refuses the windowed reports that were the point.
///
/// # Why it is stored rather than passed
///
/// `drive_backfill_range` reads its window, granularity and concurrency from
/// this row, and `/resume-backfill` re-drives a range knowing only its id. A
/// scope held only in the originating request would therefore apply to the
/// first drive and silently widen on every resume — the run that is most
/// likely to be unattended.
///
/// Nullable, and null means "every resource", which is exactly what ranges
/// created before this migration did. No backfill of existing rows: they really
/// were unscoped, and writing an empty array would claim a scope nobody chose.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(BackfillRanges::Table)
                    .add_column_if_not_exists(ColumnDef::new(BackfillRanges::Resources).json())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(BackfillRanges::Table)
                    .drop_column(BackfillRanges::Resources)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum BackfillRanges {
    Table,
    Resources,
}
