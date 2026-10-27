//! The estimate job under the amended spec: a switchback, estimated over its pinned fleet.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use oxy_airlayer_compat::engine::experiment::PanelMatrix;
use oxy_app::server::experiment::panel::{PanelContext, PanelOutcome, PanelRequest};
use oxy_app::server::experiment::source::PanelSource;
use oxy_experiment::report::DesignKind;
use oxy_shared::errors::OxyError;

use super::fixtures::*;
use super::harness::*;

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
  seed: 902113
outcome:
  target: store_days.beverages_revenue
  direction: increase
  alpha: 0.2
  mde_at_proposal: 10.0
"#;

const TAU_SB: f64 = 12.0;
/// Four pairs: (on, off), (off, on), (on, off), (off, on).
const ON_PERIODS: [bool; 8] = [true, false, false, true, true, false, false, true];

/// Twelve stores, every one lifted by `TAU_SB` on every day of an *on* period — and a thirteenth,
/// `s99`, that opens mid-run at a level 1000 above the rest. It is outside the pinned fleet, so it
/// must never enter a period mean: a source that honours `restrict_to` leaves it out, and a job
/// that forgot to pass the fleet would read it and miss `TAU_SB` by hundreds.
#[derive(Default)]
struct SwitchbackPanels {
    seen: Mutex<Vec<Option<Vec<String>>>>,
}

#[async_trait]
impl PanelSource for SwitchbackPanels {
    async fn load(&self, _c: &PanelContext, req: &PanelRequest) -> Result<PanelOutcome, OxyError> {
        assert!(
            req.membership.is_none(),
            "a switchback carries no group membership"
        );
        self.seen.lock().unwrap().push(req.restrict_to.clone());
        let start = switch_day();
        let mut rng = Rng(NOISE_SEED + 7);
        let mut rows = Vec::new();
        for u in 0..13usize {
            let name = if u == 12 {
                "s99".to_string()
            } else {
                format!("s{:02}", u + 1)
            };
            if req.restrict_to.as_ref().is_some_and(|f| !f.contains(&name)) {
                continue;
            }
            for d in (start - 300)..(start + 56) {
                if u == 12 && d < start + 20 {
                    continue; // opened mid-run
                }
                let k = d - start;
                let on = (0..56).contains(&k) && ON_PERIODS[(k / 7) as usize];
                let weekly = 2.0 * (((d % 7) as f64) - 3.0);
                let lift = if on { TAU_SB } else { 0.0 };
                let level = if u == 12 {
                    1100.0
                } else {
                    100.0 + 3.0 * u as f64
                };
                rows.push((name.clone(), d, level + weekly + lift + rng.noise(0.5)));
            }
        }
        Ok(PanelOutcome::Panel(PanelMatrix::from_triples(rows)))
    }
}

#[tokio::test]
async fn a_switchback_run_is_estimated_over_its_pinned_fleet_and_reports_its_assumptions() {
    let db = setup_db().await;
    // No control pool and no membership — a switchback has no control arm — but the pinned fleet.
    let plan = Plan {
        cfg_yaml: SWITCHBACK_YAML,
        view_text: VIEW,
        fleet: Some(serde_json::json!(stores())),
        ..Default::default()
    };
    let run = seed_plan(&db, "closing", plan).await;
    let src = Arc::new(SwitchbackPanels::default());
    let outcome = run_estimate_with(&db, run, src.clone()).await;
    assert!(is_done(&outcome), "{outcome:?}");
    // The panel was read for the pinned twelve only; `s99`, opened mid-run, never entered a period mean.
    let expected = stores();
    assert!(
        src.seen
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.as_deref() == Some(expected.as_slice()))
    );

    let row = result_of(&db, run).await.unwrap();
    let report = report_of(&row);
    assert_eq!(report.design, DesignKind::Switchback);
    assert!(report.refusal.is_none(), "{:?}", report.refusal);
    let h = report.headline.as_ref().expect("a headline");
    assert!((h.estimate - TAU_SB).abs() < 2.0, "{}", h.estimate);
    // The day-by-day series is sealed with it: the pinned fleet's mean per day, every
    // day tagged with its period's state.
    let series = row.series.as_ref().expect("a sealed series");
    assert_eq!(series["design"], "switchback");
    assert_eq!(series["fleet_units"], 12);
    let days = series["days"].as_array().expect("days");
    assert!(!days.is_empty());
    assert!(days.iter().any(|d| d["on"] == true) && days.iter().any(|d| d["on"] == false));
    // registered alpha 0.2 → directional, never "significant"
    assert!(report.directional && report.alpha == 0.2);
    assert!(
        h.verdict.starts_with("directional at α = 0.2"),
        "{}",
        h.verdict
    );
    assert!(!h.text.contains("significant"), "{}", h.text);
    // the design's own assumptions, stated every time; the fleet was treated, so nothing to project onto
    assert_eq!(report.assumptions.len(), 2, "{:?}", report.assumptions);
    assert!(report.projection.is_none());
    let total = report
        .treated_total
        .as_ref()
        .expect("a total over the on days");
    assert_eq!(
        (total.units, total.days),
        (12, 4 * (7 - 1)),
        "fleet × the engine's retained on-days (4 pairs × 6 settled days, all covered)"
    );
    assert!(total.basis.contains("on-period days"), "{}", total.basis);
    assert!(
        row.prediction.is_none() && row.coefficient_patch.is_none(),
        "no driver registered"
    );
}

#[tokio::test]
async fn a_switchback_run_with_no_pinned_fleet_is_a_refusal_not_a_whole_panel_read() {
    // Closed before the first check pinned the fleet: reading "every store the warehouse has now"
    // would let a store opened mid-run into the period means. Every section refuses; no panel is read.
    let db = setup_db().await;
    let plan = Plan {
        cfg_yaml: SWITCHBACK_YAML,
        view_text: VIEW,
        ..Default::default()
    };
    let run = seed_plan(&db, "closing", plan).await;
    let src = Arc::new(SwitchbackPanels::default());
    assert!(is_done(&run_estimate_with(&db, run, src.clone()).await));
    let report = report_of(&result_of(&db, run).await.unwrap());
    assert!(
        report
            .refusal
            .as_deref()
            .unwrap()
            .contains("fleet was never pinned"),
        "{:?}",
        report.refusal
    );
    assert!(report.headline.is_none() && report.treated_total.is_none());
    assert_eq!(
        report.assumptions.len(),
        2,
        "a switchback's caveats are stated even when refused"
    );
    assert!(
        src.seen.lock().unwrap().is_empty(),
        "no panel may be read without the pinned fleet"
    );
}
