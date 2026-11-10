//! Pure drift comparison: two numbers + a tolerance → a `DriftVerdict`.
//! No I/O, no DB — mirrors the `evaluator` purity split so it is unit-testable.

use serde::{Deserialize, Serialize};

use super::ResolvedWindow;
use super::segments::SegmentReport;
use crate::evaluator::HealthStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Combinator {
    And,
    Or,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tolerance {
    pub abs: f64,
    pub pct: f64,
    #[serde(default = "default_combinator")]
    pub combinator: Combinator,
}

fn default_combinator() -> Combinator {
    Combinator::And
}

/// Resolved presentation fields for a check, threaded through every verdict
/// constructor so the comparison and the degraded paths echo the same labels.
/// Labels are already defaulted by the caller ("Actual" / "Expected").
#[derive(Debug, Clone)]
pub struct VerdictMeta {
    pub check: String,
    pub description: Option<String>,
    pub actual_label: String,
    pub expected_label: String,
    /// The window BOTH operands were queried over, already resolved through
    /// `freshness` / `timezone` / `offset`. Carried onto every verdict —
    /// including the degraded ones — because "which period did this compare?"
    /// is the first question a drift number raises, and without it a `freshness`
    /// watermark is invisible from the health payload.
    pub window: ResolvedWindow,
}

/// One reconciliation outcome, persisted and surfaced in the health payload.
#[derive(Debug, Clone, Serialize)]
pub struct DriftVerdict {
    /// Stable machine id.
    pub check: String,
    /// Friendly text echoed from the check (if any).
    pub description: Option<String>,
    /// Resolved actual-side label ("Actual" when unset).
    pub actual_label: String,
    /// Resolved expected-side label ("Expected" when unset).
    pub expected_label: String,
    pub actual: f64,
    pub expected: f64,
    pub abs_diff: f64,
    /// Percent drift relative to the `expected` (reference) value, in percent
    /// units (3.0 == 3%). `0.0` when both values are zero.
    pub pct_diff: f64,
    pub status: HealthStatus,
    pub reason: Option<String>,
    /// Inclusive first date of the compared window (`%Y-%m-%d`).
    pub window_start: String,
    /// Inclusive last date of the compared window (`%Y-%m-%d`).
    pub window_end: String,
    /// IANA calendar the two dates were resolved on (`"UTC"` when unset).
    pub window_timezone: String,
    /// Per-segment detail for a check that declares `group_by`: which groups
    /// are outside tolerance. Detail only — `status` and the totals above are
    /// computed exactly as for an ungrouped check. Absent (and not serialized)
    /// without `group_by`, so an ungrouped verdict is unchanged on the wire.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segments: Option<SegmentReport>,
}

/// Compare the `actual` number against the `expected` (reference) number.
/// `pct_unhealthy` is the hard cutoff (percent units) above which any breach
/// escalates from Degraded to Unhealthy. Drift is relative to `expected`.
pub fn compare(
    meta: &VerdictMeta,
    actual: f64,
    expected: f64,
    tol: &Tolerance,
    pct_unhealthy: f64,
) -> DriftVerdict {
    let (abs_diff, pct_diff, breached) = breach(actual, expected, tol);

    let pct_stored = if pct_diff.is_finite() { pct_diff } else { 0.0 };

    if !breached {
        return verdict(
            meta,
            actual,
            expected,
            abs_diff,
            pct_stored,
            HealthStatus::Healthy,
            None,
        );
    }

    let over_cutoff = (pct_diff.is_finite() && pct_diff > pct_unhealthy) || pct_diff.is_infinite();
    let status = if over_cutoff {
        HealthStatus::Unhealthy
    } else {
        HealthStatus::Degraded
    };
    let pct_display = if pct_diff.is_finite() {
        format!("{pct_diff:.1}%")
    } else {
        "∞%".to_string()
    };
    let subject = meta.description.as_deref().unwrap_or(&meta.check);
    let reason = Some(format!(
        "{subject} drifts {pct_display} from {expected_label} ({actual_label} {actual:.2} vs \
         {expected_label} {expected:.2})",
        expected_label = meta.expected_label,
        actual_label = meta.actual_label,
    ));
    verdict(meta, actual, expected, abs_diff, pct_stored, status, reason)
}

/// `(abs_diff, pct_diff, breached)` for one pair of numbers under `tol`. The
/// single definition of "outside tolerance", shared by the check total and
/// every `group_by` segment so the two cannot disagree on what a breach is.
pub(super) fn breach(actual: f64, expected: f64, tol: &Tolerance) -> (f64, f64, bool) {
    let abs_diff = (actual - expected).abs();
    // pct relative to the reference `expected` value. expected == 0 → undefined;
    // report 0.0 when both are zero, else fall back to the abs arm only
    // (INFINITY, which compares as breached).
    let pct_diff = if expected == 0.0 {
        if actual == 0.0 { 0.0 } else { f64::INFINITY }
    } else {
        (abs_diff / expected.abs()) * 100.0
    };

    let abs_breached = abs_diff > tol.abs;
    let pct_breached = pct_diff > tol.pct; // INFINITY > tol.pct is true
    let breached = match tol.combinator {
        Combinator::And => abs_breached && pct_breached,
        Combinator::Or => abs_breached || pct_breached,
    };
    (abs_diff, pct_diff, breached)
}

/// Assemble a `DriftVerdict`, echoing the resolved presentation fields.
fn verdict(
    meta: &VerdictMeta,
    actual: f64,
    expected: f64,
    abs_diff: f64,
    pct_diff: f64,
    status: HealthStatus,
    reason: Option<String>,
) -> DriftVerdict {
    DriftVerdict {
        check: meta.check.clone(),
        description: meta.description.clone(),
        actual_label: meta.actual_label.clone(),
        expected_label: meta.expected_label.clone(),
        actual,
        expected,
        abs_diff,
        pct_diff,
        status,
        reason,
        window_start: meta.window.dates[0].clone(),
        window_end: meta.window.dates[1].clone(),
        window_timezone: meta.window.timezone.clone(),
        segments: None,
    }
}

/// Verdict for an external source that could not be reached / timed out. An
/// unreachable source is *unknown*, not *wrong* — Degraded, never Unhealthy.
pub fn unreachable_verdict(meta: &VerdictMeta, source: &str) -> DriftVerdict {
    degraded(meta, format!("{source} unreachable"))
}

/// Verdict for a check that errored before comparison (bad measure, missing
/// secret, unknown source). Degraded with the supplied reason.
pub fn error_verdict(meta: &VerdictMeta, reason: String) -> DriftVerdict {
    degraded(meta, reason)
}

/// A Degraded verdict with NaN values (no comparison happened).
fn degraded(meta: &VerdictMeta, reason: String) -> DriftVerdict {
    verdict(
        meta,
        f64::NAN,
        f64::NAN,
        f64::NAN,
        0.0,
        HealthStatus::Degraded,
        Some(reason),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluator::HealthStatus;

    fn tol(abs: f64, pct: f64, c: Combinator) -> Tolerance {
        Tolerance {
            abs,
            pct,
            combinator: c,
        }
    }

    fn window() -> ResolvedWindow {
        ResolvedWindow {
            dates: ["2026-07-12".to_string(), "2026-07-18".to_string()],
            timezone: "UTC".to_string(),
        }
    }

    fn meta(check: &str) -> VerdictMeta {
        VerdictMeta {
            check: check.to_string(),
            description: None,
            actual_label: "Actual".to_string(),
            expected_label: "Expected".to_string(),
            window: window(),
        }
    }

    #[test]
    fn within_tolerance_is_healthy() {
        let v = compare(
            &meta("net_sales"),
            1000.4,
            1000.0,
            &tol(1.0, 0.5, Combinator::And),
            5.0,
        );
        assert_eq!(v.status, HealthStatus::Healthy);
        assert!(v.reason.is_none());
        assert_eq!(v.actual, 1000.4);
        assert_eq!(v.expected, 1000.0);
        assert_eq!(v.actual_label, "Actual");
        assert_eq!(v.expected_label, "Expected");
    }

    #[test]
    fn and_combinator_needs_both_breached() {
        // abs_diff = 3.0 (> 1.0) but pct_diff = 0.3% (< 0.5%): AND not satisfied → healthy.
        let v = compare(
            &meta("m"),
            1003.0,
            1000.0,
            &tol(1.0, 0.5, Combinator::And),
            5.0,
        );
        assert_eq!(v.status, HealthStatus::Healthy);
    }

    #[test]
    fn and_combinator_both_breached_is_degraded() {
        // abs_diff = 30 (> 1.0) and pct_diff = 3% (> 0.5%, < 5% cutoff) → degraded.
        let v = compare(
            &meta("m"),
            1030.0,
            1000.0,
            &tol(1.0, 0.5, Combinator::And),
            5.0,
        );
        assert_eq!(v.status, HealthStatus::Degraded);
        assert!(v.reason.as_ref().unwrap().contains("3.0%"));
    }

    #[test]
    fn every_verdict_echoes_the_compared_window() {
        // The window is what makes a `freshness` watermark visible: without it
        // a drift number can't be told apart from one measured over a period
        // the warehouse hasn't finished loading. It must survive the healthy,
        // breached, AND degraded paths.
        let healthy = compare(
            &meta("m"),
            100.0,
            100.0,
            &tol(1.0, 0.5, Combinator::And),
            5.0,
        );
        let breached = compare(
            &meta("m"),
            200.0,
            100.0,
            &tol(1.0, 0.5, Combinator::And),
            5.0,
        );
        let degraded = unreachable_verdict(&meta("m"), "toast");
        for v in [healthy, breached, degraded] {
            assert_eq!(v.window_start, "2026-07-12");
            assert_eq!(v.window_end, "2026-07-18");
            assert_eq!(v.window_timezone, "UTC");
        }
    }

    #[test]
    fn reason_uses_resolved_labels_and_description() {
        let m = VerdictMeta {
            check: "net_sales_vs_toast".to_string(),
            description: Some("Daily net sales".to_string()),
            actual_label: "Oxy net sales".to_string(),
            expected_label: "Toast net sales".to_string(),
            window: window(),
        };
        let v = compare(&m, 1030.0, 1000.0, &tol(1.0, 0.5, Combinator::And), 5.0);
        let reason = v.reason.unwrap();
        assert!(reason.starts_with("Daily net sales drifts"));
        assert!(reason.contains("from Toast net sales"));
        assert!(reason.contains("Oxy net sales 1030.00"));
        assert!(reason.contains("Toast net sales 1000.00"));
    }

    #[test]
    fn or_combinator_one_breached_is_degraded() {
        // abs_diff = 3.0 (> 1.0), pct_diff = 0.3% (< 0.5%): OR satisfied by abs → degraded.
        let v = compare(
            &meta("m"),
            1003.0,
            1000.0,
            &tol(1.0, 0.5, Combinator::Or),
            5.0,
        );
        assert_eq!(v.status, HealthStatus::Degraded);
    }

    #[test]
    fn breach_over_hard_cutoff_is_unhealthy() {
        // pct_diff = 10% > 5% cutoff → unhealthy regardless of combinator.
        let v = compare(
            &meta("m"),
            1100.0,
            1000.0,
            &tol(1.0, 0.5, Combinator::And),
            5.0,
        );
        assert_eq!(v.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn zero_expected_with_nonzero_actual_breaches_via_abs() {
        // expected == 0: pct is undefined (∞); abs arm breaches and ∞ escalates.
        let v = compare(&meta("m"), 50.0, 0.0, &tol(1.0, 0.5, Combinator::Or), 5.0);
        assert_eq!(v.status, HealthStatus::Unhealthy);
        assert!(v.pct_diff.is_finite());
    }

    #[test]
    fn both_zero_is_healthy() {
        let v = compare(&meta("m"), 0.0, 0.0, &tol(1.0, 0.5, Combinator::And), 5.0);
        assert_eq!(v.status, HealthStatus::Healthy);
        assert_eq!(v.pct_diff, 0.0);
    }

    #[test]
    fn unreachable_is_degraded_with_reason() {
        let v = unreachable_verdict(&meta("m"), "toast");
        assert_eq!(v.status, HealthStatus::Degraded);
        assert!(v.reason.unwrap().contains("toast unreachable"));
        assert!(v.actual.is_nan());
        assert!(v.expected.is_nan());
    }
}
