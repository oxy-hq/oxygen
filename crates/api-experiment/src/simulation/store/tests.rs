//! The shape of the per-period writes.
//!
//! Statement-shape rather than round-trip: there is no database-backed harness
//! reachable from `--lib` tests (`crates/app/tests/common::fresh_db` is an
//! integration-only fixture), and the property at issue is entirely in the SQL
//! — whether the retry of a requeued run rewrites its rows or dies on the
//! primary key. Built the same way `api::thread`'s tests pin their filters.

use oxy_simulation::{EdgeFit, FitScore, Outcome, PeriodResult, SimulationSpec};
use sea_orm::{DatabaseBackend, QueryTrait};

use super::*;

/// A world small enough to parse; nothing here runs it.
fn spec() -> SimulationSpec {
    SimulationSpec::from_yaml(
        r#"
name: stamped
seed: 7
periods: 3
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
"#,
    )
    .expect("spec")
}

fn new_run() -> NewRun {
    NewRun {
        run_id: Uuid::nil(),
        workspace_id: Uuid::nil(),
        revision_id: None,
        policy: PolicyKind::Hold,
        replicate: 0,
    }
}

/// Enqueue stamps both clocks. `started_at` has to be written because the
/// column is NOT NULL for the listing index; `queued_at` is the one that
/// means what it says.
#[test]
fn queueing_stamps_queued_at_and_started_at_together() {
    let sql = queued_insert(new_run(), &spec())
        .build(DatabaseBackend::Postgres)
        .sql;
    assert!(sql.contains("\"queued_at\""), "{sql}");
    assert!(sql.contains("\"started_at\""), "{sql}");
    assert!(sql.contains("\"status\""), "{sql}");
}

/// The claim is what moves `started_at`. Until this ran, the column held the
/// enqueue time forever and `finished_at - started_at` read as queue wait
/// plus runtime.
#[test]
fn claiming_restamps_started_at_but_not_queued_at() {
    let sql = running_update(Uuid::nil())
        .build(DatabaseBackend::Postgres)
        .sql;
    assert!(sql.starts_with("UPDATE"), "{sql}");
    assert!(sql.contains("\"started_at\""), "{sql}");
    assert!(sql.contains("\"status\""), "{sql}");
    assert!(
        !sql.contains("\"queued_at\""),
        "a claim must not rewrite the enqueue time: {sql}"
    );
}

fn period_result() -> PeriodResult {
    PeriodResult {
        period: 3,
        mean_spend: 1_200.0,
        realized_profit: 400.0,
        cumulative_profit: 900.0,
        actions: vec![1_100.0, 1_300.0],
        fits: vec![FitScore {
            edge: "marketing_spend -> net_sales".to_string(),
            fit: EdgeFit::fitted(4.0, 0.5),
            true_local_slope: 3.6,
            outcome: Outcome::Converged,
        }],
    }
}

/// `ON CONFLICT (<keys>) DO UPDATE`, with the conflict target naming every
/// primary-key column.
fn assert_upserts_on(sql: &str, keys: &[&str]) {
    let (target, _) = sql
        .split_once("DO UPDATE")
        .unwrap_or_else(|| panic!("not an upsert — no DO UPDATE in: {sql}"));
    let (_, target) = target
        .rsplit_once("ON CONFLICT")
        .unwrap_or_else(|| panic!("not an upsert — no ON CONFLICT in: {sql}"));
    for key in keys {
        assert!(
            target.contains(key),
            "conflict target {target:?} does not name the key column {key:?}"
        );
    }
}

/// The blocker this file exists for: the durable queue requeues a task whose
/// lease expired, `fail_run` keeps the dead attempt's periods, and the retry
/// restarts at period 1 — so a plain insert conflicts on its first write and
/// the run's `error` ends up holding a raw duplicate-key string.
#[test]
fn a_period_rewrites_rather_than_conflicting() {
    let sql = period_upsert(Uuid::nil(), &period_result())
        .build(DatabaseBackend::Postgres)
        .sql;
    assert_upserts_on(&sql, &["run_id", "period"]);
    // The rewrite has to carry the values, not just survive: a `DO NOTHING`
    // would leave the retry's row behind whatever it recomputed.
    assert!(sql.contains("mean_spend"), "{sql}");
}

#[test]
fn a_fit_rewrites_rather_than_conflicting() {
    let sql = fits_upsert(Uuid::nil(), &period_result())
        .build(DatabaseBackend::Postgres)
        .sql;
    assert_upserts_on(&sql, &["run_id", "period", "edge"]);
    assert!(sql.contains("outcome"), "{sql}");
}

/// A run that already reached a terminal status must not be driven back to
/// `running`.
///
/// The durable queue's reaper requeues a task whose lease expired
/// (`crud::queue::reap_stale_tasks`), and an outcome message can be dropped
/// after the run row was already closed out
/// (`TerminalWrite::NotOwned` — "this process just finished work the queue is
/// discarding"). Either way the same payload executes twice, and the second
/// attempt's claim lands on a `done`/`failed`/`cancelled` row. An unconditional
/// `UPDATE` resurrects it: `api::simulation::runs::limits` counts
/// `queued`/`running` against `MAX_IN_FLIGHT_PER_WORKSPACE`, so the run holds a
/// cap slot until `IN_FLIGHT_MAX_AGE_HOURS` ages it out, and a reader that
/// filters to terminal runs stops seeing evidence that was already written.
#[test]
fn a_claim_will_not_move_a_terminal_run() {
    let sql = running_update(Uuid::nil())
        .build(DatabaseBackend::Postgres)
        .sql;
    let (_, conditions) = sql
        .split_once(" WHERE ")
        .unwrap_or_else(|| panic!("a claim with no WHERE moves every run: {sql}"));
    assert!(
        conditions.contains("\"status\""),
        "a claim must be conditional on the run's current status, or a requeued \
         attempt drives a terminal run back to `running`: {conditions}"
    );
}
