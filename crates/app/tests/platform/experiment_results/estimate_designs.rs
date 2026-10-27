//! The estimate job under the amended spec: predicted against measured, groups, and a
//! person-actionable refusal from the panel source. The switchback is `estimate_switchback`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use oxy_airlayer_compat::engine::experiment::PanelMatrix;
use oxy_app::server::experiment::panel::{
    GroupMembership, PanelContext, PanelOutcome, PanelRequest,
};
use oxy_app::server::experiment::source::PanelSource;
use oxy_experiment::config::PriorSource;
use oxy_experiment::patch::{CoefficientEdit, ReplacedPrior};
use oxy_experiment::report::{DesignKind, WriteBackView};
use oxy_shared::errors::OxyError;

use super::fixtures::*;
use super::harness::*;

const PRIOR_AT: &str = "  mde_at_proposal: 17.5\n";

/// `CONFIG_YAML` with the slope the World Model held when pricing froze it.
fn with_prior(value: f64, source: &str) -> String {
    CONFIG_YAML.replace(
        PRIOR_AT,
        &format!("{PRIOR_AT}  prior_coefficient: {{ value: {value}, source: {source} }}\n"),
    )
}

fn prior_plan(yaml: &str) -> Plan<'_> {
    Plan {
        cfg_yaml: yaml,
        view_text: VIEW,
        pool: Some(pinned_pool()),
        failed_unit: Some("s06"),
        flips: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn a_frozen_prior_is_set_against_the_measurement_and_stored_beside_the_report() {
    let db = setup_db().await;
    let yaml = with_prior(COEFFICIENT, "fitted"); // the planted slope: the World Model was right
    let run = seed_plan(&db, "closing", prior_plan(&yaml)).await;
    assert!(is_done(&run_estimate(&db, run).await));

    let row = result_of(&db, run).await.unwrap();
    let report = report_of(&row);
    let p = report
        .prediction
        .as_ref()
        .expect("a prior was frozen and the first stage is strong");
    assert_eq!(p.prior.source, PriorSource::Fitted);
    // predicted = prior × the MEASURED first stage (planted at TAU / COEFFICIENT), measured = ITT(target)
    assert!((p.predicted - TAU).abs() < 3.0, "{}", p.predicted);
    assert!((p.measured - TAU).abs() < 3.0, "{}", p.measured);
    assert_eq!(p.prior_in_interval, Some(true), "{}", p.statement);
    assert!(
        p.statement.contains("agrees") && p.statement.contains("fitted"),
        "{}",
        p.statement
    );
    assert_eq!(
        row.prediction,
        Some(serde_json::to_value(p).unwrap()),
        "the column is report.prediction"
    );
    assert!(!report.directional && report.alpha == 0.05 && report.assumptions.is_empty());
}

#[tokio::test]
async fn a_prior_far_from_the_earned_slope_is_contradicted_and_the_patch_names_it() {
    let db = setup_db().await;
    let yaml = with_prior(50.0, "declared"); // a declared slope ten times too steep
    let run = seed_plan(&db, "closing", prior_plan(&yaml)).await;
    assert!(is_done(&run_estimate(&db, run).await));

    let row = result_of(&db, run).await.unwrap();
    let report = report_of(&row);
    let p = report.prediction.as_ref().expect("compared");
    assert_eq!(p.prior_in_interval, Some(false));
    assert!(
        p.statement.contains("contradicts") && p.statement.contains("declared"),
        "{}",
        p.statement
    );
    // the write-back would replace exactly that prior — the report and the stored edit say so
    let edit: CoefficientEdit =
        serde_json::from_str(row.coefficient_patch.as_deref().expect("a patch")).unwrap();
    assert_eq!(
        edit.replaces,
        Some(ReplacedPrior {
            value: 50.0,
            source: PriorSource::Declared,
            edge_coefficient: None
        })
    );
    assert!(matches!(
        report.write_back,
        Some(WriteBackView::Proposed {
            replaces: Some(_),
            ..
        })
    ));
}

const GROUPED_YAML: &str = r#"
experiment: ads_by_category
hypothesis: "ads lift revenue per category-day"
owner: growth
for_each: sku
assign_by: sku.category
lever:
  on:  { via: ads_on }
  off: { via: ads_off }
  per: wave
  read_back: { when_on: "{{sku_days.ad_active}} > 0", when_off: "{{sku_days.ad_active}} == 0", grace_days: 2 }
design:
  timezone: America/Toronto
  waves:
    - { switch_on: 2026-03-02, units: [g1, g2, g3, g4] }
  seed: 4417
  pre_days: 56
  post_days: 56
  anticipation_days: 3
  washout_days: 7
outcome:
  target: sku_days.revenue
  direction: increase
  mde_at_proposal: 20.0
"#;

const MEMBER_LIFT: f64 = 8.0; // per member-day; a group of three gains 24

/// Raw per-member series for eight groups of three, summed the way C2a says `load_panel` sums:
/// by the membership the REQUEST carries. A request that carries none falls back to the
/// dimension's CURRENT values — the bug the pinned map exists to prevent.
struct GroupedPanels {
    current: BTreeMap<String, String>,
    seen: Mutex<Vec<Option<GroupMembership>>>,
}

impl GroupedPanels {
    /// `g1m3` has been reclassified into `g5` since the run was registered.
    fn reclassified() -> Self {
        let mut current: BTreeMap<String, String> = (1..=8)
            .flat_map(|g| (1..=3).map(move |m| (format!("g{g}m{m}"), format!("g{g}"))))
            .collect();
        current.insert("g1m3".into(), "g5".into());
        GroupedPanels {
            current,
            seen: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl PanelSource for GroupedPanels {
    async fn load(&self, _c: &PanelContext, req: &PanelRequest) -> Result<PanelOutcome, OxyError> {
        self.seen.lock().unwrap().push(req.membership.clone());
        let by_member: BTreeMap<String, String> = match &req.membership {
            Some(m) => {
                m.0.iter()
                    .flat_map(|(g, ms)| ms.iter().map(move |u| (u.clone(), g.clone())))
                    .collect()
            }
            None => self.current.clone(),
        };
        let switch = switch_day();
        let mut rng = Rng(NOISE_SEED + 5);
        let mut sums: BTreeMap<(String, i64), f64> = BTreeMap::new();
        for g in 1..=8usize {
            for m in 1..=3usize {
                let member = format!("g{g}m{m}");
                let lifted = g <= 4; // the group the member was RANDOMISED into carries the lift
                for d in (switch - 300)..=(switch + 100) {
                    let weekly = 1.5 * (((d % 7) as f64) - 3.0);
                    let lift = if lifted && d >= switch {
                        MEMBER_LIFT
                    } else {
                        0.0
                    };
                    let v = 40.0 + 2.0 * g as f64 + weekly + lift + rng.noise(1.0);
                    if let Some(group) = by_member.get(&member) {
                        *sums.entry((group.clone(), d)).or_default() += v;
                    }
                }
            }
        }
        Ok(PanelOutcome::Panel(PanelMatrix::from_triples(
            sums.into_iter().map(|((g, d), v)| (g, d, v)),
        )))
    }
}

fn pinned_membership() -> serde_json::Value {
    let m: BTreeMap<String, Vec<String>> = (1..=8)
        .map(|g| {
            (
                format!("g{g}"),
                (1..=3).map(|m| format!("g{g}m{m}")).collect(),
            )
        })
        .collect();
    serde_json::to_value(m).unwrap()
}

fn grouped_plan(membership: Option<serde_json::Value>) -> Plan<'static> {
    Plan {
        cfg_yaml: GROUPED_YAML,
        view_text: VIEW,
        pool: Some(serde_json::json!(["g5", "g6", "g7", "g8"])), // under assign_by the pool is control GROUPS
        membership,
        flips: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn group_member_reclassified_mid_run_stays_in_pinned_group() {
    let db = setup_db().await;
    let run = seed_plan(&db, "closing", grouped_plan(Some(pinned_membership()))).await;
    let src = Arc::new(GroupedPanels::reclassified());
    let outcome = run_estimate_with(&db, run, src.clone()).await;
    assert!(is_done(&outcome), "{outcome:?}");

    // The job handed the panel source the PINNED map, with the reclassified member still in g1.
    {
        let seen = src.seen.lock().unwrap(); // dropped before the next await
        assert!(!seen.is_empty());
        for m in seen.iter() {
            let m = m
                .as_ref()
                .expect("every request carries the run's pinned membership");
            assert!(
                m.0["g1"].contains(&"g1m3".to_string()),
                "g1m3 must stay in the group it was randomised with"
            );
            assert!(!m.0["g5"].contains(&"g1m3".to_string()));
        }
    }
    // Summed by the pinned map, g1 keeps all three lifted members: the estimate is the planted
    // group lift (3 × MEMBER_LIFT). Summed by the current dimension g1 would read 16.
    let report = report_of(&result_of(&db, run).await.unwrap());
    let h = report.headline.as_ref().expect("a headline");
    assert!(
        (h.estimate - 3.0 * MEMBER_LIFT).abs() < 4.0,
        "{}",
        h.estimate
    );
    assert_eq!(report.design, DesignKind::Groups);
    assert!(report.estimand.contains("group-day"), "{}", report.estimand);
    let total = report.treated_total.as_ref().unwrap();
    assert_eq!(total.units, 4, "four treated groups");
    assert!(total.basis.contains("treated groups"), "{}", total.basis);
}

#[tokio::test]
async fn an_assign_by_run_whose_membership_was_never_pinned_is_a_refusal_not_a_current_dimension_lookup()
 {
    let db = setup_db().await;
    let run = seed_plan(&db, "closing", grouped_plan(None)).await;
    let src = Arc::new(GroupedPanels::reclassified());
    assert!(is_done(&run_estimate_with(&db, run, src.clone()).await));
    let report = report_of(&result_of(&db, run).await.unwrap());
    assert!(
        report
            .refusal
            .as_deref()
            .unwrap()
            .contains("group membership was never pinned"),
        "{:?}",
        report.refusal
    );
    assert!(report.headline.is_none());
    assert!(
        src.seen.lock().unwrap().is_empty(),
        "no panel may be read without the pinned map"
    );
}

/// Answers every read with C2a's person-actionable refusal.
struct InvalidPanels;

#[async_trait]
impl PanelSource for InvalidPanels {
    async fn load(&self, _c: &PanelContext, _r: &PanelRequest) -> Result<PanelOutcome, OxyError> {
        Err(OxyError::ValidationError(
            "the time dimension of `store_days` is ambiguous: set design.time_dimension".into(),
        ))
    }
}

#[tokio::test]
async fn validation_error_from_panel_seals_not_retries() {
    // The worker would retry an `Err` into the dead-letter queue and the run would sit in
    // `closing` forever; the job must answer Done with a sealed, refused result instead.
    let db = setup_db().await;
    let run = seed(&db, "closing", VIEW).await;
    let outcome = run_estimate_with(&db, run, Arc::new(InvalidPanels)).await;
    assert!(
        is_done(&outcome),
        "a ValidationError must not fail the task: {outcome:?}"
    );

    let row = result_of(&db, run)
        .await
        .expect("the refusal is sealed as the run's result");
    let report = report_of(&row);
    assert!(
        report.refusal.as_deref().unwrap().contains("ambiguous"),
        "{:?}",
        report.refusal
    );
    assert!(
        report.headline.is_none() && row.coefficient_patch.is_none() && row.prediction.is_none()
    );
    assert!(
        report
            .ratio
            .as_ref()
            .unwrap()
            .refusal
            .as_deref()
            .unwrap()
            .contains("ambiguous")
    );
    assert_eq!(
        report.proposals.count, 2,
        "the operational sections still render"
    );
    // and a second delivery of the same task changes nothing
    assert!(is_done(
        &run_estimate_with(&db, run, Arc::new(InvalidPanels)).await
    ));
    assert_eq!(result_of(&db, run).await.unwrap().id, row.id);
}

/// `FixturePanels`, recording which units each read was restricted to.
#[derive(Default)]
struct RestrictLog(Mutex<Vec<Option<Vec<String>>>>);

#[async_trait]
impl PanelSource for RestrictLog {
    async fn load(&self, c: &PanelContext, req: &PanelRequest) -> Result<PanelOutcome, OxyError> {
        self.0.lock().unwrap().push(req.restrict_to.clone());
        FixturePanels.load(c, req).await
    }
}

#[tokio::test]
async fn a_unit_waves_runs_panels_are_restricted_to_its_assigned_units() {
    // A unit that opens mid-run is in neither arm: reading it would make the panel keep
    // only the days every unit has, wiping the pre window. The reads hold the wave's
    // units and the pinned control pool, nothing the warehouse holds now.
    let db = setup_db().await;
    let run = seed(&db, "closing", VIEW).await;
    let src = Arc::new(RestrictLog::default());
    assert!(is_done(&run_estimate_with(&db, run, src.clone()).await));
    let seen = src.0.lock().unwrap().clone();
    assert_eq!(seen.len(), 3, "target, sibling and driver");
    assert!(
        seen.iter()
            .all(|r| r.as_deref() == Some(stores().as_slice())),
        "{seen:?}"
    );
}
