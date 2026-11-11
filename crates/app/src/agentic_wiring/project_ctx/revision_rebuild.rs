//! Re-resolving a long-lived context to the workspace's current promoted
//! revision — the host half of
//! [`ProjectContext::at_current_promoted_revision`].
//!
//! An `OxyProjectContext` pins the revision it was built at. That is right for a
//! request, which must see one revision start to finish, and wrong for the
//! background driver, which builds one executor per run and re-claims deferred
//! tasks on it: an airway run queued just after a promote that added its
//! pipeline re-asked the old revision on every claim until it timed out.
//!
//! The rebuild itself needs the driver's cache and its build path, both in
//! `server::router`, so this module only declares the hook; the driver attaches
//! an implementation (`server::router::workspace_cache`) to the contexts it
//! caches. A context without one — every HTTP and preview context — answers
//! `None`, exactly as before.
//!
//! [`ProjectContext::at_current_promoted_revision`]: agentic_pipeline::platform::ProjectContext::at_current_promoted_revision

use std::sync::Arc;

use agentic_pipeline::platform::PlatformContext;
use async_trait::async_trait;

use super::OxyProjectContext;

/// Rebuilds a context at the workspace's current promoted revision.
#[async_trait]
pub trait RevisionRebuilder: Send + Sync {
    /// A context at the revision a fresh resolve pins now, or `None` when that
    /// is still `pinned`, nothing resolves, or the build fails.
    async fn rebuild_unless_at(&self, pinned: uuid::Uuid) -> Option<Arc<OxyProjectContext>>;
}

impl OxyProjectContext {
    /// Let this context be re-resolved to the current promoted revision. Only
    /// the background driver sets this, on the contexts it caches.
    pub(crate) fn with_revision_rebuilder(mut self, rebuilder: Arc<dyn RevisionRebuilder>) -> Self {
        self.revision_rebuilder = Some(rebuilder);
        self
    }

    /// The body of `ProjectContext::at_current_promoted_revision`.
    ///
    /// Compares against the revision this context's `ConfigManager` is pinned
    /// to, and re-checks the rebuilt one: a rebuild that lands on the same
    /// revision is `None`, which is what bounds a caller to one retry.
    pub(super) async fn rebuilt_at_current_revision(&self) -> Option<Arc<dyn PlatformContext>> {
        let rebuilder = self.revision_rebuilder.as_ref()?;
        let pinned = self.workspace_manager.config_manager.revision_id()?;
        let fresh = rebuilder.rebuild_unless_at(pinned).await?;
        if fresh.workspace_manager.config_manager.revision_id() == Some(pinned) {
            return None;
        }
        tracing::info!(
            workspace_id = %self.workspace_manager.workspace_id,
            stale_revision = %pinned,
            current_revision = ?fresh.workspace_manager.config_manager.revision_id(),
            "re-resolved a pinned context to the current promoted revision"
        );
        Some(fresh)
    }
}
