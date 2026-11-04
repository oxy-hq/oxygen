//! The maintenance pass that drops OLTP environment branches their sandbox no
//! longer asks for ([`super::maintenance`]'s sixth step).
//!
//! A switch of a sandbox's OLTP plane off `branch` or `empty` queues a `Drop`
//! with the write (`custom_apps_env_branch::planes::queue_for_change`), and a
//! `Drop` that fails — the sandbox's lock held past its waits by the cut it
//! races, a provider error — is not retried by the queue. Nothing else
//! reclaimed the branch: sending `schema` again changes nothing, the fork pass
//! reads the planes, and the orphan pass takes only branches whose sandbox is
//! gone. So it kept counting against the org's branch cap and, for `empty`,
//! at the provider with no expiry, until the sandbox was torn down.
//!
//! This pass queues a `Drop` again for each branch whose sandbox is active,
//! whose planes are readable and ask for none, and which has not moved for
//! [`stale_teardown`] — long enough that the write's own `Drop` has had its
//! turn, and the cadence of a retry. The task re-reads the planes under the
//! sandbox's lock and keeps a branch they ask for again. A sandbox whose planes
//! do not parse is left alone: nothing is dropped on a choice nobody can read.

use chrono::{DateTime, Utc};
use entity::{app_environments, apps};
use oxy_app_core::custom_app_environment::AppEnvironmentKind;
use sea_orm::{DatabaseBackend, DatabaseConnection, DbErr, EntityTrait, Statement};

use super::maintenance::{SANDBOXES_PER_PASS, stale_teardown};
use crate::custom_apps_env_branch::task::{BranchOp, EnvBranchTask, enqueue};
use crate::custom_apps_env_branch::wants_branch;
use crate::custom_apps_planes::PlaneSpec;

/// One pass: how many drops were queued. Never fails the sweep: a failed read
/// or queue is logged and left for the next pass.
pub async fn sweep(db: &DatabaseConnection, now: DateTime<Utc>) -> usize {
    match queue_drops(db, now).await {
        Ok(queued) => queued,
        Err(e) => {
            tracing::warn!(error = %e,
                "sandbox_maintenance: could not read OLTP branches no sandbox asks for");
            0
        }
    }
}

async fn queue_drops(db: &DatabaseConnection, now: DateTime<Utc>) -> Result<usize, DbErr> {
    let mut queued = 0;
    for row in unwanted(db, now - stale_teardown()).await? {
        if PlaneSpec::parse(&row.planes).is_err() || wants_branch(&row).is_some() {
            continue;
        }
        let Some(app) = apps::Entity::find_by_id(row.app_id).one(db).await? else {
            continue;
        };
        let task = EnvBranchTask {
            app_id: app.id,
            app_slug: app.slug.clone(),
            org_id: app.org_id,
            workspace_id: app.project_id,
            environment: row.name.clone(),
            op: BranchOp::Drop,
        };
        match enqueue(db, &task).await {
            Ok(_) => {
                tracing::info!(app_id = %app.id, environment = %row.name,
                    "sandbox_maintenance: queued the drop of an OLTP branch its sandbox no longer asks for");
                queued += 1;
            }
            Err(e) => {
                tracing::warn!(app_id = %app.id, environment = %row.name, error = %e,
                    "sandbox_maintenance: could not queue the drop of an unwanted OLTP branch");
            }
        }
    }
    Ok(queued)
}

/// Up to [`SANDBOXES_PER_PASS`] active sandboxes holding an OLTP branch that
/// has not moved since `moved_before` while their stored `oltp` choice names
/// neither `branch` nor `empty` — a prefilter; the caller decides on the parsed
/// planes. Longest-still first.
async fn unwanted(
    db: &DatabaseConnection,
    moved_before: DateTime<Utc>,
) -> Result<Vec<app_environments::Model>, DbErr> {
    app_environments::Entity::find()
        .from_raw_sql(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT e.* FROM app_environments e \
               JOIN oltp_env_branches b ON b.app_id = e.app_id AND b.env_name = e.name \
              WHERE e.kind = $1 AND e.deleting_at IS NULL AND b.updated_at < $2 \
                AND COALESCE(e.planes->>'oltp', 'schema') NOT IN ('branch', 'empty') \
              ORDER BY b.updated_at, e.app_id, e.name LIMIT $3",
            [
                AppEnvironmentKind::Dev.as_str().into(),
                moved_before.fixed_offset().into(),
                SANDBOXES_PER_PASS.into(),
            ],
        ))
        .all(db)
        .await
}
