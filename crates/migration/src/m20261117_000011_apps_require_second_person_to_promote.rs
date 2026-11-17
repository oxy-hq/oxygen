//! Adds `apps.require_second_person_to_promote`: the app's four-eyes rule.
//! When it is set, production refuses a **promote** by the identity that
//! published the build — the same user, or the same CI workflow or token
//! (`custom_apps_promote::second_person`). A rollback to a build production
//! served before is exempt; one to a build it never served is a promote.
//!
//! Additive: `NOT NULL DEFAULT false`, so every existing app keeps today's
//! behaviour, and a binary from before this migration, which names its
//! columns, keeps reading and inserting. Guarded with `has_column`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum Apps {
    Table,
    RequireSecondPersonToPromote,
}

const TABLE: &str = "apps";
const COLUMN: &str = "require_second_person_to_promote";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_column(TABLE, COLUMN).await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Apps::Table)
                        .add_column(
                            ColumnDef::new(Apps::RequireSecondPersonToPromote)
                                .boolean()
                                .not_null()
                                .default(false),
                        )
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
                        .drop_column(Apps::RequireSecondPersonToPromote)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}
