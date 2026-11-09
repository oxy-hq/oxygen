//! Drop the two remaining dead legacy tables: `tasks` and `settings`.
//!
//! These are the last two rows of domain-boundaries.md F2 ("some platform
//! entities are legacy") still standing. `checkpoints` went in
//! `m20260908_000002_drop_checkpoints` (#3123), `runs` in
//! `m20260909_000001_drop_runs` (#3160), `apalis_jobs` and the `a2a_*` tables in
//! `m20260915_000001_drop_dead_tables`.
//!
//! * `tasks` — a stranded prototype from `m20250501_215840_add_tasks` (title,
//!   question, answer, file path). No relation points at it and no code ever
//!   read or wrote it through the entity or raw SQL: on `main` the only mention
//!   outside the entity file was its `prelude` re-export.
//! * `settings` — the single-row, pre-multi-tenant GitHub PAT table from
//!   `m20250618_102934_create_github_config_table`. Repository access moved to
//!   `git_namespaces` (`m20250929_081920_create_git_namespaces_table`, swapped in
//!   by `m20260416_000002_swap_workspace_repo_for_git_namespace`) and the
//!   per-user `github_token` column was dropped in
//!   `m20260414_000001_drop_github_token_from_users`. The only thing the code
//!   still took from the entity module was the `SyncStatus` enum, a plain
//!   value type that now lives in `oxy::github::types`; no query named the table.
//!
//! ## Revert safety
//!
//! A dropped table breaks a revert only if the previous binary reads it
//! (`internal-docs/revert-safe-migrations.md`). Neither table has a reader in
//! this release or in any image the deploy train can roll back to: every commit
//! in the fifty before this one carries no `Tasks::find` / `Settings::find`, no
//! `tasks::`/`settings::` entity use beyond the prelude line, and no raw SQL
//! naming either table. So the binary one step back boots and serves against a
//! schema without them, which is what the rollback-safety walk's `DECLARED`
//! entry for this migration states.
//!
//! `down` recreates both tables empty, in their final shape, so the chain stays
//! reversible; they held nothing a rollback could use.

use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Tasks::Table).if_exists().to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Settings::Table).if_exists().to_owned())
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Faithful reverse of m20250501_215840_add_tasks.
        manager
            .create_table(
                Table::create()
                    .table(Tasks::Table)
                    .if_not_exists()
                    .col(uuid(Tasks::Id).primary_key())
                    .col(string(Tasks::Title))
                    .col(string(Tasks::Question))
                    .col(string(Tasks::Answer))
                    .col(string(Tasks::FilePath))
                    .col(
                        timestamp_with_time_zone(Tasks::CreatedAt)
                            .extra("DEFAULT CURRENT_TIMESTAMP".to_string()),
                    )
                    .to_owned(),
            )
            .await?;
        // Faithful reverse of m20250618_102934_create_github_config_table,
        // `serial` id included (see that migration's note on identity columns).
        manager
            .create_table(
                Table::create()
                    .table(Settings::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Settings::Id)
                            .custom(Alias::new("serial"))
                            .not_null()
                            .primary_key(),
                    )
                    .col(text(Settings::GithubToken))
                    .col(big_integer_null(Settings::SelectedRepoId))
                    .col(boolean(Settings::Onboarded).default(false))
                    .col(text_null(Settings::Revision))
                    .col(string_len(Settings::SyncStatus, 20))
                    .col(
                        timestamp_with_time_zone(Settings::CreatedAt)
                            .extra("DEFAULT CURRENT_TIMESTAMP".to_string()),
                    )
                    .col(
                        timestamp_with_time_zone(Settings::UpdatedAt)
                            .extra("DEFAULT CURRENT_TIMESTAMP".to_string()),
                    )
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Tasks {
    Table,
    Id,
    Title,
    Question,
    Answer,
    FilePath,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Settings {
    Table,
    Id,
    GithubToken,
    SelectedRepoId,
    Revision,
    SyncStatus,
    CreatedAt,
    UpdatedAt,
    Onboarded,
}
