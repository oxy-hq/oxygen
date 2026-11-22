use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ── eval_test_definitions (*.test.yml) ────────────────────────────────
        // An eval suite, compiled so `POST /api/{workspace_id}/evals/runs` reads
        // it on the worker fleet (oxy-api-eval). Keyed by path: a test file is
        // addressed by where it sits, and its default target is the agent
        // beside it. Its own migration, as `simulation_definitions` explains: an
        // append to the compile-boundary migration would never reach a database
        // that already recorded it.
        manager
            .create_table(
                Table::create()
                    .table(EvalTestDefinitions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(EvalTestDefinitions::RevisionId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EvalTestDefinitions::FilePath)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EvalTestDefinitions::Definition)
                            .json_binary()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(EvalTestDefinitions::RevisionId)
                            .col(EvalTestDefinitions::FilePath),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_eval_test_definitions_revision")
                            .from(EvalTestDefinitions::Table, EvalTestDefinitions::RevisionId)
                            .to(Revisions::Table, Revisions::RevisionId)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(EvalTestDefinitions::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Revisions {
    Table,
    RevisionId,
}

#[derive(DeriveIden)]
enum EvalTestDefinitions {
    Table,
    FilePath,
    RevisionId,
    Definition,
}
