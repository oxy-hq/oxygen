//! A retrospective run: nothing checked it, so the estimate pins its control pool
//! itself, from the units the warehouse held in the design's own pre window — never
//! from the units it holds today. The report says it was not pre-registered, compliance
//! is not measured (no lever was moved), and no coefficient is proposed.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::NaiveDate;
use oxy_app::server::experiment::panel::{PanelContext, PanelOutcome, PanelRequest};
use oxy_app::server::experiment::source::PanelSource;
use oxy_app::server::experiment::store as runs;
use oxy_experiment::report::{ReportStatus, WriteBackView};
use oxy_shared::errors::OxyError;

use super::fixtures::*;
use super::harness::*;

/// [`FixturePanels`], remembering every request.
#[derive(Default)]
struct Recording(Mutex<Vec<PanelRequest>>);

#[async_trait]
impl PanelSource for Recording {
    async fn load(&self, c: &PanelContext, req: &PanelRequest) -> Result<PanelOutcome, OxyError> {
        self.0.lock().unwrap().push(req.clone());
        FixturePanels.load(c, req).await
    }
}

async fn seed_retrospective(db: &sea_orm::DatabaseConnection) -> uuid::Uuid {
    seed_plan(
        db,
        "closing",
        Plan {
            cfg_yaml: CONFIG_YAML,
            view_text: VIEW,
            retrospective: true,
            ..Default::default()
        },
    )
    .await
}

#[tokio::test]
async fn pins_the_pool_from_the_design_pre_window_and_labels_the_report() {
    let db = setup_db().await;
    let run = seed_retrospective(&db).await;
    let panels = Arc::new(Recording::default());
    let outcome = run_estimate_with(&db, run, panels.clone()).await;
    assert!(is_done(&outcome), "{outcome:?}");

    // The pool: every unit with history in the pre window, minus the treated.
    let row = runs::run_by_id(&db, run).await.unwrap().unwrap();
    assert_eq!(runs::control_pool(&row), Some(pinned_pool_names()));
    // Read over the pre window: switch 2026-03-02 − anticipation 3 − pre 56 → 2026-01-02,
    // up to (exclusive) the anticipation window's start, 2026-02-27.
    let first = panels.0.lock().unwrap()[0].clone();
    assert_eq!(first.measure, "store_days.beverages_revenue");
    assert_eq!(first.history.from, NaiveDate::from_ymd_opt(2026, 1, 2));
    assert_eq!(
        first.history.to_exclusive,
        NaiveDate::from_ymd_opt(2026, 2, 27).unwrap()
    );

    let result = result_of(&db, run).await.expect("a results row");
    assert!(!result.aborted);
    let report = report_of(&result);
    assert_eq!(report.status, ReportStatus::Retrospective);
    assert!(
        report.label.starts_with("RETROSPECTIVE"),
        "{}",
        report.label
    );
    assert!(report.headline.is_some(), "{:?}", report.refusal);
    assert!(report.compliance.not_measured.is_some());
    assert!(result.coefficient_patch.is_none());
    assert!(matches!(
        report.write_back,
        Some(WriteBackView::Refused { ref reason }) if reason.contains("retrospective")
    ));
}

#[tokio::test]
async fn a_second_estimate_keeps_the_first_pin() {
    let db = setup_db().await;
    let run = seed_retrospective(&db).await;
    assert!(is_done(&run_estimate(&db, run).await));
    let pinned = runs::control_pool(&runs::run_by_id(&db, run).await.unwrap().unwrap());
    assert!(is_done(&run_estimate(&db, run).await));
    let again = runs::control_pool(&runs::run_by_id(&db, run).await.unwrap().unwrap());
    assert_eq!(pinned, again);
}

fn pinned_pool_names() -> Vec<String> {
    serde_json::from_value(pinned_pool()).unwrap()
}

/// `CONFIG_YAML` as a rollout that already happened: declared, not drawn — no
/// lever, no seed, no MDE.
fn observed_yaml() -> String {
    let lever = CONFIG_YAML
        .split("lever:")
        .nth(1)
        .and_then(|rest| rest.split("design:").next())
        .expect("the fixture has a lever block");
    CONFIG_YAML
        .replace(&format!("lever:{lever}"), "")
        .replace("  seed: 8841207\n", "  assignment: observed\n")
        .replace("  mde_at_proposal: 17.5\n", "")
}

#[tokio::test]
async fn an_observed_rollout_is_estimated_and_says_it_was_not_randomised() {
    let db = setup_db().await;
    let yaml = observed_yaml();
    let run = seed_plan(
        &db,
        "closing",
        Plan {
            cfg_yaml: &yaml,
            view_text: VIEW,
            retrospective: true,
            ..Default::default()
        },
    )
    .await;
    let outcome = run_estimate(&db, run).await;
    assert!(is_done(&outcome), "{outcome:?}");
    let report = report_of(&result_of(&db, run).await.expect("a results row"));
    assert!(report.headline.is_some(), "{:?}", report.refusal);
    assert!(report.proposals.observational);
    assert!(report.promise.is_none());
    assert!(
        report
            .mde_note
            .as_deref()
            .is_some_and(|n| n.starts_with("MDE unavailable")),
        "{:?}",
        report.mde_note
    );
}

#[tokio::test]
async fn a_retrospective_drops_a_named_unit_with_no_data_and_lists_it() {
    let db = setup_db().await;
    let yaml = observed_yaml().replace("s06] }", "s06, s99] }");
    assert!(yaml.contains("s99"), "the extra unit is in the wave");
    let run = seed_plan(
        &db,
        "closing",
        Plan {
            cfg_yaml: &yaml,
            view_text: VIEW,
            retrospective: true,
            ..Default::default()
        },
    )
    .await;
    let outcome = run_estimate(&db, run).await;
    assert!(is_done(&outcome), "{outcome:?}");
    let report = report_of(&result_of(&db, run).await.expect("a results row"));
    assert!(report.refusal.is_none(), "{:?}", report.refusal);
    assert_eq!(
        report.checks.dropped_units,
        vec!["s99: a treated unit with no rows in the panel".to_string()]
    );
}
