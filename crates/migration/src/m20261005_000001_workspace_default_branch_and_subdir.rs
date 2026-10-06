//! Adds two columns to `workspaces`: `default_branch`, the branch its
//! repository's `origin/HEAD` names, and `repo_subdir`, where the workspace
//! root sits inside that repository (NULL or empty = the repository root).
//!
//! Both facts live only in a checkout today — `origin/HEAD` in its `.git`, the
//! subdirectory folded into `workspaces.path` — so a process with no working
//! copy cannot answer either. Compiling a commit without one
//! (`internal-docs/factory-retirement.md`, phase 1) needs both: which branch a
//! workspace ships from, and which directory of the fetched tree to compile.
//!
//! Control-plane configuration about the workspace, not org data. Nullable
//! with no default: a workspace with no git remote has neither, and a binary
//! from before this migration, which names its columns, keeps inserting.
//! Guarded with `has_column` like the neighbouring additive migrations.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum Workspaces {
    Table,
    DefaultBranch,
    RepoSubdir,
}

const TABLE: &str = "workspaces";

/// Both columns, as `(name, iden)`: each is added and dropped the same way.
fn columns() -> [(&'static str, Workspaces); 2] {
    [
        ("default_branch", Workspaces::DefaultBranch),
        ("repo_subdir", Workspaces::RepoSubdir),
    ]
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (name, column) in columns() {
            if !manager.has_column(TABLE, name).await? {
                manager
                    .alter_table(
                        Table::alter()
                            .table(Workspaces::Table)
                            .add_column(ColumnDef::new(column).text().null())
                            .to_owned(),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (name, column) in columns() {
            if manager.has_column(TABLE, name).await? {
                manager
                    .alter_table(
                        Table::alter()
                            .table(Workspaces::Table)
                            .drop_column(column)
                            .to_owned(),
                    )
                    .await?;
            }
        }
        Ok(())
    }
}
