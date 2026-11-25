//! `oxy dev seed --retire-extra-workspaces`: give back the workspaces an
//! earlier seed made that an org may no longer have.
//!
//! Its own file because [`super::seed_partners`] is already past the file size
//! limit, and because this is a one-way door the seed opens for the migration
//! rather than part of seeding.

use oxy::database::client::establish_connection;
use oxy_shared::errors::OxyError;
use oxy_shared::theme::StyledText;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, Statement};

use entity::workspaces;

use super::seed_partners::{find_org_id, refuse_if_not_local, workspace_seed_id};

/// Workspaces an earlier seed made that an org may no longer have, as
/// `(org slug, workspace name)`.
///
/// `ORGS` gave `northwind` and `globex` two workspaces each until
/// `uq_workspaces_org_id` made one per org a rule of the table. New databases
/// get one; a database seeded before that change still holds the extra row,
/// and the migration that adds the index refuses to run while any org has two
/// — so a dev box that ran `oxy dev seed` earlier cannot boot until the row is
/// gone. [`retire_extra_workspaces`] is what removes it.
///
/// Shrink-only, like a backlog: a pair leaves this list when no database that
/// could still hold it is in use.
const RETIRED_WORKSPACES: &[(&str, &str)] = &[
    ("northwind", "Northwind Sandbox"),
    ("globex", "Globex Sandbox"),
];

/// Delete the workspaces in [`RETIRED_WORKSPACES`]
/// (`oxy dev seed --retire-extra-workspaces`), so `oxy start` can run the
/// one-per-org migration on a database an earlier seed left with two.
///
/// **Only rows this seed made.** Each is deleted by the id
/// [`workspace_seed_id`] derives, so a workspace a developer created by hand,
/// or one with the same name in another org, is never touched — which is why
/// this is a seed command and not something the migration does for itself. The
/// row cascades the way the admin delete does (revisions, members, grants,
/// schedules). Local databases only, like the rest of the seed.
pub async fn retire_extra_workspaces() -> Result<usize, OxyError> {
    refuse_if_not_local()?;
    let conn = establish_connection().await?;
    if !tables_exist(&conn).await? {
        println!(
            "{} no workspaces table yet, so nothing to retire",
            "·".info()
        );
        return Ok(0);
    }
    let mut removed = 0usize;
    for (slug, name) in RETIRED_WORKSPACES {
        let Some(org_id) = find_org_id(&conn, slug).await? else {
            continue;
        };
        let id = workspace_seed_id(org_id, name);
        let gone = workspaces::Entity::delete_by_id(id)
            .exec(&conn)
            .await
            .map_err(|e| OxyError::DBError(format!("delete workspace {slug}/{name}: {e}")))?
            .rows_affected;
        if gone > 0 {
            removed += 1;
            println!("  {} removed {slug} / {name}", "✓".success());
        }
    }
    removed += detach_the_local_row(&conn).await?;
    println!(
        "{} retired {removed} workspace(s) an earlier seed made; an org has one",
        "🧹".info()
    );
    Ok(removed)
}

/// Take the nil-UUID `Local` workspace out of the Local org, if a legacy
/// `--local` run left it there.
///
/// `airhouse::local_seed` used to give that row `org_id = LOCAL_ORG_ID`, so a
/// database that ran both `oxy dev seed` (which makes the Local org's `Demo`
/// workspace) and a `--local` boot holds two in one org. It is **detached, not
/// deleted**: `airhouse_tenants` and `airhouse_users` have foreign keys to it,
/// and an org-less workspace is what it already is to every cloud surface —
/// the workspace list filters the nil id out, and `workspace_org` calls such a
/// row `Orphaned`. The partial index does not judge a NULL `org_id`, so this is
/// enough, and `local_seed` now writes it that way to begin with.
async fn detach_the_local_row(conn: &DatabaseConnection) -> Result<usize, OxyError> {
    let done = conn
        .execute_unprepared(
            "UPDATE workspaces SET org_id = NULL, updated_at = now() \
             WHERE id = '00000000-0000-0000-0000-000000000000' AND org_id IS NOT NULL",
        )
        .await
        .map_err(|e| OxyError::DBError(format!("detach the local workspace: {e}")))?;
    if done.rows_affected() > 0 {
        println!(
            "  {} detached the nil `Local` workspace from its org",
            "✓".success()
        );
    }
    Ok(done.rows_affected() as usize)
}

/// Whether the two tables this reads are there.
///
/// `just up` runs this before the backend, because the backend is what migrates
/// — so on a new machine, or after `just up --clean` dropped the volume, the
/// database is empty and `organizations` does not exist yet. An empty database
/// has nothing to retire, so that is a no-op and not a failure; without this
/// probe the query errors and takes the whole stack down with it, on exactly
/// the database this step was meant to pass over.
async fn tables_exist(conn: &DatabaseConnection) -> Result<bool, OxyError> {
    let row = conn
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT to_regclass('public.organizations') IS NOT NULL \
             AND to_regclass('public.workspaces') IS NOT NULL AS present",
        ))
        .await
        .map_err(|e| OxyError::DBError(format!("look for the seed's tables: {e}")))?;
    Ok(row
        .map(|r| r.try_get::<bool>("", "present"))
        .transpose()
        .map_err(|e| OxyError::DBError(format!("read the table probe: {e}")))?
        .unwrap_or(false))
}
