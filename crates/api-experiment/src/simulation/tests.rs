//! Cancellation, against the real loop and stub edges.
//!
//! The property under test is the one the module docstring claims as its reason
//! for being queued work at all: a cancelled run stops. It is assertable
//! without a warehouse or a database because [`observe_cancel`] and
//! [`run_end`] are the whole of the mechanism — the loop below is the shipped
//! `Runner`, driven over stub `RowSink`/`SemanticProbe` implementations the
//! same way `oxy-simulation`'s own tests drive it.

use std::sync::atomic::{AtomicBool, Ordering};

use oxy_simulation::{EdgeFit, EntityDay, Probe, RowSink, SemanticProbe, SimulationError};
use tokio_util::sync::CancellationToken;

use super::*;

const EDGE: &str = "marketing_spend -> net_sales";

/// A world small enough that the whole loop is a few milliseconds, and short
/// enough that "stopped early" is unambiguous.
fn spec(periods: u32) -> SimulationSpec {
    SimulationSpec::from_yaml(&format!(
        r#"
name: cancel
seed: 7
periods: {periods}
period_days: 7
history_days: 120
start_date: 2025-01-06
entities:
  count: 4
  scale_sigma: 0.4
baseline:
  sales_per_entity_day: 1500.0
  margin: 0.36
  demand_shock_rho: 0.0
  demand_shock_sd: 0.01
  weekly_seasonality: 0.15
mechanism:
  driver: marketing_spend
  target: net_sales
  lag_days: 7
  noise_ratio: 0.05
  calibrate:
    anchor_spend_share: 0.02
    local_slope_at_anchor: 4.0
    optimum_at: 3.0
"#
    ))
    .expect("spec")
}

struct Collector;

impl RowSink for Collector {
    fn append(&mut self, _rows: &[EntityDay]) -> Result<(), SimulationError> {
        Ok(())
    }
}

struct FixedProbe;

impl SemanticProbe for FixedProbe {
    fn probe(&mut self) -> Result<Probe, SimulationError> {
        Ok(Probe {
            fits: vec![(EDGE.to_string(), EdgeFit::fitted(4.0, 0.5))],
            impact_quantified: true,
        })
    }
}

/// The blocker this file exists for: the token used to be handed to the worker
/// and observed by nothing, so a transport cancel resolved instantly while the
/// run kept writing periods for minutes.
#[test]
fn cancel_stops_the_loop_between_periods() {
    let periods = 6;
    let cancel = CancellationToken::new();
    let cancelled = AtomicBool::new(false);
    let spec = spec(periods);
    let curve = spec.curve().expect("curve");
    let mut policy = policy::build(PolicyKind::Hold, &spec, curve);
    let mut sink = Collector;
    let mut probe = FixedProbe;
    let mut world = World::new(spec).expect("world");

    let mut seen: Vec<u32> = Vec::new();
    let result = Runner::new(&mut world, &mut *policy, &mut sink, &mut probe).run(|result| {
        // Same order as the shipped callback: observe first, so the period the
        // cancel lands on is never persisted.
        observe_cancel(&cancel, &cancelled)?;
        seen.push(result.period);
        if result.period == 2 {
            cancel.cancel();
        }
        Ok(())
    });

    // Stopped *between* periods: everything before the cancel is intact and
    // nothing after it ran.
    assert_eq!(seen, vec![1, 2], "loop ran past the cancel");
    assert!(
        result.is_err(),
        "a cancelled loop must not report a summary"
    );
    assert!(matches!(
        run_end(result, cancelled.load(Ordering::Relaxed)),
        Ok(RunEnd::Cancelled)
    ));
}

/// A run nobody cancelled still reports its summary — the guard costs the happy
/// path nothing.
#[test]
fn an_uncancelled_loop_runs_every_period() {
    let periods = 3;
    let cancel = CancellationToken::new();
    let cancelled = AtomicBool::new(false);
    let spec = spec(periods);
    let curve = spec.curve().expect("curve");
    let mut policy = policy::build(PolicyKind::Hold, &spec, curve);
    let mut sink = Collector;
    let mut probe = FixedProbe;
    let mut world = World::new(spec).expect("world");

    let mut seen: Vec<u32> = Vec::new();
    let result = Runner::new(&mut world, &mut *policy, &mut sink, &mut probe).run(|result| {
        observe_cancel(&cancel, &cancelled)?;
        seen.push(result.period);
        Ok(())
    });

    assert_eq!(seen, vec![1, 2, 3]);
    assert!(matches!(
        run_end(result, cancelled.load(Ordering::Relaxed)),
        Ok(RunEnd::Completed(_))
    ));
}

/// The flag, not the error, decides. A genuine engine failure that happens to
/// coincide with a cancel must still close the run out as failed — otherwise a
/// drifted world would be filed as an operator's decision.
#[test]
fn a_loop_error_without_the_flag_stays_a_failure() {
    let err = SimulationError::Drift("rows stopped carrying the mechanism".into());
    match run_end(Err(err), false) {
        Err(message) => assert!(message.contains("world drift")),
        Ok(_) => panic!("an unflagged loop error must not read as cancellation"),
    }
}

/// `observe_cancel` is the only writer of the flag, and it writes it exactly
/// when it stops the loop.
#[test]
fn observe_cancel_flags_only_on_cancellation() {
    let cancel = CancellationToken::new();
    let flag = AtomicBool::new(false);

    assert!(observe_cancel(&cancel, &flag).is_ok());
    assert!(!flag.load(Ordering::Relaxed));

    cancel.cancel();
    assert!(observe_cancel(&cancel, &flag).is_err());
    assert!(flag.load(Ordering::Relaxed));
}

/// A claim write that **fails** must close the run out, not walk away from it.
///
/// `queue_run` wrote the row synchronously at enqueue, so the run exists at
/// `queued` before any worker sees it. Returning `Failed` from the executor
/// stamps the *queue* row terminal (`durable`'s `Outcome` arm →
/// `fail_queue_task`), so nothing is left alive to move the run row: it sat at
/// `queued`, invisible to the fleet, counted against
/// `MAX_IN_FLIGHT_PER_WORKSPACE` and excluded from any reader scoped to
/// terminal runs, until `IN_FLIGHT_MAX_AGE_HOURS` aged it out. The bad-spec arm
/// immediately above the claim already knew this and called `fail_run`; the
/// claim arm did not.
#[test]
fn a_failed_claim_closes_the_run_out() {
    let err = sea_orm::DbErr::Custom("connection pool timed out".to_string());
    match on_claim(Err(err)) {
        ClaimStep::Close(message) => assert!(
            message.contains("connection pool timed out"),
            "the run's `error` has to name why the claim failed: {message}"
        ),
        other => panic!("a failed claim must close the run out, got {other:?}"),
    }
}

/// A claim that moved no row means the run is already terminal — a requeued
/// attempt of a run that finished. The attempt stops, and crucially writes
/// *nothing*: closing it out here would overwrite evidence the first attempt
/// already recorded.
#[test]
fn a_claim_that_moved_nothing_leaves_the_run_alone() {
    assert!(matches!(
        on_claim(Ok(store::Claim::AlreadyClosed)),
        ClaimStep::Skip
    ));
}

/// The ordinary claim, and the requeue of a run that really was still running.
/// The guard costs the happy path nothing.
#[test]
fn a_claim_that_moved_the_row_runs() {
    assert!(matches!(
        on_claim(Ok(store::Claim::Claimed)),
        ClaimStep::Run
    ));
}
