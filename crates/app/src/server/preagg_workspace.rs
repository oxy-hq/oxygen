//! Per-workspace context construction for the pre-aggregation cycle.
//!
//! The cycle runs as a `TaskSpec::Custom { kind: "preagg_cycle" }` on the
//! worker fleet — any node, picked fresh per task from a bare `workspace_id`
//! in the payload (see `preagg_executor::PreaggTaskExecutor`), the same shape
//! `HealthEvalTaskExecutor` uses. Unlike health eval, a rebuild also needs a
//! [`WorkspaceManager`] (to resolve the view definitions and the pre-agg
//! config), which is what this module builds.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use oxy::adapters::secrets::SecretsManager;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::WorkingCopy;
use sea_orm::{DatabaseConnection, EntityTrait};
use tokio::sync::Mutex as TokioMutex;
use uuid::Uuid;

use crate::server::service::secret_manager::SecretManagerService;

/// Build a [`WorkspaceManager`] for `workspace_id` from nothing but a
/// database handle.
///
/// **Reads the promoted revision, not the working copy.** The builder is
/// handed the default branch's promoted revision (resolved below), so
/// `compiled_semantic_views()` answers from Postgres and a cycle reads the same
/// views on every node — including a worker with no checkout. Only a workspace
/// that has never promoted a revision falls back to the working copy
/// (`origin_for(_, None)` is `Origin::Disk`). See the comment at the
/// `with_working_copy` call below.
///
/// Trimmed relative to the request-path resolver
/// (`workspace_context::try_attach_workspace_manager`): no branch parameter
/// (a scheduled or on-demand cycle always targets the default branch — same
/// as the pre-scheduling worker), no worktree bookkeeping, no HTTP-shaped
/// error type. Errors are a plain string: the caller reports task failure
/// through the executor's `TaskOutcome`, not an HTTP status.
///
/// The secrets manager is NOT among the trimmings, and this is the one place
/// where the request path's shape has to be copied rather than simplified —
/// see the comment at its construction below.
pub(super) async fn build_workspace_manager(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<WorkspaceManager<WorkingCopy>, String> {
    let row = entity::workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await
        .map_err(|e| format!("workspace lookup failed: {e}"))?
        .ok_or_else(|| format!("workspace {workspace_id} does not exist"))?;
    let path = row
        .path
        .as_deref()
        .ok_or_else(|| format!("workspace {workspace_id} has no path"))?;

    // The cycle always targets the default branch, so resolve its promoted
    // revision here (branch hint `None`), the way `router::recovery` does. The
    // builder does NOT do this for us: a `None` revision means `Origin::Disk`,
    // and on a worker — no working copy — the semantic scan then finds nothing
    // and enqueues a lazy self-heal compile. That compile succeeds, so the
    // failure backoff never engages, and every preagg heartbeat promoted a
    // fresh `local-<uuid>` revision. A workspace with no promoted revision yet
    // still falls through to the working copy, as before.
    //
    // `OnMissing::Empty` rather than a hard error because a workspace that has
    // never been compiled is a real state here, and the rebuild has nothing to
    // do rather than something to fail at.
    let revision_id =
        crate::server::api::compiled_reader::resolve_request_revision(workspace_id, None).await;
    let builder = WorkspaceBuilder::new(workspace_id)
        .with_working_copy(
            std::path::Path::new(path),
            revision_id,
            oxy::config::OnMissing::Empty,
        )
        .await
        .map_err(|e| format!("preagg: workspace build failed: {e}"))?;

    // DB-first with env fallback, exactly as the request path does it
    // (`workspace_context::try_attach_workspace_manager`), so a workspace's
    // stored warehouse credentials are visible to the cycle.
    //
    // FAILING here rather than warning, which is where this diverges from the
    // request path: `WorkspaceBuilder::build` falls back to
    // `SecretsManager::from_environment()` when none is set, so without this
    // every `{{ secrets.* }}` in a connection resolves to an EMPTY STRING —
    // and a warehouse driver reads empty as absent, not as an error. A
    // ClickHouse rollup then dials airlayer's default `http://localhost:8123`
    // with `database = ''` and reports a connection failure that names a host
    // nobody configured. On the request path a missing secrets manager
    // degrades a live query the caller can retry; here it would silently
    // rebuild every rollup in the workspace against the wrong warehouse.
    let secrets_manager =
        SecretsManager::from_database_with_env_fallback(SecretManagerService::new(workspace_id))
            .map_err(|e| {
                format!("preagg: secrets manager unavailable for workspace {workspace_id}: {e}")
            })?;

    builder
        .with_secrets_manager(secrets_manager)
        .build()
        .await
        .map_err(|e| format!("preagg: workspace build failed: {e}"))
}

/// Per-workspace manifest write lock, keyed by workspace id.
///
/// `manifest.json` is one file per workspace's local cache directory; every
/// rebuild rewrites it, so any two cycles touching the SAME workspace — a
/// scheduled fire racing an on-demand "Rebuild" click — must serialize. Two
/// DIFFERENT workspaces' cycles must not: they write to different directories
/// and a single global lock would only add queueing with no correctness
/// benefit, on a fleet where many workspaces' cycles can legitimately run at
/// once.
pub(super) fn manifest_write_lock_for(workspace_id: Uuid) -> Arc<TokioMutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<Uuid, Arc<TokioMutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("preagg manifest lock registry poisoned");
    Arc::clone(
        locks
            .entry(workspace_id)
            .or_insert_with(|| Arc::new(TokioMutex::new(()))),
    )
}

#[cfg(test)]
mod tests {
    //! DB-backed; skips when `OXY_DATABASE_URL` is unset (`test_support::test_db`).
    //! Every row it writes is keyed by a fresh workspace id, so it needs no lock.

    use entity::{revisions, workspace_compiled_configs, workspaces};
    use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
    use uuid::Uuid;

    use super::build_workspace_manager;
    use crate::server::test_support::{SKIP_MSG, test_db};

    /// A workspace whose `path` is a directory with no `config.yml` — a worker
    /// node, which has the column but not the checkout — and a promoted revision
    /// carrying a compiled config.
    async fn seed_promoted_workspace(db: &DatabaseConnection, path: &str) -> (Uuid, Uuid) {
        let now = chrono::Utc::now().fixed_offset();
        let workspace_id = Uuid::new_v4();
        workspaces::ActiveModel {
            id: Set(workspace_id),
            name: Set(format!("preagg-ws-{workspace_id}")),
            git_namespace_id: Set(None),
            git_remote_url: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            path: Set(Some(path.to_string())),
            last_opened_at: Set(None),
            created_by: Set(None),
            org_id: Set(None),
            status: Set(workspaces::WorkspaceStatus::Ready),
            error: Set(None),
            monthly_vlm_budget_micros: Set(None),
            current_revision_id: Set(None),
            default_branch: Set(None),
            repo_subdir: Set(None),
        }
        .insert(db)
        .await
        .expect("seed workspace");

        let revision_id = Uuid::new_v4();
        revisions::ActiveModel {
            revision_id: Set(revision_id),
            workspace_id: Set(workspace_id),
            git_sha: Set(format!("sha-{revision_id}")),
            branch: Set(Some("main".to_string())),
            schema_version: Set(1),
            status: Set("ready".to_string()),
            kind: Set("main".to_string()),
            owner_user_id: Set(None),
            compiler_version: Set("test".to_string()),
            started_at: Set(now),
            finished_at: Set(Some(now)),
            file_count_seen: Set(0),
            file_count_compiled: Set(0),
            file_count_failed: Set(0),
            error_summary: Set(None),
        }
        .insert(db)
        .await
        .expect("seed revision");

        workspace_compiled_configs::ActiveModel {
            revision_id: Set(revision_id),
            databases: Set(serde_json::json!([])),
            models: Set(Some(serde_json::json!([]))),
            integrations: Set(None),
            repositories: Set(None),
            builder_agent: Set(None),
            mcp: Set(None),
            other: Set(None),
        }
        .insert(db)
        .await
        .expect("seed compiled config");

        let mut ws: workspaces::ActiveModel = workspaces::Entity::find_by_id(workspace_id)
            .one(db)
            .await
            .expect("load workspace")
            .expect("workspace exists")
            .into();
        ws.current_revision_id = Set(Some(revision_id));
        ws.update(db).await.expect("promote revision");

        (workspace_id, revision_id)
    }

    /// Regression: the cycle built the workspace with no revision, so the
    /// builder read the (absent) working copy. On a worker the semantic scan
    /// then found nothing and enqueued a lazy self-heal compile — which, since
    /// it succeeds, re-ran on every preagg heartbeat and promoted a fresh
    /// `local-<uuid>` revision every ~5 minutes.
    #[tokio::test]
    async fn builds_from_the_promoted_revision_not_the_working_copy() {
        let Some(db) = test_db().await else {
            eprintln!("{SKIP_MSG}");
            return;
        };
        let no_checkout = tempfile::tempdir().expect("tempdir");
        let (workspace_id, revision_id) =
            seed_promoted_workspace(&db, no_checkout.path().to_str().unwrap()).await;

        let wm = build_workspace_manager(&db, workspace_id)
            .await
            .expect("build workspace manager");

        assert_eq!(
            wm.config_manager.revision_id(),
            Some(revision_id),
            "preagg must read the promoted revision, not the working copy"
        );
    }
}
