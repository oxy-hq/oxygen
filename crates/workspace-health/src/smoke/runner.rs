//! `SmokeRunner`: the seam between the eval pass and whatever runs the probes.
//!
//! *Whether* this pass is a smoke pass is `config::smoke_due`, driven by
//! `last_smoke_at`; *what* gets probed and *how* is the live runner's, in
//! `oxy-api-workspace-health` — it builds the workspace's project context,
//! which only `oxy-app` can compose.

use async_trait::async_trait;
use oxy::config::health_check::SmokeTestConfig;

use super::SmokeVerdict;
use super::config::{resolve_compiled_config, smoke_config_from_value};

#[async_trait]
pub trait SmokeRunner: Send + Sync {
    async fn run_smoke(
        &self,
        workspace_id: uuid::Uuid,
        settings: &SmokeSettings,
    ) -> Vec<SmokeVerdict>;

    /// The working copy's own `smoke_test:` block — read only when the workspace
    /// has no promoted compiled revision (local mode / draft branch). Default:
    /// none, which resolves to the cheap connections-only default.
    async fn working_copy_config(&self, _workspace_id: uuid::Uuid) -> Option<SmokeTestConfig> {
        None
    }
}

/// A resolved smoke config plus whether the workspace actually asked for it.
///
/// The distinction only matters when a probe can't be set up. A workspace that
/// wrote `smoke_test:` in its `config.yml` and then gets no verdicts deserves a
/// Degraded "we couldn't check" — silence there would be a false OK. A workspace
/// that merely inherited the default has promised nothing, so the same failure
/// is silent rather than painting every context-less workspace amber.
pub struct SmokeSettings {
    pub config: SmokeTestConfig,
    pub explicit: bool,
}

/// Resolve the workspace's smoke config: the promoted compiled revision first,
/// then the working copy's own `config.yml` (local mode / draft branch, where
/// there is no compiled row to read), via [`SmokeRunner::working_copy_config`].
///
/// An absent `smoke_test:` block resolves to `SmokeTestConfig::default()` —
/// connections only — so opting into health checks at all buys a cheap
/// credential check for free. It buys nothing for a workspace that *didn't*
/// opt in: the smoke run is gated inside an eval pass, so with no `health_check:`
/// block nothing here runs on its own, only on the manual
/// `POST /workspace-health/{id}/eval` path. Opting *out* of the probe while
/// staying opted into health checks is `smoke_test: { enabled: false }`.
pub async fn resolve_smoke_settings(
    runner: &dyn SmokeRunner,
    workspace_id: uuid::Uuid,
) -> SmokeSettings {
    // A promoted revision is authoritative: if it has no `smoke_test` block, the
    // workspace genuinely has none, and we must not pay to build a workspace
    // context just to re-learn that on every eval pass.
    let config = match resolve_compiled_config(workspace_id).await {
        Some(compiled) => smoke_config_from_value(Some(&compiled)),
        // No compiled revision (local mode / draft branch) — read the working copy.
        None => runner.working_copy_config(workspace_id).await,
    };
    match config {
        Some(config) => SmokeSettings {
            config,
            explicit: true,
        },
        None => SmokeSettings {
            config: SmokeTestConfig::default(),
            explicit: false,
        },
    }
}
