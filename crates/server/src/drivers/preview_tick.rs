//! The global driver's per-pass preview step: the held workspace-preview runs
//! (`oxy_previews::queue::sweep`: finish those whose run is terminal, then
//! start each workspace's next queued one), then the custom-app automation
//! runs that ran as previews (`oxy_custom_apps::automation_run::preview_settle`:
//! each poll row reads its outcome now that the run is finished).
//!
//! Row writes only; the runs themselves are queue work. The settle was a step
//! inside the previews sweep until previews stopped depending on custom apps
//! (`internal-docs/crate-tiers.md`, stage A), so the two run here, in the same
//! tick, in the order the sweep ran them.

use sea_orm::DatabaseConnection;

/// One pass. Errors are logged; the next tick retries.
pub(super) async fn run(db: &DatabaseConnection) {
    oxy_previews::queue::sweep::sweep(db).await;
    if let Err(e) =
        oxy_custom_apps::automation_run::preview_settle::settle_custom_app_runs(db).await
    {
        tracing::warn!(target: "preview", error = %e, "could not settle custom-app preview runs");
    }
}
