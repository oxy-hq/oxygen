use sea_orm_migration::prelude::*;

/// How many times each blocked rollout has tried, so the channel hears about a
/// block once.
///
/// `custom_apps_functions::preflight` fails `oxy migrate` — the chart's
/// pre-upgrade hook — when a release would stop a working custom app. A failed
/// hook is retried: the Job's `backoffLimit`, then every Argo sync retry, which
/// recreates the Job. Each attempt reruns the preflight. One row per (release,
/// set of breaks) counts them. `told` flips only once a Slack post has landed,
/// so attempts keep posting until one does, then stop.
///
/// A release version and a digest of app/function/rule names, no org data.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS app_preflight_blocks (\
                   release TEXT NOT NULL, \
                   digest TEXT NOT NULL, \
                   attempts INTEGER NOT NULL DEFAULT 1, \
                   told BOOLEAN NOT NULL DEFAULT false, \
                   first_blocked_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
                   last_blocked_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
                   PRIMARY KEY (release, digest)\
                 )",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS app_preflight_blocks")
            .await?;
        Ok(())
    }
}
