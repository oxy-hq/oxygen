//! Load an airway pipeline's YAML at claim time, re-resolving the platform once
//! when the pinned revision does not serve the ref.
//!
//! A driver builds ONE executor, and so one `PlatformContext`, for a whole run,
//! and a deferred task is re-claimed by that same executor. The context pins the
//! promoted revision it was built at. So when a run is queued seconds after a
//! promote that added its pipeline, by a worker whose context predates the
//! promote, every re-claim asked the same old revision and the `NotInRevision`
//! deferral — whose whole premise is "a compile promoting it lands in seconds" —
//! could never succeed within the drive (BMG prod, 2026-11-11: 16 claims over
//! ~9 minutes, all naming the previous revision, then the run failed).
//!
//! The fix: on `NotInRevision`, ask the platform for a context at the CURRENT
//! promoted revision ([`ProjectContext::at_current_promoted_revision`]) and load
//! once more against it. The caller then runs the rest of the launch on the
//! returned platform, so a run uses one revision throughout — the new YAML is
//! never mixed with the old revision's config.
//!
//! Bounded by construction: there is no loop, so at most two loads per claim.
//! A second `NotInRevision` is returned as-is and defers as before.
//!
//! [`ProjectContext::at_current_promoted_revision`]: crate::platform::ProjectContext::at_current_promoted_revision

use std::sync::Arc;

use crate::pipeline_ref::{PipelineRefError, load_pipeline_yaml};
use crate::platform::PlatformContext;

/// A successful load, and the platform the rest of the run must use.
pub(super) struct LoadedPipeline {
    pub yaml: String,
    /// `Some` when the load succeeded only after re-resolving: the launch must
    /// run on this platform, not the executor's own.
    pub refreshed: Option<Arc<dyn PlatformContext>>,
}

/// [`load_pipeline_yaml`] against `platform`, retried once on a context at the
/// current promoted revision when the first answer is `NotInRevision`.
///
/// Every other error, and a `NotInRevision` the host cannot or need not refresh
/// past (`at_current_promoted_revision` → `None`), is returned unchanged for
/// `action_for_load_failure` to dispose of.
pub(super) async fn load_pipeline_yaml_at_current_revision(
    platform: &Arc<dyn PlatformContext>,
    pipeline_ref: &str,
) -> Result<LoadedPipeline, PipelineRefError> {
    let first = match load_pipeline_yaml(platform.as_ref(), pipeline_ref).await {
        Ok(yaml) => {
            return Ok(LoadedPipeline {
                yaml,
                refreshed: None,
            });
        }
        Err(e @ PipelineRefError::NotInRevision(_)) => e,
        Err(e) => return Err(e),
    };
    let Some(fresh) = platform.at_current_promoted_revision().await else {
        return Err(first);
    };
    tracing::info!(
        pipeline_ref,
        "airway: pinned revision does not serve the pipeline; retrying once at the \
         workspace's current promoted revision"
    );
    let yaml = load_pipeline_yaml(fresh.as_ref(), pipeline_ref).await?;
    Ok(LoadedPipeline {
        yaml,
        refreshed: Some(fresh),
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::platform::ProjectContext;

    const REF: &str = "pipelines/pnl_seller_costs.airway.yml";
    const YAML: &str = "name: pnl_seller_costs\n";

    /// A context pinned to `revision`, serving `REF` only if `serves`, and
    /// handing out `next` (once) as its current-revision rebuild.
    struct Pinned {
        revision: uuid::Uuid,
        serves: bool,
        next: Mutex<Option<Arc<dyn PlatformContext>>>,
        refresh_calls: Arc<AtomicUsize>,
        yaml_reads: AtomicUsize,
    }

    impl Pinned {
        fn new(serves: bool, next: Option<Arc<dyn PlatformContext>>) -> Self {
            Self {
                revision: uuid::Uuid::new_v4(),
                serves,
                next: Mutex::new(next),
                refresh_calls: Arc::new(AtomicUsize::new(0)),
                yaml_reads: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProjectContext for Pinned {
        async fn resolve_connector(&self, _db: &str) -> Option<agentic_connector::ConnectorConfig> {
            None
        }
        async fn resolve_model(
            &self,
            _model_ref: Option<&str>,
            _explicit: bool,
        ) -> Option<agentic_analytics::config::ResolvedModelInfo> {
            None
        }
        async fn resolve_secret(&self, _var: &str) -> Option<String> {
            None
        }
        async fn at_current_promoted_revision(&self) -> Option<Arc<dyn PlatformContext>> {
            self.refresh_calls.fetch_add(1, Ordering::SeqCst);
            self.next.lock().unwrap().take()
        }
    }

    #[async_trait::async_trait]
    impl agentic_automation::WorkspaceContext for Pinned {
        fn workspace_path(&self) -> Option<&Path> {
            None
        }
        fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
            vec![]
        }
        async fn get_connector(
            &self,
            name: &str,
        ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, String> {
            Err(format!("no connector {name}"))
        }
        async fn get_integration(
            &self,
            name: &str,
        ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
            Err(format!("no integration {name}"))
        }
        async fn list_automation_files(&self) -> Result<Vec<PathBuf>, String> {
            Ok(vec![])
        }
        async fn resolve_automation_yaml(
            &self,
            _r: &str,
        ) -> Result<String, crate::WorkspaceReadError> {
            Err("unused".into())
        }
        async fn resolve_pipeline_yaml(&self, r: &str) -> Result<Option<String>, String> {
            self.yaml_reads.fetch_add(1, Ordering::SeqCst);
            Ok((self.serves && r == REF).then(|| YAML.to_string()))
        }
        fn compiled_revision(&self) -> Option<uuid::Uuid> {
            Some(self.revision)
        }
    }

    /// THE incident: the executor's context is pinned to R1, which lacks the
    /// pipeline; R2 is promoted and serves it. The claim must resolve, and
    /// hand back R2's context for the launch.
    #[tokio::test]
    async fn a_miss_on_a_stale_revision_resolves_at_the_current_one() {
        let r2 = Arc::new(Pinned::new(true, None));
        let r2_platform: Arc<dyn PlatformContext> = r2.clone();
        let r1 = Pinned::new(false, Some(r2_platform.clone()));
        let r1_calls = r1.refresh_calls.clone();
        let r1: Arc<dyn PlatformContext> = Arc::new(r1);

        let loaded = load_pipeline_yaml_at_current_revision(&r1, REF)
            .await
            .expect("the current revision serves the pipeline");
        assert_eq!(loaded.yaml, YAML);
        let used = loaded
            .refreshed
            .expect("the launch must run on the fresh platform");
        assert!(
            Arc::ptr_eq(&used, &r2_platform),
            "the launch runs on R2, so the run never mixes R2's YAML with R1's config"
        );
        assert_eq!(r1_calls.load(Ordering::SeqCst), 1);
        assert_eq!(r2.yaml_reads.load(Ordering::SeqCst), 1);
        assert_eq!(
            r2.refresh_calls.load(Ordering::SeqCst),
            0,
            "the fresh platform is never asked to refresh again"
        );
    }

    /// The revision really lacks it: the fresh context misses too. One retry,
    /// no more, and the error is `NotInRevision` so the caller defers as before.
    #[tokio::test]
    async fn a_miss_at_the_current_revision_too_is_returned_after_one_retry() {
        // Even a fresh context that WOULD offer another rebuild is not asked.
        let r3: Arc<dyn PlatformContext> = Arc::new(Pinned::new(true, None));
        let r2 = Arc::new(Pinned::new(false, Some(r3)));
        let r2_calls = r2.refresh_calls.clone();
        let r1: Arc<dyn PlatformContext> = Arc::new(Pinned::new(false, Some(r2.clone())));

        let err = load_pipeline_yaml_at_current_revision(&r1, REF)
            .await
            .err()
            .expect("still not served");
        assert!(matches!(err, PipelineRefError::NotInRevision(_)), "{err:?}");
        assert_eq!(r2.yaml_reads.load(Ordering::SeqCst), 1, "exactly one retry");
        assert_eq!(
            r2_calls.load(Ordering::SeqCst),
            0,
            "bounded: the retry does not refresh again"
        );
    }

    /// A host that cannot rebuild (HTTP, preview, test fakes): today's
    /// behaviour, unchanged.
    #[tokio::test]
    async fn no_rebuild_available_returns_the_original_miss() {
        let r1 = Pinned::new(false, None);
        let calls = r1.refresh_calls.clone();
        let r1: Arc<dyn PlatformContext> = Arc::new(r1);
        let err = load_pipeline_yaml_at_current_revision(&r1, REF)
            .await
            .err()
            .expect("miss");
        assert!(matches!(err, PipelineRefError::NotInRevision(_)), "{err:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// A hit never asks for a rebuild — the common path costs nothing extra.
    #[tokio::test]
    async fn a_hit_does_not_refresh() {
        let r1 = Pinned::new(true, None);
        let calls = r1.refresh_calls.clone();
        let r1: Arc<dyn PlatformContext> = Arc::new(r1);
        let loaded = load_pipeline_yaml_at_current_revision(&r1, REF)
            .await
            .expect("hit");
        assert!(loaded.refreshed.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    /// Only `NotInRevision` refreshes: an invalid ref is the caller's mistake
    /// at every revision.
    #[tokio::test]
    async fn other_errors_do_not_refresh() {
        let r2: Arc<dyn PlatformContext> = Arc::new(Pinned::new(true, None));
        let r1 = Pinned::new(false, Some(r2));
        let calls = r1.refresh_calls.clone();
        let r1: Arc<dyn PlatformContext> = Arc::new(r1);
        let err = load_pipeline_yaml_at_current_revision(&r1, "../escape.airway.yml")
            .await
            .err()
            .expect("invalid");
        assert!(matches!(err, PipelineRefError::Invalid(_)), "{err:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
