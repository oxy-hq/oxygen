//! `/meta` reads no filesystem on a diskless replica.
//!
//! Moved from `oxy-app`'s `routing` group (`fleet_canary`) with the handler it
//! drives (2026-11-16). The probe it reads, `oxy::workspace_fs_probe`, is the
//! same instrument `fleet_canary` verifies there; the guard and the workspace
//! row below are copies of that file's, since a test group cannot share a
//! module across crates.

use oxy::workspace_fs_probe::{leaks, reset_leaks, set_process_owns_workspace_files};
use serial_test::serial;

/// Restores the global on drop so one test's configuration can't leak into
/// another's, whatever the outcome.
struct AsDisklessReplica;

impl AsDisklessReplica {
    fn enter() -> Self {
        set_process_owns_workspace_files(false);
        reset_leaks();
        AsDisklessReplica
    }
}

impl Drop for AsDisklessReplica {
    fn drop(&mut self) {
        set_process_owns_workspace_files(true);
        reset_leaks();
    }
}

/// `/meta` is declared FleetOk (`route_fleet` in `oxy-app` before
/// 2026-11-16), so a replica answers it — and until this was split it answered by delegating to `get_workspace`, which resolves the
/// workspace root and then runs `detect_git_mode` / `get_default_branch` /
/// `get_current_branch` and builds an `Origin::Disk` ConfigManager, before
/// `/meta` threw every git field away.
///
/// Two costs, and the second is the one that showed: on an fs-writable pod
/// whose volume is not populated yet, `build_workspace_details_response` raises
/// the "materializing" 503 — so `/meta` reported unavailable for data no volume
/// holds. Its whole response is DB row plus constants.
///
/// Asserting through the probe rather than on the status code: a 200 could
/// still have walked the filesystem to produce it.
#[tokio::test]
#[serial]
async fn the_meta_route_reads_no_filesystem_on_a_replica() {
    let _guard = AsDisklessReplica::enter();

    let workspace_id = uuid::Uuid::new_v4();
    let row = entity::workspaces::Model {
        id: workspace_id,
        name: "canary".to_string(),
        path: Some("/nonexistent/workspace/oxy".to_string()),
        ..fake_workspace()
    };

    let meta = oxy_api_workspaces::workspaces::get_workspace_meta(
        axum::extract::State(oxy_app::surface::bare_app_state()),
        oxy_auth::extractor::AuthenticatedUserExtractor(oxy_auth::types::AuthenticatedUser {
            id: uuid::Uuid::new_v4(),
            email: Some("canary@example.com".to_string()),
            name: "canary".to_string(),
            picture: None,
            status: entity::users::UserStatus::Active,
            credential: None,
        }),
        oxy_server_authz::workspace_role::EffectiveWorkspaceRole(
            entity::workspace_members::WorkspaceRole::Admin,
        ),
        axum::extract::Extension(row),
        axum::extract::Path(workspace_id),
        // The stub passes `true`: this test is about the working copy, not Factory access.
        oxy_app::surface::factory_access::FactoryAccess(true),
    )
    .await
    .expect("meta needs no working copy, so it cannot fail for want of one");

    assert_eq!(meta.0.id, workspace_id);
    assert_eq!(meta.0.name, "canary");
    assert_eq!(
        leaks(),
        0,
        "`/meta` resolved a workspace path on a diskless replica — it is \
         mounted FleetOk, so every field it returns must come from the database \
         row or a constant"
    );
}

/// A workspace row carrying only what path resolution reads: `id` and `path`.
/// The rest is inert here.
fn fake_workspace() -> entity::workspaces::Model {
    let now = chrono::Utc::now().into();
    entity::workspaces::Model {
        id: uuid::Uuid::nil(),
        name: String::new(),
        git_namespace_id: None,
        git_remote_url: None,
        created_at: now,
        updated_at: now,
        path: None,
        last_opened_at: None,
        created_by: None,
        org_id: None,
        status: entity::workspaces::WorkspaceStatus::Ready,
        error: None,
        monthly_vlm_budget_micros: None,
        current_revision_id: None,
        default_branch: None,
        repo_subdir: None,
        default_branch_attempted_at: None,
    }
}
