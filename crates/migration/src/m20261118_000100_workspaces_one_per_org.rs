use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, Statement};

/// A partial unique index on `workspaces (org_id) WHERE org_id IS NOT NULL`:
/// an org has at most one workspace.
///
/// The workspace is the hidden key that binds an org to its configuration
/// repository and keys its revisions, Airhouse tenant and previews
/// (`internal-docs/2026-11-10-org-vs-workspace-grain.md` §5.1). Since #4050,
/// `register_workspace` (the insert every creating door goes through) refuses a
/// second one, but that is a check before an insert: two GitHub imports racing
/// past it, or a staff `transfer-org` into an org that already has one, could
/// still make two. This makes it a rule of the table. `org_id IS NULL` rows
/// (local mode, pre-tenancy) are not judged.
///
/// **Refuses to run while an org has two workspaces**, naming them, so a
/// deploy stops on that sentence rather than on Postgres's duplicate-key
/// message. The extras are moved or deleted first, by hand and with a person's
/// yes (grain memo §7.3); a migration never picks which workspace an org keeps.
///
/// **In a transaction, not concurrently.** The table holds one row per org (45
/// in production on 2026-11-18) and takes a write only when an org connects
/// its repository, so the `SHARE` lock a plain build holds is over in
/// milliseconds; and the check and the build then see the same rows.
///
/// **Rollback-safe as phase 2** of #4050: declared in
/// `crates/app/tests/platform/migration_rollback_safety.rs`. The previous deploy
/// writes a second workspace in an org only through staff `transfer-org`, which
/// this index then refuses, as intended.
#[derive(DeriveMigrationName)]
pub struct Migration;

const INDEX: &str = "uq_workspaces_org_id";

/// Every org with more than one workspace, as `slug (n)`.
const ORGS_WITH_SEVERAL: &str = "\
    SELECT COALESCE(o.slug, w.org_id::text) || ' (' || count(*) || ')' AS org \
    FROM workspaces w LEFT JOIN organizations o ON o.id = w.org_id \
    WHERE w.org_id IS NOT NULL \
    GROUP BY w.org_id, o.slug \
    HAVING count(*) > 1 \
    ORDER BY 1";

async fn orgs_with_several(manager: &SchemaManager<'_>) -> Result<Vec<String>, DbErr> {
    manager
        .get_connection()
        .query_all_raw(Statement::from_string(
            manager.get_database_backend(),
            ORGS_WITH_SEVERAL,
        ))
        .await?
        .into_iter()
        .map(|row| row.try_get::<String>("", "org"))
        .collect()
}

/// The refusal, worded for the operator whose deploy it stops — and for the
/// developer whose `oxy start` it stops, which is the common case: a database
/// seeded before this migration holds two workspaces in `northwind` and
/// `globex`, because `oxy dev seed` used to make them. Those are the seed's own
/// rows and it can take them back; `--clean` is named last because a box's
/// worktrees share one database, so it wipes work that is not yours.
fn refusal(orgs: &[String]) -> DbErr {
    DbErr::Migration(format!(
        "{INDEX}: an org may have one workspace, and {} have more: {}. Move or delete the \
         extra workspaces first (internal-docs/2026-11-10-org-vs-workspace-grain.md §7.3). \
         On a local database: `oxy dev seed --retire-extra-workspaces` removes the ones \
         `oxy dev seed` itself made (the `northwind` and `globex` sandboxes) and nothing \
         else; delete any others by hand, or start over with `oxy start --clean`, which \
         drops the whole database every worktree on this box shares.",
        orgs.len(),
        orgs.join(", "),
    ))
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let several = orgs_with_several(manager).await?;
        if !several.is_empty() {
            return Err(refusal(&several));
        }
        manager
            .get_connection()
            .execute_unprepared(&format!(
                "CREATE UNIQUE INDEX IF NOT EXISTS {INDEX} \
                 ON workspaces (org_id) WHERE org_id IS NOT NULL"
            ))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(&format!("DROP INDEX IF EXISTS {INDEX}"))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refusal_names_every_org_and_where_to_go() {
        let message = refusal(&["oxygen (5)".into(), "gia-foods (2)".into()]).to_string();
        assert!(
            message.contains("2 have more: oxygen (5), gia-foods (2)"),
            "{message}"
        );
        assert!(message.contains("§7.3"), "{message}");
        // The line a developer needs: the seed takes its own rows back, and
        // `--clean` is not the first thing they reach for.
        assert!(
            message.contains("oxy dev seed --retire-extra-workspaces"),
            "{message}"
        );
        assert!(
            message.find("--retire-extra-workspaces") < message.find("--clean"),
            "{message}"
        );
    }
}
