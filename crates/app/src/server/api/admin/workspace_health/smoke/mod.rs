//! Workspace smoke test: active end-to-end probes of a workspace's own
//! artifacts, rolled up as the `smoke_test` health dimension.
//!
//! Where the passive dimensions count rows in Postgres, this one *exercises* the
//! workspace: pings every warehouse connection, runs a measure query through the
//! semantic model, and asks an agent a fixed question. It is
//! the same shape as `reconcile` — a runner trait producing verdicts that the
//! evaluator folds into a dimension — but it runs on its own slower cadence
//! (default 6h) because every probe costs a warehouse round-trip and the agent
//! probe costs tokens. See `config.rs` for the cadence gate.
//!
//! Verdict semantics, mirroring `reconcile::unreachable_verdict`:
//! * probe ran and passed → Healthy
//! * probe ran and failed → **Unhealthy** (the artifact is genuinely broken)
//! * probe timed out, or could not be attempted → **Degraded** (unknown, not
//!   wrong — an unknown must never page as a hard failure)
//! * targets skipped by the `max_targets` cap → Healthy, with a reason. Caps are
//!   recorded, never silent, but a large workspace is not a degraded one.

pub(crate) mod config;
pub(crate) mod probes;
pub(crate) mod runner;
pub(crate) mod targets;

use serde::{Deserialize, Serialize};

use super::evaluator::HealthStatus;

pub(crate) use runner::{LiveSmokeRunner, SmokeRunner};

/// Which probe produced a verdict. Serialized into the health payload, so the
/// admin UI can group checks by kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmokeProbeKind {
    /// `SELECT 1` against a configured database.
    Connection,
    /// One measure query per topic, through the semantic model.
    Semantic,
    /// Every task of one `.app.yml`. Data Apps are removed, so nothing produces
    /// this any more; it stays because verdicts stored before then still carry
    /// it and are read back as-is.
    App,
    /// A fixed question put to an agentic agent.
    Agent,
}

impl SmokeProbeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SmokeProbeKind::Connection => "connection",
            SmokeProbeKind::Semantic => "semantic",
            SmokeProbeKind::App => "app",
            SmokeProbeKind::Agent => "agent",
        }
    }
}

/// One smoke-probe outcome, persisted in the health payload and surfaced in the
/// per-workspace Health tab.
///
/// Unlike `DriftVerdict` this is `Deserialize` as well as `Serialize`: on an
/// eval pass where the smoke interval has not elapsed, the previous run's
/// verdicts are read back out of the stored payload and reused verbatim, so the
/// dimension keeps its last known value instead of flapping to Healthy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SmokeVerdict {
    /// Stable machine id, `"<kind>:<target>"` — e.g. `"connection:bigquery"`.
    pub check: String,
    pub kind: SmokeProbeKind,
    /// What was probed: a database name, topic name, app path, or agent ref.
    pub target: String,
    pub status: HealthStatus,
    pub reason: Option<String>,
    /// Wall time of the probe. `0` for verdicts that never ran a probe (caps,
    /// unavailable context).
    pub duration_ms: u64,
}

impl SmokeVerdict {
    fn new(
        kind: SmokeProbeKind,
        target: impl Into<String>,
        status: HealthStatus,
        reason: Option<String>,
        duration_ms: u64,
    ) -> Self {
        let target = target.into();
        Self {
            check: format!("{}:{}", kind.as_str(), target),
            kind,
            target,
            status,
            reason,
            duration_ms,
        }
    }
}

/// The probe ran and succeeded.
pub fn passed(kind: SmokeProbeKind, target: impl Into<String>, duration_ms: u64) -> SmokeVerdict {
    SmokeVerdict::new(kind, target, HealthStatus::Healthy, None, duration_ms)
}

/// The probe ran and the artifact is broken — a compile error, a bad join, a
/// failing app task, an agent that errored. This is the one path to Unhealthy.
pub fn failed(
    kind: SmokeProbeKind,
    target: impl Into<String>,
    reason: String,
    duration_ms: u64,
) -> SmokeVerdict {
    SmokeVerdict::new(
        kind,
        target,
        HealthStatus::Unhealthy,
        Some(reason),
        duration_ms,
    )
}

/// The probe exceeded its time budget. Like an unreachable reconcile source, a
/// timeout is *unknown*, not *wrong* — Degraded, never Unhealthy. A cold
/// warehouse that autoscales up must not page anyone.
pub fn timed_out(
    kind: SmokeProbeKind,
    target: impl Into<String>,
    budget: std::time::Duration,
    duration_ms: u64,
) -> SmokeVerdict {
    let secs = budget.as_secs();
    SmokeVerdict::new(
        kind,
        target,
        HealthStatus::Degraded,
        Some(format!("probe exceeded its {secs}s budget")),
        duration_ms,
    )
}

/// The probe could not be attempted at all: no workspace context, no compiled
/// revision, no semantic runner wired. Degraded — we learned nothing, and
/// reporting Healthy here would be a false OK.
pub fn unavailable(
    kind: SmokeProbeKind,
    target: impl Into<String>,
    reason: String,
) -> SmokeVerdict {
    SmokeVerdict::new(kind, target, HealthStatus::Degraded, Some(reason), 0)
}

/// A Healthy verdict that carries a note rather than a problem — used to record
/// targets dropped by the `max_targets` cap. Surfaced in the checks table so the
/// cap is visible, but it does not move the dimension: a workspace with more
/// topics than the cap is large, not unhealthy.
pub fn note(kind: SmokeProbeKind, target: impl Into<String>, reason: String) -> SmokeVerdict {
    SmokeVerdict::new(kind, target, HealthStatus::Healthy, Some(reason), 0)
}

/// Whether one probe kind is turned on for a workspace, persisted alongside the
/// verdicts so the admin UI can tell a **disabled** probe (show "not enabled")
/// from an **enabled** one that simply found no targets or hasn't run yet (show
/// "no results"). Without this the two are indistinguishable — both are just an
/// absence of verdicts — and the tab can't explain why a kind shows nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SmokeProbeStatus {
    pub kind: SmokeProbeKind,
    pub enabled: bool,
}

/// The enabled/disabled state of every probe kind, in the fixed cost order the
/// UI renders. Always three entries — a disabled kind is present-and-false, not
/// omitted, so the tab can name it.
pub fn probe_statuses(cfg: &oxy::config::health_check::SmokeTestConfig) -> Vec<SmokeProbeStatus> {
    [
        (SmokeProbeKind::Connection, cfg.connections),
        (SmokeProbeKind::Semantic, cfg.semantic.enabled()),
        (SmokeProbeKind::Agent, !cfg.resolved_agents().is_empty()),
    ]
    .into_iter()
    .map(|(kind, enabled)| SmokeProbeStatus { kind, enabled })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn check_id_is_kind_prefixed_target() {
        let v = passed(SmokeProbeKind::Connection, "bigquery", 12);
        assert_eq!(v.check, "connection:bigquery");
        assert_eq!(v.target, "bigquery");
        assert_eq!(v.status, HealthStatus::Healthy);
        assert!(v.reason.is_none());
    }

    #[test]
    fn only_a_real_failure_is_unhealthy() {
        assert_eq!(
            failed(
                SmokeProbeKind::Semantic,
                "orders",
                "compile error".into(),
                5
            )
            .status,
            HealthStatus::Unhealthy
        );
        // A timeout is unknown, not wrong.
        assert_eq!(
            timed_out(
                SmokeProbeKind::App,
                "a.app.yml",
                Duration::from_secs(30),
                30_000
            )
            .status,
            HealthStatus::Degraded
        );
        // So is a probe we never got to run.
        assert_eq!(
            unavailable(SmokeProbeKind::Agent, "x", "no context".into()).status,
            HealthStatus::Degraded
        );
    }

    #[test]
    fn a_cap_note_is_healthy_but_carries_its_reason() {
        let v = note(
            SmokeProbeKind::Semantic,
            "topics",
            "skipped 5 of 30 topics (max_targets=25)".to_string(),
        );
        assert_eq!(v.status, HealthStatus::Healthy);
        assert!(v.reason.unwrap().contains("skipped 5 of 30"));
        assert_eq!(v.duration_ms, 0);
    }

    #[test]
    fn timeout_reason_names_the_budget() {
        let v = timed_out(
            SmokeProbeKind::Connection,
            "bq",
            Duration::from_secs(30),
            30_001,
        );
        assert_eq!(v.reason.unwrap(), "probe exceeded its 30s budget");
    }

    #[test]
    fn verdicts_round_trip_through_the_payload() {
        // The cadence gate reads the previous run's verdicts back out of the
        // stored JSON payload, so this round-trip is load-bearing.
        let original = vec![
            passed(SmokeProbeKind::Connection, "bigquery", 12),
            failed(SmokeProbeKind::Semantic, "orders", "boom".into(), 3),
        ];
        let json = serde_json::to_value(&original).unwrap();
        let back: Vec<SmokeVerdict> = serde_json::from_value(json).unwrap();
        assert_eq!(back, original);
        assert_eq!(back[1].kind, SmokeProbeKind::Semantic);
    }
}
