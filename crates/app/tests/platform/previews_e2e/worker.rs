//! A worker in the background, as a deployment has one: every tick it drives
//! the workspace's pending global runs the way the latency worker does — the
//! compile a preview queued, the change check that compile queued, a staff dry
//! run — with the production `PreviewRunResolver` and the change check's
//! executor registered as production registers it, then runs the previews
//! maintenance sweep that advances each workspace's queue.

use std::sync::Arc;
use std::time::Duration;

use agentic_pipeline::platform::PlatformContext;
use agentic_runtime::state::RuntimeState;
use agentic_runtime::worker::CustomTaskRegistry;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy_app::agentic_wiring::OxyProjectContext;
use oxy_app::server::previews::analyze::{PREVIEW_ANALYZE_KIND, PreviewAnalyzeExecutor};
use oxy_app::server::previews::runtime::PreviewRunResolver;
use tokio::task::JoinHandle;

use super::fixture::Fx;

/// Stops the loop when dropped.
pub(super) struct Worker(JoinHandle<()>);

impl Drop for Worker {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The workspace's production context at its promoted revision, with the
/// database a compile dispatch needs.
async fn base_ctx(fx: &Fx, root: &std::path::Path) -> Arc<dyn PlatformContext> {
    std::fs::write(root.join("config.yml"), "databases: []\nmodels: []\n").unwrap();
    let manager = WorkspaceBuilder::new(fx.ws)
        .with_working_copy(root, Some(fx.main_revision), oxy::config::OnMissing::Empty)
        .await
        .expect("base config")
        .build()
        .await
        .expect("base manager");
    Arc::new(OxyProjectContext::new(manager).with_db(Arc::new(fx.db.clone())))
}

pub(super) async fn start(fx: &Fx) -> Worker {
    let root = tempfile::tempdir().unwrap();
    let base = base_ctx(fx, root.path()).await;
    let db = fx.db.clone();
    let ws = fx.ws;
    let resolver = PreviewRunResolver::shared(&db);
    let mut registry = CustomTaskRegistry::new();
    registry.register(
        PREVIEW_ANALYZE_KIND,
        Arc::new(PreviewAnalyzeExecutor::airhouse(db.clone())),
    );
    let registry = Arc::new(registry);
    let state = Arc::new(RuntimeState::new());
    Worker(tokio::spawn(async move {
        let _root = root;
        loop {
            agentic_pipeline::recovery::recover_pending_global_runs(
                db.clone(),
                state.clone(),
                base.clone(),
                resolver.clone(),
                None,
                None,
                None,
                None,
                Arc::new(agentic_runtime::router::NoopTaskRouter),
                Some(ws),
                Some(registry.clone()),
                &[],
            )
            .await;
            oxy_app::server::previews::runs::sweep(&db).await;
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }))
}
