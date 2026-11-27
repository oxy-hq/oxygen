//! Adds `apps.promotion_checks`: which promote checks hold the app's promote
//! to production, and what happens after one (`{required, seen_required,
//! production_smoke, auto_rollback}`, `custom_apps_build_checks::settings`).
//! The platform's own configuration of an app, beside
//! `require_second_person_to_promote`; read only where the org's
//! `promote_checks_gate` flag is on.
//!
//! Additive: nullable JSONB with no default, NULL meaning the defaults, so
//! every existing app keeps today's behaviour and a binary from before this
//! migration, which names its columns, keeps reading and inserting. Guarded
//! with `has_column`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum Apps {
    Table,
    PromotionChecks,
}

const TABLE: &str = "apps";
const COLUMN: &str = "promotion_checks";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_column(TABLE, COLUMN).await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Apps::Table)
                        .add_column(ColumnDef::new(Apps::PromotionChecks).json_binary().null())
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.has_column(TABLE, COLUMN).await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Apps::Table)
                        .drop_column(Apps::PromotionChecks)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}
