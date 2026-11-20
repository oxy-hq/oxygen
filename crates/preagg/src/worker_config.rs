//! The bag one `preagg_cycle` task threads through the runtime.

use std::sync::Arc;

use uuid::Uuid;

/// The bag `cycle::run_preagg_task`/`RebuildContext` need, built fresh per task from
/// the payload's `workspace_id` by `oxy-app`'s `preagg_executor`
/// (`build_task_config`).
pub struct PreaggWorkerConfig {
    pub renewal_threshold: std::time::Duration,
    /// The pre-aggregation cache key. Not the workspace PATH, which the
    /// reader side resolves per-branch — see `state_dir::airlayer_cache_key`.
    pub workspace_id: Uuid,
    pub schema: String,
    pub database: Option<String>,
    pub manifest_write_lock: Arc<tokio::sync::Mutex<()>>,
}
