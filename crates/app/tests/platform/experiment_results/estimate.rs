//! The estimate job, end to end, on a synthetic panel with a known injected effect.
//!
//! If `an_estimate_job_reports_a_known_injected_effect_within_its_ci` misses the
//! interval, the fixture's noise draw is unlucky (a 95% interval misses 5% of the
//! time): change `harness::NOISE_SEED`, never the assertion.

use agentic_core::delegation::TaskOutcome;
use oxy_app::server::experiment::estimate::store;
use oxy_app::server::experiment::jobs::{ESTIMATE_KIND, EstimatePayload};
use oxy_app::server::experiment::store as runs;
use oxy_experiment::config::parse;
use oxy_experiment::patch::CoefficientEdit;
use oxy_experiment::report::{DesignKind, WriteBackView};
use uuid::Uuid;

use super::fixtures::*;
use super::harness::*;

#[test]
fn the_kind_and_payload_are_the_contracts() {
    assert_eq!(ESTIMATE_KIND, "experiment_estimate");
    let id = Uuid::new_v4();
    assert_eq!(
        serde_json::to_value(EstimatePayload { run_id: id }).unwrap(),
        serde_json::json!({ "run_id": id })
    );
}

#[tokio::test]
async fn the_sealed_result_carries_the_day_by_day_series_across_the_whole_run() {
    let db = setup_db().await;
    let run = seed(&db, "closing", VIEW).await;
    assert!(is_done(&run_estimate(&db, run).await));

    let row = result_of(&db, run).await.expect("a results row");
    let s = row.series.as_ref().expect("a sealed series");
    assert_eq!(s["design"], "waves");
    assert_eq!(s["measure"], "store_days.beverages_revenue");
    assert_eq!(
        (s["treated_units"].as_u64(), s["control_units"].as_u64()),
        (Some(6), Some(6))
    );
    assert_eq!(s["waves"][0]["switch_on"], "2026-03-02");
    assert!(s["measured_until"].is_null());
    // From the pre window's first day (switch − anticipation 3 − pre 56) to the post
    // window's last (switch + washout 7 + post 56, exclusive): every day, none dropped.
    let days = s["days"].as_array().expect("days");
    assert_eq!(days.first().unwrap()["date"], "2026-01-02");
    assert_eq!(days.last().unwrap()["date"], "2026-05-03");
    assert_eq!(days.len(), 59 + 63);
    let gap =
        |d: &serde_json::Value| d["treated"].as_f64().unwrap() - d["control"].as_f64().unwrap();
    let mean_gap = |on: bool| {
        let picked: Vec<f64> = days
            .iter()
            .filter(|d| (d["on_units"].as_u64() == Some(6)) == on)
            .map(gap)
            .collect();
        picked.iter().sum::<f64>() / picked.len() as f64
    };
    // The planted lift shows in the raw days, not only in the estimate.
    assert!(
        mean_gap(true) - mean_gap(false) > TAU / 2.0,
        "on {} vs off {}",
        mean_gap(true),
        mean_gap(false)
    );
}

#[tokio::test]
async fn an_estimate_job_reports_a_known_injected_effect_within_its_ci() {
    let db = setup_db().await;
    let run = seed(&db, "closing", VIEW).await;

    let outcome = run_estimate(&db, run).await;
    assert!(is_done(&outcome), "{outcome:?}");

    let row = result_of(&db, run)
        .await
        .expect("a results row was written");
    assert!(!row.aborted);
    let report = report_of(&row);
    let h = report
        .headline
        .as_ref()
        .unwrap_or_else(|| panic!("a headline, not a refusal: {:?}", report.refusal));
    let (lo, hi) = (
        h.interval.low.expect("bounded"),
        h.interval.high.expect("bounded"),
    );
    assert!(
        lo <= TAU && TAU <= hi,
        "the injected effect {TAU} is outside the reported interval [{lo}, {hi}]"
    );
    assert!(
        report.substitution.floor.is_some(),
        "the sibling's placebo floor is stated"
    );
    assert_eq!(
        report.proposals.count, 2,
        "only `done` proposals before `registered_at` count"
    );
    assert!(
        report.compliance.c_treated < 1.0 && report.compliance.c_treated > 0.5,
        "one of six flips failed: {}",
        report.compliance.c_treated
    );
    assert_eq!(
        report.compliance.c_control_leak, 0.0,
        "six pinned controls, none leaked"
    );
    assert!(
        report.lever_note.is_none(),
        "one automation revision throughout"
    );
    assert_ne!(
        report.seeds.placebo, report.seeds.effect,
        "seeds are recorded per random step"
    );
    assert_eq!(report.design, DesignKind::Units);
    assert!(
        !report.directional
            && report.assumptions.is_empty()
            && report.prediction.is_none()
            && row.prediction.is_none()
    );

    // The slope is proposed: a qualitative edge, a strong first stage.
    let ratio = report.ratio.as_ref().expect("a driver was registered");
    assert!(
        ratio.interval.low.unwrap() <= COEFFICIENT && COEFFICIENT <= ratio.interval.high.unwrap()
    );
    assert!(matches!(
        report.write_back,
        Some(WriteBackView::Proposed { .. })
    ));
    let edit: CoefficientEdit =
        serde_json::from_str(row.coefficient_patch.as_deref().expect("a patch")).unwrap();
    let pinned = oxy_app::server::experiment::store::run_by_id(&db, run)
        .await
        .unwrap()
        .expect("the run");
    let frozen = oxy_app::server::experiment::design_store::design_by_id(&db, pinned.design_id)
        .await
        .unwrap()
        .expect("the frozen design");
    assert_eq!(
        edit.refs_entry,
        format!("experiment:{NAME}@{}", &frozen.content_hash[..12]),
        "the registered design, by its hash"
    );
    assert_eq!(edit.file_path, "views/store_days.view.yml");

    // A second attempt of the same task is a no-op: same row, still one row.
    let again = run_estimate(&db, run).await;
    assert!(is_done(&again), "{again:?}");
    let after = result_of(&db, run).await.unwrap();
    assert_eq!(
        after.id, row.id,
        "the registered result must not be recomputed or replaced"
    );
}

#[tokio::test]
async fn an_unpinned_control_pool_is_reported_as_a_refusal_not_a_failed_job() {
    // Aborted before the first check ever ran: no pool, so no honest assignment. The job
    // does not fall back to a fresh unit query — the report says so, and says it is an abort.
    let db = setup_db().await;
    let run = seed_with(
        &db,
        "aborted",
        VIEW,
        None,
        Some("guardrail: store_days.refund_rate"),
    )
    .await;
    assert!(is_done(&run_estimate(&db, run).await));
    let row = result_of(&db, run).await.unwrap();
    assert!(row.aborted);
    let report = report_of(&row);
    assert!(
        report.refusal.as_deref().unwrap().contains("never pinned"),
        "{:?}",
        report.refusal
    );
    assert!(report.headline.is_none());
    assert!(report.label.starts_with("ABORTED RUN"), "{}", report.label);
    assert!(row.coefficient_patch.is_none());
}

#[tokio::test]
async fn an_aborted_run_with_a_pinned_pool_is_estimated_and_labelled_as_an_abort() {
    let db = setup_db().await;
    let run = seed_with(
        &db,
        "aborted",
        VIEW,
        Some(pinned_pool()),
        Some("owner call"),
    )
    .await;
    let outcome = run_estimate(&db, run).await;
    assert!(is_done(&outcome), "{outcome:?}");
    let row = result_of(&db, run).await.unwrap();
    assert!(row.aborted);
    let report = report_of(&row);
    assert!(report.label.starts_with("ABORTED RUN"), "{}", report.label);
    assert!(report.headline.is_some(), "{:?}", report.refusal);
}

#[tokio::test]
async fn the_view_is_read_at_the_promoted_revision() {
    // Views stay in git and resolve by name: the edge lookup reads the workspace's promoted
    // revision, which is the file a write-back edits — never an older one.
    let db = setup_db().await;
    let (ws, older) = seed_workspace(&db, "prune").await;
    let promoted = seed_revision(&db, ws, "def5678").await;
    promote(&db, ws, promoted).await;
    let definition: serde_json::Value = serde_yaml::from_str(VIEW).unwrap();
    seed_view(
        &db,
        promoted,
        "store_days",
        "views/store_days.view.yml",
        definition,
    )
    .await; // NOT on `older`
    let cfg = parse(CONFIG_YAML).unwrap();
    let run = seed_run(
        &db,
        RunSeed {
            config: serde_json::to_value(&cfg).unwrap(),
            ..RunSeed::new(ws, NAME, "closing")
        },
    )
    .await;
    let model = runs::run_by_id(&db, run).await.unwrap().unwrap();
    let pinned_cfg = runs::pinned_config(&db, &model).await.unwrap();
    let v = store::load_view(&db, &model, &pinned_cfg)
        .await
        .unwrap()
        .expect("found on the promoted revision");
    assert_eq!(v.file_path, "views/store_days.view.yml");
    let _ = older;
}

#[tokio::test]
async fn estimate_job_refuses_a_live_run() {
    let db = setup_db().await;
    for state in ["live", "registered"] {
        let run = seed(&db, state, VIEW).await;
        let outcome = run_estimate(&db, run).await;
        let TaskOutcome::Failed(why) = outcome else {
            panic!("a {state} run must not be estimated: {outcome:?}")
        };
        assert!(why.contains("sealed"), "{why}");
        assert!(
            result_of(&db, run).await.is_none(),
            "no row, no computation"
        );
    }
}

#[tokio::test]
async fn write_back_is_refused_for_a_non_linear_edge() {
    let db = setup_db().await;
    let text = VIEW.replace(
        "        direction: positive\n        strength: strong\n",
        "        form: log-log\n        coefficient: 1.2\n",
    );
    let run = seed(&db, "closing", &text).await;
    assert!(is_done(&run_estimate(&db, run).await));
    let row = result_of(&db, run).await.unwrap();
    assert!(
        row.coefficient_patch.is_none(),
        "no patch for a non-linear edge"
    );
    let report = report_of(&row);
    let Some(WriteBackView::Refused { reason }) = report.write_back else {
        panic!("refused")
    };
    assert!(reason.contains("log-log"), "{reason}");
    assert!(
        report.headline.is_some(),
        "the effect itself is still reported"
    );
}
