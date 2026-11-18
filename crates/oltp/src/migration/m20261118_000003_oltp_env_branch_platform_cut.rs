use sea_orm_migration::prelude::*;

/// The cut `oltp_env_branches.platform_schema_version` was recorded on.
///
/// A version recorded on a sandbox's own branch holds for that copy of
/// production only. The previous binary does not know the version column: its
/// reset stamps `last_reset_at` and leaves the version as a newer binary
/// recorded it on the copy before, so the newer binary would trust a version
/// the fresh copy may not carry, skip the platform steps, and every publish and
/// claim there would fail. Tied to its cut, the version is trusted only while
/// `platform_schema_cut_at` is the cut the row describes now
/// (`COALESCE(last_reset_at, created_at)`); on any other the branch reads as
/// version 0, and its first touch replays every step, each idempotent
/// (`platform.rs` rule 2).
///
/// NULL until the outbox next records a row current, so a version recorded
/// before this column is replayed once. Additive and nullable, so the previous
/// binary reads and writes the table as before. A new migration rather than an
/// edit of `m20261118_000002`, which a database may already have applied.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE oltp_env_branches \
                 ADD COLUMN IF NOT EXISTS platform_schema_cut_at TIMESTAMPTZ NULL",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE oltp_env_branches DROP COLUMN IF EXISTS platform_schema_cut_at",
            )
            .await?;
        Ok(())
    }
}
