//! External source adapter abstraction. One impl per system (Toast first);
//! `source_for` maps a `reconcile.yml` `source:` key to an adapter.
//!
//! Sources resolve the external value for a *batch* of checks: Toast's
//! Analytics API is async + batched (one report per window covers every
//! restaurant), so fetching per check would blow the rate limit. The runner
//! groups a workspace's checks by source and calls `fetch_externals` once.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;

use oxy::config::model::ToastAnalyticsIntegration;

use super::config::{ExternalKey, ExternalSpec};
use super::toast::ToastSource;

/// One external value to fetch: an [`ExternalSpec`] (metric + restaurants) and
/// the already-resolved window it applies to. Decouples the source from the
/// check shape so a source batches purely by `(spec, window)` — either side of
/// a check can carry an external operand.
pub struct ExternalRequest<'a> {
    pub spec: &'a ExternalSpec,
    pub window: &'a [String; 2],
    /// The check's `group_by`, mapped to source dimensions. Empty for an
    /// ungrouped check, which then gets no `groups`.
    pub group_by: &'a [ExternalKey],
}

/// One external operand's value: the total (exactly what an ungrouped check
/// compares) and, for a grouped request, the value per group — each key holds
/// the group's values in `ExternalRequest::group_by` order.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternalValue {
    pub total: f64,
    pub groups: Option<Vec<(Vec<String>, f64)>>,
}

/// Per-batch context: the reference instant for window resolution, the
/// adapter's decrypted secrets (keyed by name, only those present), and the
/// time budget for any async report.
pub struct SourceCtx {
    pub now: chrono::DateTime<chrono::Utc>,
    pub secrets: HashMap<String, String>,
    pub report_timeout: Duration,
}

impl SourceCtx {
    /// Decrypted value for `name`, if the workspace had that secret configured.
    pub fn secret(&self, name: &str) -> Option<&str> {
        self.secrets.get(name).map(String::as_str)
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ReconcileError {
    #[error("{0} unreachable")]
    Unreachable(String),
    #[error("{0} rate limited")]
    RateLimited(String),
    #[error("{0} not configured")]
    NotConfigured(String),
    #[error("unknown reconcile source: {0}")]
    Unknown(String),
    #[error("fetch failed: {0}")]
    Fetch(String),
}

#[async_trait]
pub trait ReconcileSource: Send + Sync {
    /// Logical secret slots this source may use, each mapped to the
    /// workspace-secret var name to resolve it from (config-driven, so the
    /// names live in `config.yml` rather than being hardcoded). The runner
    /// resolves every var that exists and keys `SourceCtx::secrets` by the
    /// *logical* slot, so the source reads `ctx.secret("client_id")` regardless
    /// of the configured var name.
    fn secret_vars(&self) -> Vec<(&'static str, String)>;
    /// Resolve the external value for every request (returned in the same
    /// order). Implementations batch network calls where possible — Toast
    /// issues one report per distinct window and shares it across requests.
    async fn fetch_externals(
        &self,
        ctx: &SourceCtx,
        requests: &[ExternalRequest<'_>],
    ) -> Vec<Result<ExternalValue, ReconcileError>>;
}

/// Registry: `reconcile.yml` `source:` string → adapter instance. The
/// workspace's resolved `toast_analytics` integration (bound by the check's
/// `integration:` name) supplies the source's secret var-names and API base
/// URL; `None` falls back to the built-in base-URL default with no secrets.
pub fn source_for(
    id: &str,
    toast: Option<&ToastAnalyticsIntegration>,
) -> Option<Box<dyn ReconcileSource>> {
    match id {
        "toast" => Some(Box::new(ToastSource::from_config(toast))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_source_resolves() {
        assert!(source_for("toast", None).is_some());
    }

    #[test]
    fn unknown_source_is_none() {
        assert!(source_for("square", None).is_none());
    }
}
