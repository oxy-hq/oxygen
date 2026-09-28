//! Adds `app_builds.published_via` — who published a build when no *user* did.
//!
//! A trusted-publishing (GitHub OIDC) publish authenticates as a machine
//! principal with no `users` row, so `published_by` (a FK → `users`) must be
//! NULL for it. Writing the principal's synthetic nil id there instead violated
//! `fk_app_builds_published_by` and 500'd every machine publish. This column
//! keeps the audit trail that NULL would otherwise lose: the identity the OIDC
//! exchange verified, e.g.
//! `github-oidc:acme/app/.github/workflows/oxy-publish.yml@refs/heads/main env=production`.
//!
//! Exactly one of `published_by` / `published_via` is set on a new build.
//! Plain text, no FK: it must outlive the short-lived token and the publisher
//! registration that produced it. Guarded with `has_column` like 000004.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum AppBuilds {
    Table,
    PublishedVia,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_column("app_builds", "published_via").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AppBuilds::Table)
                        .add_column(ColumnDef::new(AppBuilds::PublishedVia).text().null())
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.has_column("app_builds", "published_via").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AppBuilds::Table)
                        .drop_column(AppBuilds::PublishedVia)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}
