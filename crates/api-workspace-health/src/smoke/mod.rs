//! `SmokeRunner`: run every probe target the config asks for with bounded
//! concurrency and per-probe timeouts, and return one verdict each.
//!
//! *What* gets probed is `targets::collect_targets`; this module is the *how*.
//! The runner is also unconditional — deciding whether this eval pass is a smoke
//! pass belongs to `config::should_run_smoke`, driven by `last_smoke_at`.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use futures::StreamExt;
use oxy::config::health_check::SmokeTestConfig;
use sea_orm::DatabaseConnection;

use oxy_app::surface::workspace_runtime::{OxyProjectContext, build_workspace_ctx};
use oxy_workspace_health::smoke::config::SmokeLimits;
use oxy_workspace_health::smoke::{
    SmokeProbeKind, SmokeRunner, SmokeSettings, SmokeVerdict, failed, passed, timed_out,
    unavailable,
};

use probes::ProbeFailure;
use targets::{Target, collect_targets};

mod probes;
mod targets;

pub struct LiveSmokeRunner {
    limits: SmokeLimits,
    /// Central Postgres handle, needed to build the per-workspace context every
    /// probe runs against. `None` (unit tests) means no probe can run.
    db: Option<DatabaseConnection>,
}

impl LiveSmokeRunner {
    pub fn from_env() -> Self {
        Self {
            limits: SmokeLimits::from_env(),
            db: None,
        }
    }

    pub fn with_db(mut self, db: DatabaseConnection) -> Self {
        self.db = Some(db);
        self
    }
}

#[async_trait]
impl SmokeRunner for LiveSmokeRunner {
    /// The working copy's `health_check.smoke_test`, read through the
    /// workspace's project context — only reached when there is no promoted
    /// compiled revision (local mode / draft branch).
    async fn working_copy_config(&self, workspace_id: uuid::Uuid) -> Option<SmokeTestConfig> {
        let ctx = build_workspace_ctx(workspace_id, self.db.as_ref()?).await?;
        ctx.workspace_manager()
            .config_manager
            .get_config()
            .health_check
            .as_ref()
            .and_then(|hc| hc.smoke_test.clone())
    }

    async fn run_smoke(
        &self,
        workspace_id: uuid::Uuid,
        settings: &SmokeSettings,
    ) -> Vec<SmokeVerdict> {
        let cfg = &settings.config;
        if !cfg.enabled || cfg.is_inert() {
            return Vec::new();
        }
        // A smoke test the workspace *asked for* and that we cannot even set up
        // must not read Healthy — that would be a false OK on exactly the
        // workspaces most likely broken. One it merely inherited by default
        // promised nothing, so it stays silent rather than painting every
        // context-less workspace amber.
        let cannot_run = |reason: &str| -> Vec<SmokeVerdict> {
            if settings.explicit {
                vec![unavailable(
                    SmokeProbeKind::Connection,
                    "workspace",
                    reason.to_string(),
                )]
            } else {
                Vec::new()
            }
        };

        let Some(db) = self.db.as_ref() else {
            return cannot_run("no database handle for the smoke runner");
        };
        let Some(ctx) = build_workspace_ctx(workspace_id, db).await else {
            return cannot_run("workspace context unavailable");
        };

        let (targets, mut verdicts) = collect_targets(&ctx, workspace_id, cfg).await;
        if targets.is_empty() {
            return verdicts;
        }
        verdicts.extend(execute_all(&ctx, workspace_id, targets, &self.limits).await);
        // Probes complete out of order; sort so the checks table and the payload
        // diff are stable across runs.
        verdicts.sort_by(|a, b| a.check.cmp(&b.check));
        verdicts
    }
}

/// Run every target with bounded concurrency, under a whole-run backstop.
async fn execute_all(
    ctx: &Arc<OxyProjectContext>,
    workspace_id: uuid::Uuid,
    targets: Vec<Target>,
    limits: &SmokeLimits,
) -> Vec<SmokeVerdict> {
    let probe_stream = futures::stream::iter(targets)
        .map(|t| run_one(ctx, t, limits))
        .buffer_unordered(limits.concurrency)
        .collect::<Vec<_>>();

    match tokio::time::timeout(limits.total_timeout, probe_stream).await {
        Ok(results) => results,
        Err(_) => {
            // Partial results are not recoverable from a cancelled stream, so
            // report the budget overrun itself. Degraded, not Unhealthy: a slow
            // run tells us nothing about whether the artifacts work.
            tracing::warn!(
                target: "health_eval", %workspace_id,
                "smoke run exceeded its total budget"
            );
            vec![timed_out(
                SmokeProbeKind::Connection,
                "workspace",
                limits.total_timeout,
                limits.total_timeout.as_millis() as u64,
            )]
        }
    }
}

/// Run one probe under its time budget and map the outcome to a verdict.
async fn run_one(
    ctx: &Arc<OxyProjectContext>,
    target: Target,
    limits: &SmokeLimits,
) -> SmokeVerdict {
    let kind = target.kind();
    let label = target.label();
    // The agent probe makes real LLM calls and is the long pole by an order of
    // magnitude; giving it the same budget as a `SELECT 1` would time it out.
    let budget = match kind {
        SmokeProbeKind::Agent => limits.agent_timeout,
        _ => limits.probe_timeout,
    };

    let started = Instant::now();
    let outcome = tokio::time::timeout(budget, execute(ctx, &target)).await;
    let elapsed = started.elapsed().as_millis() as u64;

    match outcome {
        Err(_) => timed_out(kind, label, budget, elapsed),
        Ok(Ok(())) => passed(kind, label, elapsed),
        Ok(Err(ProbeFailure::Broken(reason))) => failed(kind, label, reason, elapsed),
        Ok(Err(ProbeFailure::Unavailable(reason))) => unavailable(kind, label, reason),
    }
}

async fn execute(ctx: &Arc<OxyProjectContext>, target: &Target) -> Result<(), ProbeFailure> {
    match target {
        Target::Connection(db_name) => probes::ping(ctx, db_name).await,
        Target::Semantic(t) => probes::query(ctx, t).await,
        Target::Agent {
            agent_ref, prompt, ..
        } => probes::ask(ctx, agent_ref, prompt).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy::config::health_check::{SemanticProbeConfig, SmokeAgentProbe};
    use oxy_workspace_health::evaluator::HealthStatus;
    use oxy_workspace_health::smoke::config::smoke_config_from_value;

    fn settings(config: SmokeTestConfig, explicit: bool) -> SmokeSettings {
        SmokeSettings { config, explicit }
    }

    #[tokio::test]
    async fn disabled_or_inert_config_runs_nothing() {
        let runner = LiveSmokeRunner::from_env();
        let ws = uuid::Uuid::new_v4();

        let disabled = SmokeTestConfig {
            enabled: false,
            ..SmokeTestConfig::default()
        };
        assert!(
            runner
                .run_smoke(ws, &settings(disabled, true))
                .await
                .is_empty()
        );

        let inert = SmokeTestConfig {
            connections: false,
            semantic: SemanticProbeConfig::Sweep(false),
            agent: None,
            ..SmokeTestConfig::default()
        };
        assert!(
            runner
                .run_smoke(ws, &settings(inert, true))
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn an_explicit_config_that_cannot_run_is_degraded_not_healthy() {
        // The false-OK guard: a smoke test the workspace ASKED for and that we
        // can't set up must never read clear.
        let runner = LiveSmokeRunner::from_env(); // no db handle
        let verdicts = runner
            .run_smoke(
                uuid::Uuid::new_v4(),
                &settings(SmokeTestConfig::default(), true),
            )
            .await;
        assert_eq!(verdicts.len(), 1);
        assert_eq!(verdicts[0].status, HealthStatus::Degraded);
    }

    #[tokio::test]
    async fn a_defaulted_config_that_cannot_run_stays_silent() {
        // Smoke is on by default for every workspace, so a context we can't build
        // (local-mode sentinel, deleted working copy) must NOT paint the whole
        // fleet amber over a probe nobody asked for.
        let runner = LiveSmokeRunner::from_env(); // no db handle
        let verdicts = runner
            .run_smoke(
                uuid::Uuid::new_v4(),
                &settings(SmokeTestConfig::default(), false),
            )
            .await;
        assert!(verdicts.is_empty());
    }

    #[test]
    fn the_default_config_probes_connections_and_nothing_costly() {
        // What a workspace with no `smoke_test:` block actually runs.
        let d = SmokeTestConfig::default();
        assert!(!d.is_inert(), "the default must still do something");
        assert!(d.connections);
        assert!(!d.semantic.enabled() && d.agent.is_none());
    }

    #[test]
    fn agent_probe_parses_from_config_json() {
        let cfg = serde_json::json!({
            "health_check": {
                "smoke_test": {
                    "agent": { "agent_ref": "agents/analytics.agentic.yml", "prompt": "how many orders?" }
                }
            }
        });
        let s = smoke_config_from_value(Some(&cfg)).unwrap();
        let SmokeAgentProbe {
            agent_ref, prompt, ..
        } = s.agent.unwrap();
        assert_eq!(agent_ref, "agents/analytics.agentic.yml");
        assert_eq!(prompt, "how many orders?");
    }

    #[test]
    fn the_customised_surface_survives_the_compile_boundary() {
        // The config reaches the runner as JSON out of `workspace_compiled_configs`,
        // not as YAML, so the untagged bool-or-block enums must round-trip there too.
        let cfg = serde_json::json!({
            "health_check": {
                "smoke_test": {
                    "semantic": [{ "topic": "sales", "measures": ["orders.net", "orders.gross"] }],
                    "apps": [{ "app": "apps/sales.app.yml", "variables": { "region": "us-east" } }],
                    "agents": [
                        { "agent_ref": "a.agentic.yml", "prompt": "one?" },
                        { "agent_ref": "a.agentic.yml", "prompt": "two?", "name": "second" }
                    ]
                }
            }
        });
        let s = smoke_config_from_value(Some(&cfg)).expect("customised block parses from JSON");

        let topics = s.semantic.selection().expect("named topics");
        assert_eq!(topics[0].measures, ["orders.net", "orders.gross"]);

        let agents = s.resolved_agents();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].label, "a.agentic.yml");
        assert_eq!(agents[1].label, "second");
    }
}
