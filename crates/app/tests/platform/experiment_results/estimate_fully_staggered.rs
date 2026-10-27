//! A fully staggered rollout: every unit placed over two waves, nothing held back.
//! The second wave is the first's not-yet-treated control; it has none itself, so it
//! is dropped and reported, never folded in.

use super::fixtures::*;
use super::harness::*;

#[tokio::test]
async fn a_fully_staggered_run_estimates_every_wave_but_the_last_with_an_empty_pool() {
    let db = setup_db().await;
    // s07..s12 switch on 11 May, after the first wave's window (2 March + 7 + 56).
    let cfg = CONFIG_YAML.replace(
        "    - { switch_on: 2026-03-02, units: [s01, s02, s03, s04, s05, s06] }\n",
        "    - { switch_on: 2026-03-02, units: [s01, s02, s03, s04, s05, s06] }\n    - { switch_on: 2026-05-11, units: [s07, s08, s09, s10, s11, s12] }\n",
    );
    let run = seed_plan(
        &db,
        "closing",
        Plan {
            cfg_yaml: &cfg,
            view_text: VIEW,
            pool: Some(serde_json::json!([])),
            flips: true,
            ..Default::default()
        },
    )
    .await;
    let outcome = run_estimate(&db, run).await;
    assert!(is_done(&outcome), "{outcome:?}");

    let report = report_of(&result_of(&db, run).await.expect("a result row"));
    let h = report
        .headline
        .as_ref()
        .unwrap_or_else(|| panic!("a headline, not a refusal: {:?}", report.refusal));
    let (lo, hi) = (h.interval.low.unwrap(), h.interval.high.unwrap());
    assert!(
        lo <= TAU && TAU <= hi,
        "the first wave's {TAU} is outside [{lo}, {hi}]"
    );
    assert_eq!(
        report.checks.dropped_waves.len(),
        1,
        "{:?}",
        report.checks.dropped_waves
    );
}
