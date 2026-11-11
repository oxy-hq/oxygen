//! Shim: the generic `TtlCache` and the two `AppState`-held aliases
//! (`SemanticLayerCache`, `SemanticEngineCache`) moved to `oxy-app-core`.
//! Re-exported here so existing `server::router::workspace_cache::*` paths are
//! unchanged.
//!
//! `WorkspaceContextCache` stays in `oxy-app`: it is keyed on
//! `agentic_wiring::OxyProjectContext` (the pipeline adapter, oxy-app-internal),
//! and `AppState` does not carry it — only the scheduler recovery path
//! (`recovery.rs`) uses it.

use std::sync::Arc;
use std::time::Duration;

pub use oxy_app_core::workspace_cache::{
    SemanticEngineCache, SemanticLayerCache, TtlCache, new_semantic_engine_cache,
    new_semantic_layer_cache,
};

use crate::agentic_wiring::OxyProjectContext;

const DEFAULT_TTL: Duration = Duration::from_secs(600);

#[doc(hidden)]
pub type WorkspaceContextCache = TtlCache<OxyProjectContext>;

#[doc(hidden)]
pub fn new_workspace_context_cache() -> Arc<WorkspaceContextCache> {
    TtlCache::with_ttl(DEFAULT_TTL)
}
