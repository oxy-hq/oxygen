//! The carryover window, end to end: the days after the close `off`s are read on their
//! own panel and reported beside — never inside — the registered estimate.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::NaiveDate;
use entity::experiment_flips;
use oxy_airlayer_compat::engine::experiment::{PanelMatrix, day_ordinal};
use oxy_app::server::experiment::panel::{PanelContext, PanelOutcome, PanelRequest};
use oxy_app::server::experiment::source::PanelSource;
use oxy_shared::errors::OxyError;
use sea_orm::{ActiveModelTrait, Set};
use uuid::Uuid;

use super::fixtures::*;
use super::harness::*;

const CARRY: f64 = 10.0;

fn day(m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, m, d).unwrap()
}

/// The close: the post window ends (exclusive) on 4 May, when the levers went off.
fn off_day() -> NaiveDate {
    day(5, 4)
}

/// s01..s06 gain `TAU` while on and `CARRY` for the 14 days after the off; the others
/// are flat. Honours `to_exclusive`, and records it per request.
#[derive(Default)]
struct CarryPanels {
    ends: Mutex<Vec<(String, NaiveDate)>>,
}

#[async_trait]
impl PanelSource for CarryPanels {
    async fn load(&self, _c: &PanelContext, req: &PanelRequest) -> Result<PanelOutcome, OxyError> {
        self.ends
            .lock()
            .unwrap()
            .push((req.measure.clone(), req.history.to_exclusive));
        let (switch, off, end) = (
            switch_day(),
            day_ordinal(off_day()),
            day_ordinal(req.history.to_exclusive),
        );
        let lifted = |d: i64| match (d >= switch, d >= off, d < off + 14) {
            (true, false, _) => TAU,
            (true, true, true) => CARRY,
            _ => 0.0,
        };
        let mut rng = Rng(NOISE_SEED + 21);
        let mut rows = Vec::new();
        for (u, name) in stores().iter().enumerate() {
            for d in (switch - 300)..end {
                let lift = if u < 6 { lifted(d) } else { 0.0 };
                let weekly = 2.0 * (((d % 7) as f64) - 3.0);
                rows.push((
                    name.clone(),
                    d,
                    155.0 + 3.0 * u as f64 + weekly + lift + rng.noise(1.0),
                ));
            }
        }
        Ok(PanelOutcome::Panel(PanelMatrix::from_triples(rows)))
    }
}

#[tokio::test]
async fn the_days_after_the_off_are_reported_as_carryover_beside_the_registered_effect() {
    let db = setup_db().await;
    let cfg = CONFIG_YAML
        .replace(
            "  washout_days: 7\n",
            "  washout_days: 7\n  carryover_days: 14\n",
        )
        .replace("  driver: store_days.promo_redemptions\n", "")
        .replace(
            "substitution_check: { sibling: store_days.entree_revenue }\n",
            "",
        );
    let run = seed_plan(
        &db,
        "closing",
        Plan {
            cfg_yaml: &cfg,
            view_text: VIEW,
            pool: Some(pinned_pool()),
            flips: true,
            ..Default::default()
        },
    )
    .await;
    for unit in ["s01", "s02", "s03", "s04", "s05", "s06"] {
        experiment_flips::ActiveModel {
            id: Set(Uuid::new_v4()),
            created_at: Set(now()),
            run_id: Set(run),
            unit: Set(unit.into()),
            target_state: Set("off".into()),
            due_at: Set(off_day().and_hms_opt(0, 0, 0).unwrap()),
            wave: Set(0),
            task_id: Set(Some(Uuid::new_v4())),
            agentic_run_id: Set(None),
            lever_revision_id: Set(None),
            status: Set("confirmed".into()),
            confirmed_on: Set(Some(off_day().and_hms_opt(0, 0, 0).unwrap())),
            reason: Set(None),
        }
        .insert(&db)
        .await
        .expect("seed the close off");
    }
    let panels = Arc::new(CarryPanels::default());
    let outcome = run_estimate_with(&db, run, panels.clone()).await;
    assert!(is_done(&outcome), "{outcome:?}");

    let report = report_of(&result_of(&db, run).await.expect("a result row"));
    let h = report.headline.as_ref().expect("a headline");
    assert!(
        (h.estimate - TAU).abs() < 3.0,
        "the registered effect stays the on-effect: {}",
        h.estimate
    );
    let c = report.carryover.as_ref().expect("a carryover section");
    assert_eq!(c.days, 14);
    assert!(c.lingered, "{}", c.statement);
    let est = c.effect.estimate.expect("a carryover estimate");
    assert!(
        (est - CARRY).abs() < 3.0,
        "the carryover reads the after-off lift: {est}"
    );

    let ends = panels.ends.lock().unwrap();
    let target_ends: Vec<NaiveDate> = ends
        .iter()
        .filter(|(m, _)| m == "store_days.beverages_revenue")
        .map(|(_, e)| *e)
        .collect();
    assert_eq!(
        target_ends,
        vec![off_day(), day(5, 18)],
        "the registered read stops at the off; the carryover read runs 14 days past it"
    );
}
