//! An aborted run's estimate stops reading at the abort: days after the levers went off are
//! never read as treated post-period, so they cannot dilute (or inflate) the effect.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::NaiveDate;
use entity::experiment_flips;
use oxy_airlayer_compat::engine::experiment::{PanelMatrix, day_ordinal};
use oxy_app::server::experiment::panel::{PanelContext, PanelOutcome, PanelRequest};
use oxy_app::server::experiment::source::PanelSource;
use oxy_shared::errors::OxyError;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    Statement,
};
use uuid::Uuid;

use super::fixtures::*;
use super::harness::*;

/// Twelve weekly periods from 2 March, one-day washout: six pairs, alternating (on, off), (off, on).
/// Four pairs is the fewest whose sign-flip test can reach alpha 0.2.
const SWITCHBACK_YAML: &str = r#"
experiment: ramen_prompt_switchback
hypothesis: "the beverage prompt lifts beverage revenue per store-day"
owner: ops
for_each: store
lever:
  on:  { via: promo_on }
  off: { via: promo_off }
  per: wave
  read_back: { when_on: "{{store_days.promo_active}} > 0", when_off: "{{store_days.promo_active}} == 0", grace_days: 2 }
design:
  timezone: Australia/Sydney
  washout_days: 1
  switchback:
    period_days: 7
    periods:
      - { from: 2026-03-02, state: on }
      - { from: 2026-03-09, state: off }
      - { from: 2026-03-16, state: off }
      - { from: 2026-03-23, state: on }
      - { from: 2026-03-30, state: on }
      - { from: 2026-04-06, state: off }
      - { from: 2026-04-13, state: off }
      - { from: 2026-04-20, state: on }
      - { from: 2026-04-27, state: on }
      - { from: 2026-05-04, state: off }
      - { from: 2026-05-11, state: off }
      - { from: 2026-05-18, state: on }
  seed: 902113
outcome:
  target: store_days.beverages_revenue
  direction: increase
  alpha: 0.2
  mde_at_proposal: 10.0
"#;

const ON_PERIODS: [bool; 12] = [
    true, false, false, true, true, false, false, true, true, false, false, true,
];
const TAU: f64 = 12.0;
/// Planted on every on-period day from the abort on — days the levers were off.
const AFTER_ABORT: f64 = 500.0;

fn day(m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, m, d).unwrap()
}

/// The abort: decided on 29 April, mid-way through period 9 (an `on` from 27 April).
fn abort_day() -> NaiveDate {
    day(4, 29)
}

/// A warehouse that holds the whole registered calendar, and honours `to_exclusive` and
/// `restrict_to` as the real source does. Records every request's `to_exclusive`.
#[derive(Default)]
struct AbortPanels {
    ends: Mutex<Vec<NaiveDate>>,
}

#[async_trait]
impl PanelSource for AbortPanels {
    async fn load(&self, _c: &PanelContext, req: &PanelRequest) -> Result<PanelOutcome, OxyError> {
        self.ends.lock().unwrap().push(req.history.to_exclusive);
        let start = day_ordinal(day(3, 2));
        let (abort, end) = (
            day_ordinal(abort_day()),
            day_ordinal(req.history.to_exclusive),
        );
        let mut rng = Rng(NOISE_SEED + 9);
        let mut rows = Vec::new();
        for u in 0..12usize {
            let name = format!("s{:02}", u + 1);
            if req.restrict_to.as_ref().is_some_and(|f| !f.contains(&name)) {
                continue;
            }
            for d in (start - 300)..(start + 84).min(end) {
                let k = d - start;
                let on = (0..84).contains(&k) && ON_PERIODS[(k / 7) as usize];
                let lift = match (on, d >= abort) {
                    (true, false) => TAU,
                    (true, true) => AFTER_ABORT,
                    (false, _) => 0.0,
                };
                let weekly = 2.0 * (((d % 7) as f64) - 3.0);
                rows.push((
                    name.clone(),
                    d,
                    100.0 + 3.0 * u as f64 + weekly + lift + rng.noise(0.5),
                ));
            }
        }
        Ok(PanelOutcome::Panel(PanelMatrix::from_triples(rows)))
    }
}

async fn flip(
    db: &DatabaseConnection,
    run: Uuid,
    unit: &str,
    state: &str,
    due: NaiveDate,
    wave: i32,
) {
    experiment_flips::ActiveModel {
        id: Set(Uuid::new_v4()),
        created_at: Set(now()),
        run_id: Set(run),
        unit: Set(unit.into()),
        target_state: Set(state.into()),
        due_at: Set(due.and_hms_opt(0, 0, 0).unwrap()),
        wave: Set(wave),
        task_id: Set(Some(Uuid::new_v4())),
        agentic_run_id: Set(None),
        lever_revision_id: Set(None),
        status: Set("confirmed".into()),
        confirmed_on: Set(Some(due.and_hms_opt(0, 0, 0).unwrap())),
        reason: Set(None),
    }
    .insert(db)
    .await
    .expect("seed flip");
}

/// The flips the run recorded: every boundary up to period 9's `on` (27 April), then the
/// abort's offs, which carry that `on`'s index.
async fn seed_flips(db: &DatabaseConnection, run: Uuid) {
    for unit in stores() {
        for (k, on) in ON_PERIODS.iter().enumerate().take(9) {
            let from = day(3, 2) + chrono::Days::new(7 * k as u64);
            let state = if *on { "on" } else { "off" };
            flip(db, run, &unit, state, from, k as i32).await;
        }
        flip(db, run, &unit, "off", abort_day(), 8).await;
    }
}

#[tokio::test]
async fn an_aborted_runs_estimate_does_not_read_past_the_abort() {
    let db = setup_db().await;
    let plan = Plan {
        cfg_yaml: SWITCHBACK_YAML,
        view_text: VIEW,
        fleet: Some(serde_json::json!(stores())),
        abort_reason: Some("owner call"),
        ..Default::default()
    };
    let run = seed_plan(&db, "aborted", plan).await;
    seed_flips(&db, run).await;
    let src = Arc::new(AbortPanels::default());
    let outcome = run_estimate_with(&db, run, src.clone()).await;
    assert!(is_done(&outcome), "{outcome:?}");

    // Every read ends, exclusive, on the abort day — not the registered end (25 May).
    let ends = src.ends.lock().unwrap().clone();
    assert!(!ends.is_empty());
    assert!(ends.iter().all(|e| *e == abort_day()), "{ends:?}");

    let row = result_of(&db, run).await.unwrap();
    assert!(row.aborted);
    // The series states the cut and holds no day on or after it.
    let series = row.series.as_ref().expect("a sealed series");
    assert_eq!(series["measured_until"], "2026-04-29");
    let days = series["days"].as_array().expect("days");
    assert!(!days.is_empty());
    assert!(
        days.iter()
            .all(|d| d["date"].as_str().unwrap() < "2026-04-29")
    );
    let report = report_of(&row);
    assert!(
        report
            .label
            .contains("the measurement stops before 2026-04-29"),
        "{}",
        report.label
    );
    let h = report
        .headline
        .as_ref()
        .unwrap_or_else(|| panic!("a headline: {:?}", report.refusal));
    // Only the four pairs before the abort are read: the planted post-abort lift never enters.
    assert!((h.estimate - TAU).abs() < 2.0, "{}", h.estimate);
    let total = report
        .treated_total
        .as_ref()
        .expect("a total over the on days");
    assert_eq!(
        (total.units, total.days),
        (12, 4 * (7 - 1)),
        "the engine's retained on-days: the four pairs read, not the registered six"
    );
}

/// A waves warehouse holding every day to 100 after the switch, honouring `to_exclusive`.
struct WavesPanels;

#[async_trait]
impl PanelSource for WavesPanels {
    async fn load(&self, _c: &PanelContext, req: &PanelRequest) -> Result<PanelOutcome, OxyError> {
        let (switch, end) = (switch_day(), day_ordinal(req.history.to_exclusive));
        let mut rng = Rng(NOISE_SEED + 11);
        let mut rows = Vec::new();
        for (u, name) in stores().iter().enumerate() {
            for d in (switch - 300)..(switch + 100).min(end) {
                let lift = if u < 6 && d >= switch { TAU } else { 0.0 };
                rows.push((
                    name.clone(),
                    d,
                    155.0 + 3.0 * u as f64 + lift + rng.noise(1.0),
                ));
            }
        }
        Ok(PanelOutcome::Panel(PanelMatrix::from_triples(rows)))
    }
}

#[tokio::test]
async fn an_aborted_waves_run_cut_before_its_post_window_is_a_sealed_refusal() {
    // Aborted on 20 March, two weeks into a 56-day post window: the cut panel cannot hold the
    // registered window, so the engine refuses — and that refusal is sealed as the report, not a
    // failed (retried) job.
    let db = setup_db().await;
    let run = seed_with(
        &db,
        "aborted",
        VIEW,
        Some(pinned_pool()),
        Some("owner call"),
    )
    .await;
    for unit in ["s01", "s02", "s03", "s04", "s05"] {
        flip(&db, run, unit, "off", day(3, 20), 0).await;
    }
    let outcome = run_estimate_with(&db, run, Arc::new(WavesPanels)).await;
    assert!(is_done(&outcome), "{outcome:?}");
    let row = result_of(&db, run).await.expect("the sealed result row");
    assert!(row.aborted);
    let report = report_of(&row);
    assert!(
        report.refusal.is_some() && report.headline.is_none(),
        "{:?}",
        report.headline
    );
    assert!(
        report
            .label
            .contains("the measurement stops before 2026-03-20"),
        "{}",
        report.label
    );
}

#[tokio::test]
async fn a_guardrail_abort_names_the_breaching_unit_in_its_label() {
    // A breach is stored in `guardrail_breach`; `abort_reason` stays null (that is the
    // owner's call). The label must still say why the run was aborted, and name the unit.
    let db = setup_db().await;
    let run = seed_with(&db, "aborted", VIEW, Some(pinned_pool()), None).await;
    let breach = "`{{store_days.labor_cost_pct}} > 0.34` held at `s03` on 2026-03-04 (value 0.4)";
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE experiment_runs SET guardrail_breach = $1 WHERE id = $2",
        [breach.into(), run.into()],
    ))
    .await
    .unwrap();
    let outcome = run_estimate_with(&db, run, Arc::new(FixturePanels)).await;
    assert!(is_done(&outcome), "{outcome:?}");
    let report = report_of(&result_of(&db, run).await.unwrap());
    assert!(
        report
            .label
            .contains(&format!("(guardrail breached: {breach})")),
        "{}",
        report.label
    );
}

#[tokio::test]
async fn a_breach_outranks_an_owner_abort_that_landed_in_the_same_tick() {
    // The runner aborts on the breach first (`run::abort_reason`) and tells the owner so;
    // an owner's request saved before that tick must not replace it in the report.
    let db = setup_db().await;
    let run = seed_with(&db, "aborted", VIEW, Some(pinned_pool()), None).await;
    let breach = "`{{store_days.labor_cost_pct}} > 0.34` held at `s03` on 2026-03-04 (value 0.4)";
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE experiment_runs SET guardrail_breach = $1, abort_reason = $2 WHERE id = $3",
        [breach.into(), "owner stopped it".into(), run.into()],
    ))
    .await
    .unwrap();
    let outcome = run_estimate_with(&db, run, Arc::new(FixturePanels)).await;
    assert!(is_done(&outcome), "{outcome:?}");
    let report = report_of(&result_of(&db, run).await.unwrap());
    assert!(
        report
            .label
            .contains(&format!("(guardrail breached: {breach})")),
        "{}",
        report.label
    );
}
