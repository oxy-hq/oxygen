use std::collections::{BTreeMap, HashMap};

use async_trait::async_trait;
use chrono::{Days, Duration, NaiveDate};
use oxy_app::server::experiment::check::reader::{AnomalyQuery, AnomalyScan, Membership, UnitDays};
use oxy_app::server::experiment::check::{Readers, run_check};
use oxy_app::server::experiment::panel::GroupMembership;
use oxy_app::server::experiment::store;
use oxy_experiment::timeline::last_post_window_end_date;

use super::fixture::*;

const PROMO: &str = "store_days.promo_active";
const TARGET: &str = "store_days.beverages_revenue";
const ALL: &[&str] = &[
    "bondi",
    "newtown",
    "manly",
    "surry_hills",
    "glebe",
    "paddington",
];

/// The warehouse, as a table the test writes. A series over several units
/// (a group) is keyed by the units joined with `+`.
#[derive(Default)]
pub(crate) struct FakeDays {
    values: HashMap<(String, String), BTreeMap<NaiveDate, f64>>,
    units: Vec<String>,
    groups: BTreeMap<String, Vec<String>>,
}

impl FakeDays {
    pub(crate) fn new(units: &[&str]) -> Self {
        Self {
            units: units.iter().map(|u| u.to_string()).collect(),
            ..Default::default()
        }
    }

    pub(crate) fn group(mut self, name: &str, members: &[&str]) -> Self {
        self.groups
            .insert(name.into(), members.iter().map(|m| m.to_string()).collect());
        self
    }

    pub(crate) fn set(mut self, measure: &str, unit: &str, days: &[(&str, f64)]) -> Self {
        let series = self
            .values
            .entry((measure.into(), unit.into()))
            .or_default();
        for (day, v) in days {
            series.insert(d(day), *v);
        }
        self
    }

    /// A series over several units at once — a group's aggregate (used from Task 10a on).
    #[allow(dead_code)]
    pub(crate) fn set_over(self, measure: &str, units: &[&str], days: &[(&str, f64)]) -> Self {
        let key = units.join("+");
        self.set(measure, &key, days)
    }

    pub(crate) fn fill(
        mut self,
        measure: &str,
        unit: &str,
        from: NaiveDate,
        to: NaiveDate,
        v: f64,
    ) -> Self {
        let series = self
            .values
            .entry((measure.into(), unit.into()))
            .or_default();
        for day in from.iter_days().take_while(|day| *day <= to) {
            series.insert(day, v);
        }
        self
    }
}

#[async_trait]
impl UnitDays for FakeDays {
    async fn values(
        &self,
        measure: &str,
        unit: &str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<BTreeMap<NaiveDate, f64>, String> {
        Ok(self
            .values
            .get(&(measure.to_string(), unit.to_string()))
            .map(|s| s.range(from..=to).map(|(d, v)| (*d, *v)).collect())
            .unwrap_or_default())
    }

    async fn values_over(
        &self,
        measure: &str,
        units: &[String],
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<BTreeMap<NaiveDate, f64>, String> {
        self.values(measure, &units.join("+"), from, to).await
    }

    async fn units(&self, _measure: &str, _since_days: u32) -> Result<Vec<String>, String> {
        Ok(self.units.clone())
    }
}

#[async_trait]
impl Membership for FakeDays {
    async fn groups(&self, _entity: &str, _dimension: &str) -> Result<GroupMembership, String> {
        Ok(GroupMembership(self.groups.clone()))
    }
}

/// The detector: `Some(detail)` flags every scanned unit on its `since` day.
pub(crate) struct Scan(pub Option<String>);

#[async_trait]
impl AnomalyScan for Scan {
    async fn flagged(&self, q: &AnomalyQuery<'_>) -> Result<Vec<(NaiveDate, String)>, String> {
        Ok(self
            .0
            .iter()
            .map(|detail| (q.since, detail.clone()))
            .collect())
    }
}

pub(crate) async fn check(
    fx: &Fx,
    days: &FakeDays,
    scan: &Scan,
    at: chrono::DateTime<chrono::Utc>,
) {
    let run = fx.run().await;
    let readers = Readers {
        days,
        anomalies: scan,
        groups: days,
    };
    run_check(&fx.db, &run, &readers, at)
        .await
        .expect("check completes");
}

async fn live() -> Fx {
    let fx = Fx::seed(DESIGN).await;
    fx.register().await;
    fx.tick(switch_at("2026-03-02")).await;
    fx
}

#[tokio::test]
async fn read_back_confirms_a_flip_on_the_first_day_when_on_held() {
    let fx = live().await;
    let days = FakeDays::new(ALL)
        .set(PROMO, "bondi", &[("2026-03-02", 0.0), ("2026-03-03", 1.0)])
        .set(PROMO, "newtown", &[("2026-03-02", 1.0)]);
    check(
        &fx,
        &days,
        &Scan(None),
        switch_at("2026-03-03") + Duration::hours(12),
    )
    .await;
    let bondi = fx.flip("bondi", "on").await;
    assert_eq!(
        (bondi.status.as_str(), bondi.confirmed_on),
        (
            "confirmed",
            Some(d("2026-03-03").and_hms_opt(0, 0, 0).unwrap())
        )
    );
    assert_eq!(
        fx.flip("newtown", "on").await.confirmed_on,
        Some(d("2026-03-02")).map(|d| d.and_hms_opt(0, 0, 0).unwrap())
    );
}

#[tokio::test]
async fn a_flip_whose_grace_ends_seen_and_never_held_is_unconfirmed() {
    let fx = live().await;
    let days = FakeDays::new(ALL).set(
        PROMO,
        "newtown",
        &[
            ("2026-03-02", 0.0),
            ("2026-03-03", 0.0),
            ("2026-03-04", 0.0),
        ],
    );
    check(&fx, &days, &Scan(None), switch_at("2026-03-05")).await;
    let f = fx.flip("newtown", "on").await;
    assert_eq!(f.status, "unconfirmed");
    assert!(f.reason.unwrap().contains("never held"));
}

#[tokio::test]
async fn read_back_waits_on_a_day_the_warehouse_has_not_loaded() {
    let fx = live().await;
    let days = FakeDays::new(ALL).set(PROMO, "bondi", &[("2026-03-02", 0.0), ("2026-03-04", 0.0)]);
    check(&fx, &days, &Scan(None), switch_at("2026-03-09")).await;
    assert_eq!(
        fx.flip("bondi", "on").await.status,
        "dispatched",
        "undecided, not unconfirmed"
    );
}

#[tokio::test]
async fn the_control_pool_is_pinned_once_and_excludes_treated_units() {
    let fx = live().await;
    check(
        &fx,
        &FakeDays::new(ALL),
        &Scan(None),
        switch_at("2026-03-03"),
    )
    .await;
    assert_eq!(
        store::control_pool(&fx.run().await),
        Some(vec!["glebe".into(), "paddington".into()])
    );
    let grown: Vec<&str> = ALL.iter().copied().chain(["redfern"]).collect();
    check(
        &fx,
        &FakeDays::new(&grown),
        &Scan(None),
        switch_at("2026-03-04"),
    )
    .await;
    assert_eq!(
        store::control_pool(&fx.run().await),
        Some(vec!["glebe".into(), "paddington".into()]),
        "a store opening mid-run cannot drift into the pool"
    );
}

#[tokio::test]
async fn a_control_where_when_off_fails_is_a_leak_recorded_once() {
    let fx = live().await;
    let days = FakeDays::new(ALL)
        .set(PROMO, "glebe", &[("2026-03-02", 0.0), ("2026-03-04", 1.0)])
        .set(
            PROMO,
            "paddington",
            &[("2026-03-02", 0.0), ("2026-03-04", 0.0)],
        );
    check(&fx, &days, &Scan(None), switch_at("2026-03-05")).await;
    let obs = store::observations(&fx.run().await);
    assert_eq!(
        obs.control_leaks,
        BTreeMap::from([("glebe".to_string(), d("2026-03-04"))])
    );
    assert_eq!(obs.controls_checked_on, Some(d("2026-03-05")));
    // Recorded once: the next day's check — which, if it re-read glebe, would
    // find an earlier failing day — leaves the recorded leak untouched.
    let later = FakeDays::new(ALL)
        .set(
            PROMO,
            "glebe",
            &[
                ("2026-03-02", 0.0),
                ("2026-03-03", 1.0),
                ("2026-03-04", 1.0),
            ],
        )
        .set(
            PROMO,
            "paddington",
            &[("2026-03-02", 0.0), ("2026-03-04", 0.0)],
        );
    check(&fx, &later, &Scan(None), switch_at("2026-03-06")).await;
    let again = store::observations(&fx.run().await);
    assert_eq!(
        again.control_leaks, obs.control_leaks,
        "a leak is sticky, never re-read"
    );
    assert_eq!(again.controls_checked_on, Some(d("2026-03-06")));
}

#[tokio::test]
async fn coverage_is_days_present_over_post_days() {
    let fx = live().await;
    let c = fx.design().await;
    let end = last_post_window_end_date(&c).unwrap();
    let start = end - Days::new(56);
    let half = start + Days::new(27);
    let days = FakeDays::new(ALL)
        .fill(TARGET, "manly", start, half, 10.0)
        .fill(TARGET, "surry_hills", start, half, 10.0);
    check(
        &fx,
        &days,
        &Scan(None),
        oxy_experiment::timeline::day_start_utc(end, chrono_tz::Australia::Sydney),
    )
    .await;
    assert_eq!(store::observations(&fx.run().await).coverage, Some(0.5));
}

#[tokio::test]
async fn a_stratified_pool_holds_only_units_the_strata_name_and_pinning_says_why() {
    let design = DESIGN.replace(
        "  seed: 8841207",
        "  strata:\n    - [bondi, manly, glebe]\n    - [newtown, surry_hills, paddington]\n  seed: 8841207",
    );
    let fx = Fx::seed(&design).await;
    fx.register().await;
    fx.tick(switch_at("2026-03-02")).await;
    // redfern opened after pricing: history, but no stratum.
    let opened: Vec<&str> = ALL.iter().copied().chain(["redfern"]).collect();
    check(
        &fx,
        &FakeDays::new(&opened),
        &Scan(None),
        switch_at("2026-03-03"),
    )
    .await;
    assert_eq!(
        store::control_pool(&fx.run().await),
        Some(vec!["glebe".into(), "paddington".into()]),
        "a unit in no stratum is in neither arm"
    );
    let notices = fx.notices().await;
    assert!(
        notices.iter().any(|n| n
            .body
            .as_deref()
            .is_some_and(|b| b.starts_with("at pinning: ") && b.contains("redfern"))),
        "{:?}",
        notices.iter().map(|n| n.body.clone()).collect::<Vec<_>>()
    );
    // Never re-run with later units: a store opened after pinning (alexandria)
    // is in no stratum by design — no notice, no problem.
    let before = fx.notices().await;
    let later: Vec<&str> = opened.iter().copied().chain(["alexandria"]).collect();
    check(
        &fx,
        &FakeDays::new(&later),
        &Scan(None),
        switch_at("2026-03-04"),
    )
    .await;
    let after = fx.notices().await;
    assert_eq!(after.len(), before.len());
    assert!(
        after
            .iter()
            .all(|n| !n.body.as_deref().is_some_and(|b| b.contains("alexandria"))),
        "{:?}",
        after.iter().map(|n| n.body.clone()).collect::<Vec<_>>()
    );
    let obs = store::observations(&fx.run().await);
    assert!(
        obs.problems.iter().all(|p| !p.contains("alexandria")),
        "{:?}",
        obs.problems
    );
}

#[tokio::test]
async fn an_empty_unit_list_is_not_yet_read_so_nothing_is_pinned() {
    let fx = live().await;
    check(
        &fx,
        &FakeDays::new(&[]),
        &Scan(None),
        switch_at("2026-03-03"),
    )
    .await;
    let run = fx.run().await;
    assert_eq!(
        store::control_pool(&run),
        None,
        "an empty read pins nothing"
    );
    assert!(
        store::observations(&run)
            .problems
            .iter()
            .any(|p| p.contains("no units")),
        "{:?}",
        store::observations(&run).problems
    );
    check(
        &fx,
        &FakeDays::new(ALL),
        &Scan(None),
        switch_at("2026-03-04"),
    )
    .await;
    assert_eq!(
        store::control_pool(&fx.run().await),
        Some(vec!["glebe".into(), "paddington".into()]),
        "retried on the next check"
    );
}

/// Defensive only: PR 2's `validate_static` refuses this expression at compile
/// (C2 `expr.rs`), so a registered design cannot carry it — unless its row was
/// written before that check existed. Then it is a problem, not a crash.
#[tokio::test]
async fn an_unparseable_read_back_is_a_problem_not_a_crash() {
    let design = DESIGN.replace(
        "\"{{store_days.promo_active}} > 0\"",
        "\"store_days.promo_active > 0\"",
    );
    let fx = Fx::seed(&design).await;
    fx.register().await;
    fx.tick(switch_at("2026-03-02")).await;
    check(
        &fx,
        &FakeDays::new(ALL),
        &Scan(None),
        switch_at("2026-03-03"),
    )
    .await;
    let obs = store::observations(&fx.run().await);
    assert!(
        obs.problems
            .iter()
            .any(|p| p.contains("lever.read_back.when_on")),
        "{:?}",
        obs.problems
    );
    assert_eq!(fx.flip("bondi", "on").await.status, "dispatched");
}

#[tokio::test]
async fn a_check_that_cannot_run_says_so_on_the_run_and_keeps_what_was_recorded() {
    use oxy_app::server::experiment::check::executor::check_and_record;
    let fx = live().await;
    // A breach and a coverage already on the run: a failed check erases neither.
    check(
        &fx,
        &FakeDays::new(ALL),
        &Scan(Some("labor spike".into())),
        switch_at("2026-03-03"),
    )
    .await;
    fx.exec(
        "UPDATE experiment_runs SET guardrail_breach = 'labor_cost_pct breached' \
         WHERE workspace_id = $1",
        [fx.ws.into()],
    )
    .await;
    let before = store::observations(&fx.run().await);
    // The seeded workspace has no checkout on this node: no context to read through.
    let run = fx.run().await;
    for _ in 0..2 {
        let err = check_and_record(&fx.db, run.id, switch_at("2026-03-04"))
            .await
            .expect_err("no workspace context");
        assert!(err.contains("workspace context"), "{err}");
    }
    let after = fx.run().await;
    let obs = store::observations(&after);
    let failures: Vec<_> = obs
        .problems
        .iter()
        .filter(|p| p.contains("the last check failed"))
        .collect();
    assert_eq!(failures.len(), 1, "recorded once: {:?}", obs.problems);
    assert!(failures[0].contains("workspace context"), "{failures:?}");
    assert_eq!(
        obs.checked_at, before.checked_at,
        "a failure is not a check"
    );
    assert_eq!(obs.coverage, before.coverage);
    assert_eq!(obs.control_leaks, before.control_leaks);
    assert_eq!(
        after.guardrail_breach.as_deref(),
        Some("labor_cost_pct breached")
    );
}
