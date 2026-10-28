//! Resolving the smoke-test config, and deciding whether this eval pass is the
//! one that runs it.
//!
//! The config lives inside `config.yml` under `health_check.smoke_test`, so it
//! needs no new compile-boundary artifact: the compiled reader already serves
//! `config.yml` from `workspace_compiled_configs`, and `health_check` rides
//! through the `other` JSONB column. The FS path is a fallback for local mode
//! and draft branches, exactly as `reconcile` does for `reconcile.yml`.

use std::time::Duration;

use oxy::config::health_check::{HealthCheckConfig, SmokeTestConfig, resolve_smoke_interval};

use crate::server::api::compiled_reader::resolve_workspace_config;

/// Per-probe time budgets and fan-out, env-overridable so ops can retune a
/// pathological workspace without a redeploy. Every value is clamped to at least
/// 1 — a zero timeout would fail every probe instantly and read as Degraded
/// across the fleet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmokeLimits {
    /// Budget for one connection / semantic / app probe.
    pub probe_timeout: Duration,
    /// Budget for the agent probe, which makes real LLM calls and is the long
    /// pole by an order of magnitude.
    pub agent_timeout: Duration,
    /// Backstop for the whole smoke run. On expiry the run reports a single
    /// Degraded verdict rather than partial results — the eval pass must not be
    /// held open indefinitely by a wedged warehouse.
    pub total_timeout: Duration,
    /// How many probes run at once.
    pub concurrency: usize,
}

impl Default for SmokeLimits {
    fn default() -> Self {
        Self {
            probe_timeout: Duration::from_secs(30),
            agent_timeout: Duration::from_secs(120),
            total_timeout: Duration::from_secs(300),
            concurrency: 4,
        }
    }
}

impl SmokeLimits {
    pub fn from_env() -> Self {
        let d = Self::default();
        Self {
            probe_timeout: env_secs("OXY_SMOKE_PROBE_TIMEOUT_SECS", d.probe_timeout),
            agent_timeout: env_secs("OXY_SMOKE_AGENT_TIMEOUT_SECS", d.agent_timeout),
            total_timeout: env_secs("OXY_SMOKE_TOTAL_TIMEOUT_SECS", d.total_timeout),
            concurrency: env_usize("OXY_SMOKE_CONCURRENCY", d.concurrency),
        }
    }
}

fn env_secs(key: &str, fallback: Duration) -> Duration {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(|s| Duration::from_secs(s.max(1)))
        .unwrap_or(fallback)
}

fn env_usize(key: &str, fallback: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.max(1))
        .unwrap_or(fallback)
}

/// Pull `health_check.smoke_test` out of an already-resolved `config.yml` JSON
/// object. Pure, so the compiled and FS paths share one parse.
///
/// A `health_check` block that fails to deserialize yields `None` — a malformed
/// cadence must never wedge the eval pass, matching `resolve_interval`'s
/// fail-safe contract. It warns on the way, though: `health_check` rides the
/// `other` JSONB catch-all, so compile never validates it and a typo'd key is
/// only ever rejected here. Silently dropping a smoke config the tenant wrote
/// looks exactly like one they never wrote.
///
/// `None` is not "no smoke test": the caller maps it to
/// [`SmokeTestConfig::default`] (connections probe only), so the credential
/// check still runs. In practice only on the manual `POST
/// /workspace-health/{id}/eval` path — an unparseable block also disables the
/// schedule, so nothing fires on its own.
pub fn smoke_config_from_value(config: Option<&serde_json::Value>) -> Option<SmokeTestConfig> {
    let hc = config.and_then(|v| v.get("health_check"))?;
    match serde_json::from_value::<HealthCheckConfig>(hc.clone()) {
        Ok(parsed) => parsed.smoke_test,
        Err(e) => {
            tracing::warn!(
                target: "health_eval",
                error = %e,
                "config.yml has a `health_check:` block that could not be read (an unknown \
                 or mistyped key, or an empty block — write `health_check: {{}}` for the \
                 defaults); falling back to the default smoke config (connections probe \
                 only) for this workspace"
            );
            None
        }
    }
}

/// The workspace's promoted compiled `config.yml`, as raw JSON.
///
/// `None` means there is no compiled revision to read (local mode / draft branch
/// on a non-serve node) — *not* that the config lacks a `smoke_test` block.
/// Callers need that distinction: a promoted revision without the block is
/// authoritative ("this workspace has no smoke config"), whereas no revision at
/// all means fall through to the working copy.
pub async fn resolve_compiled_config(workspace_id: uuid::Uuid) -> Option<serde_json::Value> {
    resolve_workspace_config(workspace_id, None)
        .await
        .map_err(|e| {
            tracing::warn!(target: "health_eval", %workspace_id, error = %e,
                "compiled config read failed for smoke test");
        })
        .ok()
        .flatten()
}

/// Whether this eval pass runs the probes: the cadence has elapsed, **or** the
/// smoke config changed since the run that produced the stored verdicts.
///
/// The config arm is not an optimization — it is what keeps the tab honest. The
/// stamp records *when* the last run happened, not *what it ran*, so without
/// this a workspace that turns on `semantic`/`agent` keeps serving the
/// old run's verdicts for up to a whole interval (6h by default) while the UI
/// already reports the new kinds as enabled. The two disagree, and an enabled
/// kind with no carried-forward verdicts renders as "No targets found" — a
/// workspace full of topics reading as a workspace with none. A config that
/// differs from the one the stored verdicts were produced under is due now.
pub fn smoke_due(
    prev_config: Option<&SmokeTestConfig>,
    current: &SmokeTestConfig,
    last_smoke_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    if prev_config != Some(current) {
        return true;
    }
    should_run_smoke(last_smoke_at, now, smoke_interval(current))
}

/// Whether the cadence alone says the probes are due.
///
/// The smoke test rides inside the 10-minute eval pass but fires on its own
/// (default 6h) cadence, so most passes skip and reuse the previous verdicts.
/// `last_smoke_at == None` means it has never run, so it runs now.
pub fn should_run_smoke(
    last_smoke_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    now: chrono::DateTime<chrono::Utc>,
    interval: Duration,
) -> bool {
    let Some(last) = last_smoke_at else {
        return true;
    };
    let elapsed = now.signed_duration_since(last.with_timezone(&chrono::Utc));
    // A `last_smoke_at` in the future (clock skew across replicas, a restored
    // backup) would otherwise wedge the probe off for as long as the skew lasts.
    // Treat any non-positive elapsed as "due now".
    match elapsed.to_std() {
        Ok(e) => e >= interval,
        Err(_) => true,
    }
}

/// The cadence for a resolved smoke config, clamped to `[10m, 7d]`.
pub fn smoke_interval(cfg: &SmokeTestConfig) -> Duration {
    resolve_smoke_interval(Some(cfg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use oxy::config::health_check::SemanticProbeConfig;

    fn utc(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn extracts_smoke_block_from_compiled_config() {
        let cfg = serde_json::json!({
            "databases": [],
            "health_check": { "interval": "10m", "smoke_test": { "interval": "6h", "apps": true } }
        });
        let s = smoke_config_from_value(Some(&cfg)).expect("smoke block found");
        assert!(s.enabled);
        assert_eq!(smoke_interval(&s), Duration::from_secs(21_600));
    }

    #[test]
    fn absent_blocks_yield_none() {
        assert!(smoke_config_from_value(None).is_none());
        assert!(smoke_config_from_value(Some(&serde_json::json!({ "databases": [] }))).is_none());
        // `health_check` present but no `smoke_test` → no *explicit* config.
        // The caller reads that as `SmokeTestConfig::default()`, not as "run
        // nothing" — see `resolve_smoke_settings`.
        assert!(
            smoke_config_from_value(Some(&serde_json::json!({
                "health_check": { "interval": "10m" }
            })))
            .is_none()
        );
    }

    #[test]
    fn malformed_health_check_does_not_panic_or_wedge() {
        // `deny_unknown_fields` rejects this; we must degrade to the default
        // smoke config rather than propagate an error into the eval pass.
        let cfg = serde_json::json!({ "health_check": { "bogus_key": 1 } });
        assert!(smoke_config_from_value(Some(&cfg)).is_none());
    }

    #[test]
    fn a_bare_health_check_key_degrades_like_a_malformed_one() {
        // `health_check:` with nothing under it reaches this reader as
        // `Value::Null` (unprojected keys ride the compiled `other` catch-all as
        // raw YAML→JSON), and `HealthCheckConfig` rejects null rather than
        // yielding its default — the same shape the compiled reader pins in
        // `compile_worker::a_bare_health_check_key_is_off_like_the_typed_path`.
        // Pins the outcome only: unlike that sibling, this reader exposes no
        // branch signal, so which arm produced the `None` isn't observable from
        // here and the warning itself stays unasserted.
        let cfg = serde_json::json!({ "health_check": serde_json::Value::Null });
        assert!(smoke_config_from_value(Some(&cfg)).is_none());
    }

    #[test]
    fn never_run_is_due_immediately() {
        assert!(should_run_smoke(
            None,
            utc("2026-07-10T00:00:00Z"),
            Duration::from_secs(21_600)
        ));
    }

    #[test]
    fn runs_only_once_the_interval_has_elapsed() {
        let last = chrono::Utc
            .with_ymd_and_hms(2026, 7, 10, 0, 0, 0)
            .unwrap()
            .fixed_offset();
        let six_h = Duration::from_secs(21_600);
        // 10 minutes later — the next eval pass, but not the next smoke run.
        assert!(!should_run_smoke(
            Some(last),
            utc("2026-07-10T00:10:00Z"),
            six_h
        ));
        // Just shy of 6h.
        assert!(!should_run_smoke(
            Some(last),
            utc("2026-07-10T05:59:00Z"),
            six_h
        ));
        assert!(should_run_smoke(
            Some(last),
            utc("2026-07-10T06:00:00Z"),
            six_h
        ));
    }

    fn ten_minutes_ago() -> chrono::DateTime<chrono::FixedOffset> {
        chrono::Utc
            .with_ymd_and_hms(2026, 7, 10, 0, 0, 0)
            .unwrap()
            .fixed_offset()
    }

    #[test]
    fn enabling_a_probe_kind_makes_the_smoke_test_due_immediately() {
        // The regression: a workspace turns on `semantic`/`agent` in
        // config.yml. The cadence says "not for another 6h", but the tab renders
        // the new kinds as enabled *beside the old run's verdicts* — which have
        // no semantic/agent checks at all, so a workspace full of topics
        // reads as "No targets found". A changed config must re-probe now.
        let before = SmokeTestConfig::default(); // connections only
        let after = SmokeTestConfig {
            semantic: SemanticProbeConfig::Sweep(true),
            ..SmokeTestConfig::default()
        };
        let now = utc("2026-07-10T00:10:00Z"); // 10m after the last smoke run

        assert!(
            !smoke_due(Some(&before), &before, Some(ten_minutes_ago()), now),
            "an unchanged config still waits out its interval"
        );
        assert!(
            smoke_due(Some(&before), &after, Some(ten_minutes_ago()), now),
            "a newly-enabled probe kind must not wait 6h to produce its first verdict"
        );
    }

    #[test]
    fn editing_a_selection_re_probes_without_waiting_out_the_interval() {
        // Same reason as toggling a kind on: the stamp records *when* the last
        // run happened, not *what* it ran. Adding a topic to the list
        // means the stored verdicts no longer
        // describe the config the tab is showing.
        let before = SmokeTestConfig {
            semantic: serde_yaml::from_str("[{ topic: sales }]").unwrap(),
            ..SmokeTestConfig::default()
        };
        let after = SmokeTestConfig {
            semantic: serde_yaml::from_str("[{ topic: sales }, { topic: support }]").unwrap(),
            ..SmokeTestConfig::default()
        };
        let now = utc("2026-07-10T00:10:00Z");
        assert!(!smoke_due(
            Some(&before),
            &before,
            Some(ten_minutes_ago()),
            now
        ));
        assert!(smoke_due(
            Some(&before),
            &after,
            Some(ten_minutes_ago()),
            now
        ));

        // Adding a second agent prompt is the same story.
        let with_agent = SmokeTestConfig {
            agents: serde_yaml::from_str("[{ agent_ref: a.agentic.yml, prompt: hi }]").unwrap(),
            ..before.clone()
        };
        assert!(smoke_due(
            Some(&before),
            &with_agent,
            Some(ten_minutes_ago()),
            now
        ));
    }

    #[test]
    fn an_unknown_previous_config_is_due_now() {
        // A state row written before `smoke_config` was persisted: we cannot
        // vouch that the stored verdicts match the config the UI is showing, so
        // re-probe once rather than serve a tab that may contradict itself.
        let cfg = SmokeTestConfig::default();
        assert!(smoke_due(
            None,
            &cfg,
            Some(ten_minutes_ago()),
            utc("2026-07-10T00:10:00Z")
        ));
    }

    #[test]
    fn a_future_last_smoke_at_is_due_now_not_wedged() {
        // Clock skew between replicas, or a restored backup, must not disable the
        // probe until the skew unwinds.
        let last = chrono::Utc
            .with_ymd_and_hms(2027, 1, 1, 0, 0, 0)
            .unwrap()
            .fixed_offset();
        assert!(should_run_smoke(
            Some(last),
            utc("2026-07-10T00:00:00Z"),
            Duration::from_secs(21_600)
        ));
    }

    #[test]
    fn limits_clamp_zero_to_one_second() {
        // SAFETY: nextest gives each test its own process; no concurrent env access.
        unsafe {
            std::env::set_var("OXY_SMOKE_PROBE_TIMEOUT_SECS", "0");
            std::env::set_var("OXY_SMOKE_CONCURRENCY", "0");
        }
        let l = SmokeLimits::from_env();
        assert_eq!(l.probe_timeout, Duration::from_secs(1));
        assert_eq!(l.concurrency, 1);
        // Unset keys keep their defaults.
        assert_eq!(l.agent_timeout, Duration::from_secs(120));
    }
}
