//! `build_custom_task_registry` and the task kinds the composition root hands in
//! (`SurfaceSeams::tasks`).

use agentic_core::delegation::TaskAssignment;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};

use super::*;

struct Dummy;

#[async_trait::async_trait]
impl TaskExecutor for Dummy {
    async fn execute(&self, _assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        Err("dummy".to_string())
    }
}

fn seam(kind: &'static str) -> TaskSeam {
    TaskSeam {
        kind,
        build: |_| Arc::new(Dummy),
    }
}

fn registry(tasks: &[TaskSeam]) -> Arc<agentic_runtime::worker::CustomTaskRegistry> {
    build_custom_task_registry(
        &sea_orm::DatabaseConnection::default(),
        &PreaggCacheCtx::default(),
        tasks,
        None,
    )
}

/// A kind the composition root hands in is registered, and one it does not hand
/// in is not: the registry knows the seam's kinds and nothing the seam omits,
/// so a queued task of an omitted kind fails as unknown instead of running.
#[test]
fn the_registry_holds_the_kinds_the_seam_hands_in_and_no_others() {
    let handed_in = registry(&[seam("custom_app_staging_migrations")]);
    assert!(handed_in.get("custom_app_staging_migrations").is_some());
    assert!(
        handed_in.get("custom_app_sandbox_teardown").is_none(),
        "a kind absent from the seam must fail as unknown, not be registered by `oxy-app`"
    );
    assert!(
        registry(&[]).get("custom_app_staging_migrations").is_none(),
        "`oxy-app` must not register a custom-app kind itself"
    );
}

/// The in-tree kinds do not depend on the seam.
#[test]
fn the_in_tree_kinds_are_registered_without_the_seam() {
    let reg = registry(&[]);
    assert!(reg.get("preagg_cycle").is_some());
}

/// The health eval is `oxy-api-workspace-health`'s: absent without the seam,
/// present once it is handed in.
#[test]
fn the_health_eval_kind_comes_only_from_the_seam() {
    assert!(
        registry(&[]).get("health_eval_workspace").is_none(),
        "`oxy-app` must not register the health eval itself"
    );
}
