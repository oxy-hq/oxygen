//! Shim: the generic `TtlCache` and the two `AppState`-held aliases
//! (`SemanticLayerCache`, `SemanticEngineCache`) moved to `oxy-app-core`.
//! Re-exported here so existing `server::router::workspace_cache::*` paths are
//! unchanged.
//!
//! `WorkspaceContextCache`, keyed on `OxyProjectContext`, followed that type
//! down into `oxy-workspace-runtime` on 2026-11-22 (decompose W4); `AppState`
//! does not carry it — only the scheduler recovery path (`recovery.rs`) uses
//! it. Re-exported here too.

pub use oxy_app_core::workspace_cache::{
    SemanticEngineCache, SemanticLayerCache, new_semantic_engine_cache, new_semantic_layer_cache,
};
pub use oxy_workspace_runtime::workspace_cache::{
    WorkspaceContextCache, new_workspace_context_cache,
};
