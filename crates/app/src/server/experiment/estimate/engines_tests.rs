use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::NaiveDate;
use oxy_experiment::config::parse;
use oxy_experiment::timeline::last_post_window_end_date;
use oxy_shared::errors::OxyError;
use uuid::Uuid;

use super::engines::{load_panels, run_engines};
use super::plan::derive_seeds;
use super::testdata::{CONFIG_YAML, switchback_config};
use crate::server::experiment::panel::{
    GroupMembership, PanelContext, PanelOutcome, PanelRefusal, PanelRequest, is_revision_refusal,
};
use crate::server::experiment::source::PanelSource;

/// Records every request, answers "no history" to all of them.
#[derive(Default)]
struct NoHistoryEverywhere {
    seen: Mutex<Vec<PanelRequest>>,
}

#[async_trait]
impl PanelSource for NoHistoryEverywhere {
    async fn load(&self, _c: &PanelContext, req: &PanelRequest) -> Result<PanelOutcome, OxyError> {
        self.seen.lock().unwrap().push(req.clone());
        Ok(PanelOutcome::NoHistory {
            measure: req.measure.clone(),
            entity: req.entity.clone(),
        })
    }
}

struct Broken;

#[async_trait]
impl PanelSource for Broken {
    async fn load(&self, _c: &PanelContext, _r: &PanelRequest) -> Result<PanelOutcome, OxyError> {
        Err(OxyError::RuntimeError(
            "warehouse connection refused".into(),
        ))
    }
}

/// Answers every read with one error, as `load_panel` would return it.
struct Erroring(fn() -> OxyError);

#[async_trait]
impl PanelSource for Erroring {
    async fn load(&self, _c: &PanelContext, _r: &PanelRequest) -> Result<PanelOutcome, OxyError> {
        Err((self.0)())
    }
}

const WHY: &str = "the assign_by dimension `store.region` matched no row of the entity `store`";

fn invalid() -> OxyError {
    OxyError::ValidationError(WHY.to_string())
}

fn revision_race() -> OxyError {
    PanelRefusal::Revision {
        expected: Uuid::from_u128(1),
        actual: Uuid::from_u128(2).to_string(),
    }
    .into()
}

fn ctx() -> PanelContext {
    PanelContext {
        workspace_id: Uuid::nil(),
        revision_id: Uuid::nil(),
        requested_by: Uuid::nil(),
    }
}

fn pool() -> Vec<String> {
    vec!["s07".into(), "s08".into(), "s09".into()]
}

#[tokio::test]
async fn panels_are_requested_for_the_entity_up_to_the_exclusive_window_end() {
    let cfg = parse(CONFIG_YAML).unwrap();
    let src = NoHistoryEverywhere::default();
    load_panels(&src, &ctx(), &cfg, None, None, None)
        .await
        .expect("loads");
    let seen = src.seen.lock().unwrap();
    let measures: Vec<&str> = seen.iter().map(|r| r.measure.as_str()).collect();
    assert_eq!(
        measures,
        [
            "store_days.beverages_revenue",
            "store_days.entree_revenue",
            "store_days.promo_redemptions"
        ]
    );
    let end: NaiveDate = last_post_window_end_date(&cfg).unwrap();
    for r in seen.iter() {
        assert_eq!(r.entity, "store");
        assert_eq!(
            (r.history.from, r.history.to_exclusive),
            (None, end),
            "all history, up to the exclusive end"
        );
        assert!(r.membership.is_none(), "no assign_by, no membership");
    }
}

#[tokio::test]
async fn no_history_is_a_refusal_in_every_section_not_a_failed_job() {
    let cfg = parse(CONFIG_YAML).unwrap();
    let panels = load_panels(
        &NoHistoryEverywhere::default(),
        &ctx(),
        &cfg,
        None,
        None,
        None,
    )
    .await
    .expect("loads");
    let e = run_engines(&cfg, &pool(), &panels, derive_seeds(1), 20);
    assert_eq!(
        e.effect.view.refusal.as_deref(),
        Some("no history for `store_days.beverages_revenue` over `store`")
    );
    assert!(
        e.sibling
            .unwrap()
            .view
            .refusal
            .unwrap()
            .contains("store_days.entree_revenue")
    );
    assert!(e.sibling_power.unwrap().view.refusal.is_some());
    assert!(e.ratio.unwrap().view.refusal.is_some());
    assert_eq!(e.n_untreated, 0);
}

#[tokio::test]
async fn a_panel_source_error_fails_the_load() {
    let cfg = parse(CONFIG_YAML).unwrap();
    assert!(
        load_panels(&Broken, &ctx(), &cfg, None, None, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn validation_error_from_panel_seals_not_retries() {
    // A refusal a person can act on is data. `Err` here would be retried into the dead-letter queue.
    let cfg = parse(CONFIG_YAML).unwrap();
    let panels = load_panels(&Erroring(invalid), &ctx(), &cfg, None, None, None)
        .await
        .expect("a refusal is not an infrastructure error");
    let e = run_engines(&cfg, &pool(), &panels, derive_seeds(1), 20);
    assert_eq!(
        e.effect.view.refusal.as_deref(),
        Some(WHY),
        "the person sees the source's own words"
    );
    assert!(e.sibling.unwrap().view.refusal.unwrap().contains(WHY));
    assert!(e.ratio.unwrap().view.refusal.unwrap().contains(WHY));
    assert_eq!(e.n_untreated, 0);
}

#[tokio::test]
async fn a_revision_refusal_fails_the_load_and_is_never_sealed() {
    // A promotion raced the read, or the workspace was never promoted: not a property of
    // the run. Sealing it would make "ask for pricing again" the registered result forever.
    assert!(is_revision_refusal(&revision_race()));
    assert!(!is_revision_refusal(&invalid()));
    let cfg = parse(CONFIG_YAML).unwrap();
    let err = load_panels(&Erroring(revision_race), &ctx(), &cfg, None, None, None)
        .await
        .err()
        .expect("a revision refusal is retried, not sealed");
    assert!(err.to_string().contains("revision"), "{err}");
}

#[tokio::test]
async fn under_assign_by_every_request_carries_the_runs_pinned_membership() {
    let mut cfg = parse(CONFIG_YAML).unwrap();
    cfg.assign_by = Some("store.region".into());
    let pinned = GroupMembership(BTreeMap::from([(
        "north".to_string(),
        vec!["s01".to_string()],
    )]));
    let src = NoHistoryEverywhere::default();
    load_panels(&src, &ctx(), &cfg, Some(&pinned), None, None)
        .await
        .unwrap();
    let seen = src.seen.lock().unwrap();
    assert_eq!(seen.len(), 3, "target, sibling and driver");
    for r in seen.iter() {
        assert_eq!(
            r.membership.as_ref().map(|m| &m.0),
            Some(&pinned.0),
            "every measure is summed by the SAME pinned map"
        );
    }
}

#[tokio::test]
async fn a_switchbacks_panels_are_restricted_to_its_pinned_fleet() {
    // The host switchback registers no sibling or driver; give it both, so all three reads show.
    let mut cfg = switchback_config();
    cfg.substitution_check = parse(CONFIG_YAML).unwrap().substitution_check;
    cfg.outcome.driver = Some("store_days.promo_redemptions".into());
    let fleet = vec!["s01".to_string(), "s02".to_string()];
    let src = NoHistoryEverywhere::default();
    load_panels(&src, &ctx(), &cfg, None, Some(&fleet), None)
        .await
        .unwrap();
    let seen = src.seen.lock().unwrap();
    assert_eq!(seen.len(), 3, "target, sibling and driver");
    assert!(
        seen.iter()
            .all(|r| r.restrict_to.as_deref() == Some(fleet.as_slice())),
        "every measure reads the SAME fleet"
    );
}

#[tokio::test]
async fn a_switchbacks_panels_run_to_the_day_after_its_last_period() {
    let cfg = switchback_config(); // pre_days / post_days are None on a switchback
    let src = NoHistoryEverywhere::default();
    load_panels(&src, &ctx(), &cfg, None, None, None)
        .await
        .expect("loads");
    let end = NaiveDate::from_ymd_opt(2026, 3, 30).unwrap(); // the day after the 23rd–29th period
    let seen = src.seen.lock().unwrap();
    assert!(!seen.is_empty());
    assert!(seen.iter().all(|r| r.history.to_exclusive == end));
}

#[tokio::test]
async fn a_measure_in_another_view_reads_that_views_own_calendar() {
    // `design.time_dimension` is a column of the target's view; a driver in another view
    // (the spec's `amazon_ads_daily.spend_cad`) passes no calendar, so the panel read
    // resolves that view's only date dimension instead of refusing a foreign column.
    let mut cfg = parse(CONFIG_YAML).unwrap();
    cfg.design.time_dimension = Some("store_days.day".into());
    cfg.outcome.driver = Some("ads_daily.spend".into());
    let src = NoHistoryEverywhere::default();
    load_panels(&src, &ctx(), &cfg, None, None, None)
        .await
        .unwrap();
    let seen = src.seen.lock().unwrap();
    let td = |m: &str| {
        seen.iter()
            .find(|r| r.measure == m)
            .unwrap_or_else(|| panic!("{m} was read"))
            .time_dimension
            .clone()
    };
    assert_eq!(
        td("store_days.beverages_revenue").as_deref(),
        Some("store_days.day")
    );
    assert_eq!(
        td("store_days.entree_revenue").as_deref(),
        Some("store_days.day")
    );
    assert_eq!(td("ads_daily.spend"), None, "another view's own calendar");
}

#[tokio::test]
async fn each_measure_is_read_with_its_own_filter() {
    use oxy_experiment::{FilterOp, MeasureFilter};
    let only = |dim: &str, v: &str| MeasureFilter {
        dimension: dim.into(),
        op: FilterOp::Equals,
        values: vec![v.into()],
    };
    let mut cfg = parse(CONFIG_YAML).unwrap();
    cfg.outcome.target_filter = vec![only("store_days.sku", "TB-40")];
    cfg.outcome.driver_filter = vec![only("store_days.channel", "ads")];
    cfg.substitution_check.as_mut().unwrap().filter =
        vec![only("store_days.category", "backpacks")];
    let src = NoHistoryEverywhere::default();
    load_panels(&src, &ctx(), &cfg, None, None, None)
        .await
        .unwrap();
    let seen = src.seen.lock().unwrap();
    let dims: Vec<Vec<&str>> = seen
        .iter()
        .map(|r| r.filters.iter().map(|f| f.dimension.as_str()).collect())
        .collect();
    assert_eq!(
        dims,
        [
            vec!["store_days.sku"],
            vec!["store_days.category"],
            vec!["store_days.channel"]
        ],
        "target, sibling and driver each carry only their own filter"
    );
}

/// Answers every request with a thinned panel that dropped `sparse` for that
/// measure, and records each request.
#[derive(Default)]
struct ThinnedFor {
    seen: Mutex<Vec<PanelRequest>>,
    sparse: BTreeMap<String, String>,
}

#[async_trait]
impl PanelSource for ThinnedFor {
    async fn load(&self, _c: &PanelContext, req: &PanelRequest) -> Result<PanelOutcome, OxyError> {
        self.seen.lock().unwrap().push(req.clone());
        let rows: Vec<(String, i64, f64)> = ["s01", "s02"]
            .iter()
            .flat_map(|u| (0..3).map(move |d| (u.to_string(), 739_000 + d, 1.0)))
            .collect();
        let dropped = self
            .sparse
            .get(&req.measure)
            .map(|u| {
                vec![oxy_airlayer_compat::engine::experiment::DroppedUnit {
                    unit: u.clone(),
                    reason: format!("sparse in {}", req.measure),
                }]
            })
            .unwrap_or_default();
        Ok(PanelOutcome::Thinned {
            panel: oxy_airlayer_compat::engine::experiment::PanelMatrix::from_triples(rows),
            dropped,
        })
    }
}

#[tokio::test]
async fn a_ratio_denominator_is_read_leniently_and_its_sparse_unit_listed_once() {
    let mut cfg = parse(CONFIG_YAML).unwrap();
    cfg.outcome.denominator = Some("store_days.covers".into());
    let src = ThinnedFor {
        sparse: BTreeMap::from([
            (
                "store_days.beverages_revenue".to_string(),
                "s05".to_string(),
            ),
            ("store_days.covers".to_string(), "s05".to_string()),
        ]),
        ..Default::default()
    };
    let panels = super::engines::load_panels_for(true, &src, &ctx(), &cfg, None, None, None)
        .await
        .expect("loads");
    let seen = src.seen.lock().unwrap();
    let covers = seen
        .iter()
        .find(|r| r.measure == "store_days.covers")
        .expect("the denominator is read");
    assert_eq!(covers.unit_floor, Some(cfg.design.coverage_floor));
    let units: Vec<&str> = panels.dropped.iter().map(|d| d.unit.as_str()).collect();
    assert_eq!(units, ["s05"], "dropped by both reads, listed once");
}

#[tokio::test]
async fn a_ratio_outcome_reads_no_driver_slope() {
    // The headline is a share; the driver slope is read on the numerator's levels, so
    // "predicted against measured" would compare two different quantities.
    let mut cfg = parse(CONFIG_YAML).unwrap();
    cfg.outcome.denominator = Some("store_days.covers".into());
    assert!(cfg.outcome.driver.is_some(), "the fixture names a driver");
    let panels = super::engines::load_panels_for(
        true,
        &ThinnedFor::default(),
        &ctx(),
        &cfg,
        None,
        None,
        None,
    )
    .await
    .expect("loads");
    let e = run_engines(&cfg, &pool(), &panels, derive_seeds(1), 20);
    let refusal = e.ratio.expect("a driver is named").view.refusal;
    assert!(
        refusal
            .as_deref()
            .is_some_and(|r| r.contains("ratio outcome")),
        "{refusal:?}"
    );
}
