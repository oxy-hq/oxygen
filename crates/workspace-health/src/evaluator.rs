use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::app_availability::AppAvailabilityVerdict;
use crate::reconcile::DriftVerdict;
use crate::smoke::SmokeVerdict;

/// Workspace status. Declaration order matters: `Ord` makes
/// `Unhealthy > Degraded > Healthy`, so the worst dimension is `.max()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

impl HealthStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            HealthStatus::Healthy => "healthy",
            HealthStatus::Degraded => "degraded",
            HealthStatus::Unhealthy => "unhealthy",
        }
    }
}

/// Declaration order is the display/compare order — `Ord` exists so a failure set
/// can be stored sorted and diffed positionally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthDimension {
    JobLiveness,
    Pipeline,
    Queue,
    Reconciliation,
    SmokeTest,
    CustomAppAvailability,
}

/// Raw per-workspace signal counts gathered from Postgres. Pure input —
/// no DB handle, no I/O. Window-bounded counts are computed by the query layer.
#[derive(Debug, Clone)]
pub struct WorkspaceSignals {
    pub workspace_id: Uuid,
    pub failed_runs: i64,
    pub timed_out_runs: i64,
    pub total_runs: i64,
    pub airway_last_run_failed: bool,
    pub airway_completed_with_errors: bool,
    /// Open anomaly counts. Reported on the health panel but deliberately NOT
    /// evaluated into any dimension: severity measures distance past the
    /// seasonal envelope (`severity_from_headroom`), so a campaign, a holiday or
    /// a new location scores High while nothing is broken. Correctness is
    /// `reconcile.yml`'s job — it compares a measure against the live external
    /// source, which is a defect signal rather than a business observation.
    pub open_high_anomalies: i64,
    pub open_medium_anomalies: i64,
    pub dead_letter_count: i64,
    pub reconciliation: Vec<DriftVerdict>,
    /// Smoke-probe verdicts. Unlike the other signals these are not gathered
    /// every pass — the smoke test runs on its own slower cadence, and passes in
    /// between reuse the previous run's verdicts (see `eval_pass`). Empty when no
    /// smoke test is configured.
    pub smoke: Vec<SmokeVerdict>,
    /// Per-app availability verdicts, derived from the wide-event stream rather
    /// than from a probe. Like `smoke`, empty means "no signal" and reads clear —
    /// which is what an unconfigured observability backend produces.
    pub custom_apps: Vec<AppAvailabilityVerdict>,
}

impl WorkspaceSignals {
    /// A zeroed signal set for `workspace_id` — no runs, no anomalies, no
    /// reconciliation. Evaluates to Healthy. Used when a workspace has no
    /// activity in the window so the single-workspace eval still persists a row.
    pub fn empty(workspace_id: Uuid) -> Self {
        Self {
            workspace_id,
            failed_runs: 0,
            timed_out_runs: 0,
            total_runs: 0,
            airway_last_run_failed: false,
            airway_completed_with_errors: false,
            open_high_anomalies: 0,
            open_medium_anomalies: 0,
            dead_letter_count: 0,
            reconciliation: Vec::new(),
            smoke: Vec::new(),
            custom_apps: Vec::new(),
        }
    }
}

/// Tunable cutoffs. Env-overridable so ops can retune without a redeploy.
#[derive(Debug, Clone)]
pub struct HealthThresholds {
    pub window_hours: i64,
    pub job_failure_rate_unhealthy: f64,
    pub job_failure_rate_degraded: f64,
    pub min_runs_for_rate: i64,
}

impl Default for HealthThresholds {
    fn default() -> Self {
        Self {
            window_hours: 24,
            job_failure_rate_unhealthy: 0.5,
            job_failure_rate_degraded: 0.2,
            min_runs_for_rate: 5,
        }
    }
}

impl HealthThresholds {
    /// Read overrides from env, falling back to `Default`. Invalid values are
    /// ignored (kept at default) rather than failing the eval pass. Parsed values
    /// are then clamped to sane ranges so a hostile/typo'd override can't silently
    /// invert the signal: a non-positive `window_hours` would make
    /// `now() - make_interval(hours => $1)` resolve to a future timestamp, so
    /// `created_at > <future>` matches nothing and every workspace reads Healthy
    /// with zero signals — a false OK. Rates must stay a valid `[0.0, 1.0]`
    /// fraction, and `min_runs_for_rate` must be at least 1.
    pub fn from_env() -> Self {
        let d = Self::default();
        Self {
            window_hours: env_i64("OXY_HEALTH_WINDOW_HOURS", d.window_hours).max(1),
            job_failure_rate_unhealthy: env_f64(
                "OXY_HEALTH_FAIL_RATE_UNHEALTHY",
                d.job_failure_rate_unhealthy,
            )
            .clamp(0.0, 1.0),
            job_failure_rate_degraded: env_f64(
                "OXY_HEALTH_FAIL_RATE_DEGRADED",
                d.job_failure_rate_degraded,
            )
            .clamp(0.0, 1.0),
            min_runs_for_rate: env_i64("OXY_HEALTH_MIN_RUNS", d.min_runs_for_rate).max(1),
        }
    }
}

fn env_i64(key: &str, fallback: i64) -> i64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(fallback)
}

fn env_f64(key: &str, fallback: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(fallback)
}

#[derive(Debug, Clone, Serialize)]
pub struct DimensionResult {
    pub dimension: HealthDimension,
    pub status: HealthStatus,
    pub reason: Option<String>,
}

/// One failing dimension, reduced to the two things that identify the failure:
/// *what* is broken and *how badly*.
///
/// This is deliberately not the reason string. Reason text embeds live counts and
/// percentages (`"4/12 runs failed in window (33%)"`, `"3 dead-letter task(s)"`)
/// that move between passes for a workspace that is continuously broken on the
/// same dimension, so comparing text reads normal churn as a brand-new failure.
/// Alerting diffs *this* instead — it only changes when a dimension starts
/// failing or gets worse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DimensionFailure {
    pub dimension: HealthDimension,
    pub status: HealthStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceHealth {
    pub workspace_id: Uuid,
    pub status: HealthStatus,
    pub dimensions: Vec<DimensionResult>,
    pub reasons: Vec<String>,
}

impl WorkspaceHealth {
    /// The currently-failing dimensions, sorted by dimension so two evaluations
    /// of the same failure set always compare equal.
    pub fn failures(&self) -> Vec<DimensionFailure> {
        let mut out: Vec<DimensionFailure> = self
            .dimensions
            .iter()
            .filter(|d| d.status != HealthStatus::Healthy)
            .map(|d| DimensionFailure {
                dimension: d.dimension,
                status: d.status,
            })
            .collect();
        out.sort_unstable_by_key(|f| f.dimension);
        out
    }
}

/// Pure rollup: evaluate each dimension, take the worst as the workspace status.
pub fn evaluate(s: &WorkspaceSignals, t: &HealthThresholds) -> WorkspaceHealth {
    let dimensions = vec![
        eval_job_liveness(s, t),
        eval_pipeline(s),
        eval_queue(s),
        eval_reconciliation(s),
        eval_smoke_test(s),
        eval_custom_app_availability(s),
    ];
    let status = dimensions
        .iter()
        .map(|d| d.status)
        .max()
        .unwrap_or(HealthStatus::Healthy);
    let reasons = dimensions
        .iter()
        .filter(|d| d.status != HealthStatus::Healthy)
        .filter_map(|d| d.reason.clone())
        .collect();
    WorkspaceHealth {
        workspace_id: s.workspace_id,
        status,
        dimensions,
        reasons,
    }
}

fn eval_job_liveness(s: &WorkspaceSignals, t: &HealthThresholds) -> DimensionResult {
    let failures = s.failed_runs + s.timed_out_runs;
    if s.total_runs < t.min_runs_for_rate || failures == 0 {
        return clear(HealthDimension::JobLiveness);
    }
    let rate = failures as f64 / s.total_runs as f64;
    if rate >= t.job_failure_rate_unhealthy {
        unhealthy(
            HealthDimension::JobLiveness,
            format!(
                "{failures}/{} runs failed in window ({:.0}%)",
                s.total_runs,
                rate * 100.0
            ),
        )
    } else if rate >= t.job_failure_rate_degraded {
        degraded(
            HealthDimension::JobLiveness,
            format!("elevated run failure rate ({:.0}%)", rate * 100.0),
        )
    } else {
        clear(HealthDimension::JobLiveness)
    }
}

fn eval_pipeline(s: &WorkspaceSignals) -> DimensionResult {
    if s.airway_last_run_failed {
        unhealthy(HealthDimension::Pipeline, "latest Airway run failed".into())
    } else if s.airway_completed_with_errors {
        unhealthy(
            HealthDimension::Pipeline,
            "latest Airway run completed with errors".into(),
        )
    } else {
        clear(HealthDimension::Pipeline)
    }
}

fn eval_queue(s: &WorkspaceSignals) -> DimensionResult {
    if s.dead_letter_count > 0 {
        unhealthy(
            HealthDimension::Queue,
            format!("{} dead-letter task(s)", s.dead_letter_count),
        )
    } else {
        clear(HealthDimension::Queue)
    }
}

/// Worst reconciliation verdict drives the dimension. Empty (no `reconcile.yml`)
/// reads clear, exactly like the other dimensions with no signals.
fn eval_reconciliation(s: &WorkspaceSignals) -> DimensionResult {
    let worst = s.reconciliation.iter().max_by_key(|v| v.status);
    match worst {
        None => clear(HealthDimension::Reconciliation),
        Some(v) if v.status == HealthStatus::Healthy => clear(HealthDimension::Reconciliation),
        Some(v) => DimensionResult {
            dimension: HealthDimension::Reconciliation,
            status: v.status,
            reason: v.reason.clone(),
        },
    }
}

/// Worst smoke verdict drives the dimension. Empty (no smoke test configured, or
/// it is disabled) reads clear, exactly like the other dimensions with no
/// signals. Healthy verdicts that carry a reason — the `max_targets` cap notes —
/// are informational and never move the dimension.
///
/// When more than one probe is unhappy the reason names the worst one and counts
/// the rest, so a workspace with eight broken topics doesn't hide seven of them
/// behind a single line.
fn eval_smoke_test(s: &WorkspaceSignals) -> DimensionResult {
    let Some(worst) = s.smoke.iter().max_by_key(|v| v.status) else {
        return clear(HealthDimension::SmokeTest);
    };
    if worst.status == HealthStatus::Healthy {
        return clear(HealthDimension::SmokeTest);
    }
    let failing = s
        .smoke
        .iter()
        .filter(|v| v.status != HealthStatus::Healthy)
        .count();
    let base = worst
        .reason
        .clone()
        .unwrap_or_else(|| "probe failed".to_string());
    let reason = match failing {
        1 => format!("{}: {base}", worst.check),
        n => format!("{}: {base} (+{} more failing probe(s))", worst.check, n - 1),
    };
    DimensionResult {
        dimension: HealthDimension::SmokeTest,
        status: worst.status,
        reason: Some(reason),
    }
}

/// Worst app drives the dimension, exactly like the smoke and reconciliation
/// verdict lists. Empty (observability off, or no published apps) reads clear.
///
/// The severity mapping — which burn grade becomes Unhealthy and which becomes
/// Degraded — lives in `app_availability::status_for`, because it is the alerting
/// policy rather than a roll-up detail.
fn eval_custom_app_availability(s: &WorkspaceSignals) -> DimensionResult {
    let Some(worst) = s.custom_apps.iter().max_by_key(|v| v.status) else {
        return clear(HealthDimension::CustomAppAvailability);
    };
    if worst.status == HealthStatus::Healthy {
        return clear(HealthDimension::CustomAppAvailability);
    }
    let failing = s
        .custom_apps
        .iter()
        .filter(|v| v.status != HealthStatus::Healthy)
        .count();
    let base = worst
        .reason
        .clone()
        .unwrap_or_else(|| format!("{} is unavailable", worst.app_slug));
    let reason = match failing {
        1 => base,
        n => format!("{base} (+{} more app(s) affected)", n - 1),
    };
    DimensionResult {
        dimension: HealthDimension::CustomAppAvailability,
        status: worst.status,
        reason: Some(reason),
    }
}

fn clear(dimension: HealthDimension) -> DimensionResult {
    DimensionResult {
        dimension,
        status: HealthStatus::Healthy,
        reason: None,
    }
}

fn degraded(dimension: HealthDimension, reason: String) -> DimensionResult {
    DimensionResult {
        dimension,
        status: HealthStatus::Degraded,
        reason: Some(reason),
    }
}

fn unhealthy(dimension: HealthDimension, reason: String) -> DimensionResult {
    DimensionResult {
        dimension,
        status: HealthStatus::Unhealthy,
        reason: Some(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn base() -> WorkspaceSignals {
        WorkspaceSignals {
            workspace_id: Uuid::nil(),
            failed_runs: 0,
            timed_out_runs: 0,
            total_runs: 0,
            airway_last_run_failed: false,
            airway_completed_with_errors: false,
            open_high_anomalies: 0,
            open_medium_anomalies: 0,
            dead_letter_count: 0,
            reconciliation: Vec::new(),
            smoke: Vec::new(),
            custom_apps: Vec::new(),
        }
    }

    #[test]
    fn all_clear_is_healthy() {
        let h = evaluate(&base(), &HealthThresholds::default());
        assert_eq!(h.status, HealthStatus::Healthy);
        assert!(h.reasons.is_empty());
    }

    #[test]
    fn from_env_clamps_hostile_overrides() {
        // nextest runs each test in its own process, so these env mutations are
        // isolated. A non-positive window or out-of-range rate must clamp rather
        // than invert the signal (a future `created_at >` cutoff → false Healthy).
        // SAFETY: single-threaded test process, no concurrent env access.
        unsafe {
            std::env::set_var("OXY_HEALTH_WINDOW_HOURS", "0");
            std::env::set_var("OXY_HEALTH_FAIL_RATE_UNHEALTHY", "5.0");
            std::env::set_var("OXY_HEALTH_FAIL_RATE_DEGRADED", "-1.0");
            std::env::set_var("OXY_HEALTH_MIN_RUNS", "0");
        }
        let t = HealthThresholds::from_env();
        assert_eq!(t.window_hours, 1);
        assert_eq!(t.job_failure_rate_unhealthy, 1.0);
        assert_eq!(t.job_failure_rate_degraded, 0.0);
        assert_eq!(t.min_runs_for_rate, 1);
    }

    /// Anomalies are a business observation, not a defect signal: severity
    /// measures distance past the seasonal envelope, so a marketing push or a
    /// holiday scores High while the workspace is working perfectly. They are
    /// reported as signal counts and deliberately do not vote on the verdict —
    /// `reconcile.yml` is the correctness check that does.
    #[test]
    fn anomalies_do_not_move_the_verdict() {
        let mut s = base();
        s.open_high_anomalies = 7;
        s.open_medium_anomalies = 3;
        let h = evaluate(&s, &HealthThresholds::default());
        assert_eq!(h.status, HealthStatus::Healthy);
        assert!(h.reasons.is_empty());
        assert!(
            !h.dimensions.iter().any(|d| format!("{:?}", d.dimension)
                .to_lowercase()
                .contains("correct")),
            "the correctness dimension must be gone, not merely silent"
        );
    }

    #[test]
    fn airway_errors_are_unhealthy() {
        let mut s = base();
        s.airway_completed_with_errors = true;
        assert_eq!(
            evaluate(&s, &HealthThresholds::default()).status,
            HealthStatus::Unhealthy
        );
    }

    #[test]
    fn dead_letter_is_unhealthy() {
        let mut s = base();
        s.dead_letter_count = 2;
        assert_eq!(
            evaluate(&s, &HealthThresholds::default()).status,
            HealthStatus::Unhealthy
        );
    }

    #[test]
    fn high_failure_rate_is_unhealthy() {
        let mut s = base();
        s.total_runs = 10;
        s.failed_runs = 6; // 0.6 > 0.5 default unhealthy cutoff
        assert_eq!(
            evaluate(&s, &HealthThresholds::default()).status,
            HealthStatus::Unhealthy
        );
    }

    #[test]
    fn moderate_failure_rate_is_degraded() {
        let mut s = base();
        s.total_runs = 10;
        s.failed_runs = 3; // 0.3: between degraded(0.2) and unhealthy(0.5)
        assert_eq!(
            evaluate(&s, &HealthThresholds::default()).status,
            HealthStatus::Degraded
        );
    }

    #[test]
    fn small_sample_does_not_flag_rate() {
        let mut s = base();
        s.total_runs = 2; // below min_runs_for_rate (5)
        s.failed_runs = 2;
        assert_eq!(
            evaluate(&s, &HealthThresholds::default()).status,
            HealthStatus::Healthy
        );
    }

    #[test]
    fn reconciliation_within_tolerance_is_healthy() {
        let h = evaluate(&base(), &HealthThresholds::default());
        let dim = h
            .dimensions
            .iter()
            .find(|d| d.dimension == HealthDimension::Reconciliation)
            .unwrap();
        assert_eq!(dim.status, HealthStatus::Healthy);
    }

    #[test]
    fn reconciliation_worst_verdict_drives_dimension() {
        let mut s = base();
        s.reconciliation = vec![
            DriftVerdict {
                check: "a".into(),
                description: None,
                actual_label: "Actual".into(),
                expected_label: "Expected".into(),
                actual: 1.0,
                expected: 1.0,
                abs_diff: 0.0,
                pct_diff: 0.0,
                status: HealthStatus::Healthy,
                reason: None,
                window_start: "2026-07-12".into(),
                window_end: "2026-07-18".into(),
                window_timezone: "UTC".into(),
                segments: None,
            },
            DriftVerdict {
                check: "b".into(),
                description: None,
                actual_label: "Actual".into(),
                expected_label: "Expected".into(),
                actual: 110.0,
                expected: 100.0,
                abs_diff: 10.0,
                pct_diff: 10.0,
                status: HealthStatus::Unhealthy,
                reason: Some("b drifts 10.0% from source".into()),
                window_start: "2026-07-12".into(),
                window_end: "2026-07-18".into(),
                window_timezone: "UTC".into(),
                segments: None,
            },
        ];
        let h = evaluate(&s, &HealthThresholds::default());
        assert_eq!(h.status, HealthStatus::Unhealthy);
        assert!(h.reasons.iter().any(|r| r.contains("drifts 10.0%")));
    }

    #[test]
    fn no_smoke_test_configured_reads_clear() {
        let h = evaluate(&base(), &HealthThresholds::default());
        let dim = h
            .dimensions
            .iter()
            .find(|d| d.dimension == HealthDimension::SmokeTest)
            .expect("the smoke dimension is always present");
        assert_eq!(dim.status, HealthStatus::Healthy);
        assert!(dim.reason.is_none());
    }

    #[test]
    fn a_broken_probe_makes_the_workspace_unhealthy() {
        use crate::smoke::{SmokeProbeKind, failed, passed};
        let mut s = base();
        s.smoke = vec![
            passed(SmokeProbeKind::Connection, "bigquery", 12),
            failed(
                SmokeProbeKind::Semantic,
                "orders",
                "measure 'orders.net' failed: column not found".into(),
                40,
            ),
        ];
        let h = evaluate(&s, &HealthThresholds::default());
        assert_eq!(h.status, HealthStatus::Unhealthy);
        assert!(h.reasons.iter().any(|r| r.contains("semantic:orders")));
        assert!(h.reasons.iter().any(|r| r.contains("column not found")));
    }

    #[test]
    fn a_timed_out_probe_is_degraded_not_unhealthy() {
        use crate::smoke::{SmokeProbeKind, timed_out};
        let mut s = base();
        s.smoke = vec![timed_out(
            SmokeProbeKind::Connection,
            "snowflake",
            std::time::Duration::from_secs(30),
            30_000,
        )];
        assert_eq!(
            evaluate(&s, &HealthThresholds::default()).status,
            HealthStatus::Degraded
        );
    }

    #[test]
    fn cap_notes_are_healthy_and_never_move_the_dimension() {
        // A workspace with more topics than `max_targets` is large, not sick.
        use crate::smoke::{SmokeProbeKind, note, passed};
        let mut s = base();
        s.smoke = vec![
            passed(SmokeProbeKind::Semantic, "orders", 30),
            note(
                SmokeProbeKind::Semantic,
                "topics",
                "probed 25 of 30 topics; skipped 5 (max_targets=25)".into(),
            ),
        ];
        let h = evaluate(&s, &HealthThresholds::default());
        assert_eq!(h.status, HealthStatus::Healthy);
        assert!(h.reasons.is_empty());
    }

    #[test]
    fn worst_probe_leads_and_the_rest_are_counted() {
        use crate::smoke::{SmokeProbeKind, failed, timed_out};
        let mut s = base();
        s.smoke = vec![
            timed_out(
                SmokeProbeKind::App,
                "a.app.yml",
                std::time::Duration::from_secs(30),
                30_000,
            ),
            failed(SmokeProbeKind::Agent, "analytics", "no LLM key".into(), 5),
            failed(SmokeProbeKind::Semantic, "orders", "boom".into(), 5),
        ];
        let h = evaluate(&s, &HealthThresholds::default());
        assert_eq!(h.status, HealthStatus::Unhealthy);
        let reason = h
            .reasons
            .iter()
            .find(|r| r.contains("+2 more"))
            .expect("the other two failing probes must be counted, not hidden");
        // Worst-first: an Unhealthy probe leads, not the Degraded timeout.
        assert!(reason.starts_with("agent:") || reason.starts_with("semantic:"));
    }

    #[test]
    fn empty_signals_are_healthy() {
        let ws = Uuid::new_v4();
        let s = WorkspaceSignals::empty(ws);
        assert_eq!(s.workspace_id, ws);
        assert_eq!(s.total_runs, 0);
        assert!(s.reconciliation.is_empty());
        assert!(s.smoke.is_empty());
        assert_eq!(
            evaluate(&s, &HealthThresholds::default()).status,
            HealthStatus::Healthy
        );
    }

    #[test]
    fn worst_dimension_wins() {
        let mut s = base();
        s.total_runs = 10; // 3/10 = 30% -> degraded on job liveness
        s.failed_runs = 3;
        s.dead_letter_count = 1; // unhealthy on queue
        assert_eq!(
            evaluate(&s, &HealthThresholds::default()).status,
            HealthStatus::Unhealthy
        );
    }
}
