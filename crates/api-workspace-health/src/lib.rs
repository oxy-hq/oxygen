//! Workspace health's HTTP surface and the live half of its evaluation.
//!
//! # The crate
//!
//! A sibling surface crate (`internal-docs/domain-boundaries.md`), moved out of
//! `oxy-app`'s `server::api::admin::workspace_health` with its paths, roles and
//! guards unchanged. The domain — the evaluator, the reconcile and smoke
//! verdicts, alerting, history, the rollup and the eval pass itself — is
//! `oxy-workspace-health`, which does not depend on `oxy-app`. What is here is
//! what does:
//!
//! - [`admin_sections`] rides `SurfaceSeams::admin`: the console's routes under
//!   `/api/admin`, behind `PlatformOperate` exactly as they were in `oxy-app`.
//!   FleetOk — they read and enqueue in Postgres and touch no disk.
//! - [`tasks`] rides `SurfaceSeams::tasks`: the `health_eval_workspace`
//!   executor, a kind `oxy-app` queues (the schedule tick, "Re-evaluate", "Run
//!   now") and refuses to start without.
//! - The live runners, [`reconcile_runner::LiveReconcileRunner`] and
//!   [`smoke::LiveSmokeRunner`], build the workspace's project context
//!   (`oxy_workspace_runtime`) and fill the domain's
//!   `ReconcileRunner` / `SmokeRunner` seams.

// The live runners' futures nest deep enough to need it, as `oxy-app`'s do.
#![recursion_limit = "256"]

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use oxy_app_core::AppState;
use oxy_app_core::seams::AdminSection;
use oxy_app_core::seams::TaskSeam;
use oxy_workspace_health::eval_pass::EvalRunners;
use sea_orm::DatabaseConnection;

pub mod executor;
pub mod handlers;
pub mod reconcile_runner;
pub mod smoke;

/// The staff console's workspace-health section, mounted under `/api/admin`
/// through the admin seam behind `PlatformOperate` — the capability it had in
/// `oxy-app`. No route declarations: the console is FleetOk wholesale.
pub fn admin_sections() -> Vec<AdminSection> {
    vec![AdminSection {
        capability: oxy_server_authz::Action::PlatformOperate,
        routes: admin_routes(),
        decls: Vec::new(),
    }]
}

/// Relative to `/admin`. Its own function so the route catalog can seed it.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/workspace-health", get(handlers::list_workspace_health))
        .route(
            "/workspace-health/{workspace_id}/eval",
            post(handlers::trigger_workspace_health_eval),
        )
        .route(
            "/workspace-health/{workspace_id}/history",
            get(handlers::workspace_health_history),
        )
}

/// The task kinds this surface runs: one workspace's health eval.
pub fn tasks() -> Vec<TaskSeam> {
    vec![TaskSeam {
        kind: executor::HEALTH_EVAL_KIND,
        build: |deps| {
            Arc::new(executor::HealthEvalTaskExecutor {
                db: deps.db.clone(),
            })
        },
    }]
}

/// One workspace's eval with the live runners: the domain's
/// [`oxy_workspace_health::eval_pass::run_eval_pass_single`], handed the
/// runners that build the workspace's project context.
pub async fn run_eval_pass_single(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    force_smoke: bool,
) -> Result<String, String> {
    let now = chrono::Utc::now();
    let reconcile = reconcile_runner::LiveReconcileRunner::from_env(now).with_db(db.clone());
    let smoke = smoke::LiveSmokeRunner::from_env().with_db(db.clone());
    let runners = EvalRunners {
        reconcile: &reconcile,
        smoke: &smoke,
        now,
    };
    oxy_workspace_health::eval_pass::run_eval_pass_single(db, workspace_id, force_smoke, runners)
        .await
}

/// The shared in-source DB harness (`oxy_test_utils::db`), as `oxy-workspace-health` takes it.
#[cfg(test)]
mod test_support {
    pub(crate) use oxy_test_utils::db::SKIP_MSG;

    /// [`oxy_test_utils::db::test_db`] with the runtime migrator.
    pub(crate) async fn test_db() -> Option<sea_orm::DatabaseConnection> {
        oxy_test_utils::db::test_db::<agentic_runtime::migration::RuntimeMigrator>().await
    }
}

#[cfg(test)]
mod tests {
    /// The kind string is the persisted task-queue contract, and `oxy-app`
    /// requires it by string (`HEALTH_TASK_KINDS`), since it cannot name this
    /// crate's constant.
    #[test]
    fn the_task_kind_is_the_one_oxy_app_requires() {
        let kinds: Vec<&str> = super::tasks().iter().map(|t| t.kind).collect();
        assert_eq!(kinds, oxy_app_core::seams::HEALTH_TASK_KINDS);
    }
}
