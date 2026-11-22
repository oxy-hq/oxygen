//! `WorkspaceContextCache`: the scheduler recovery path's TTL cache of built
//! `OxyProjectContext`s. Out of `oxy-app`'s `server/router/workspace_cache.rs`
//! on 2026-11-22 (decompose W4), beside the type it is keyed on; `oxy-app`
//! re-exports it at the old path. The generic `TtlCache` and the two
//! `AppState`-held semantic caches are `oxy-app-core`'s.

use std::sync::Arc;
use std::time::Duration;

use oxy_app_core::workspace_cache::TtlCache;

use crate::project_ctx::OxyProjectContext;

const DEFAULT_TTL: Duration = Duration::from_secs(600);

#[doc(hidden)]
pub type WorkspaceContextCache = TtlCache<OxyProjectContext>;

#[doc(hidden)]
pub fn new_workspace_context_cache() -> Arc<WorkspaceContextCache> {
    TtlCache::with_ttl(DEFAULT_TTL)
}
