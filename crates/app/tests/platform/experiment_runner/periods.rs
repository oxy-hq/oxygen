use chrono::{Duration, NaiveDate};
use oxy_app::server::experiment::check::periods::read_periods;
use oxy_app::server::experiment::check::read_back::ReadBackInputs;
use oxy_app::server::experiment::store;
use oxy_experiment::compliance::FlipOutcome;

use super::check::{FakeDays, Scan, check};
use super::fixture::*;

const PROMO: &str = "store_days.promo_active";
const TARGET: &str = "store_days.beverages_revenue";
const FLEET: &[&str] = &["bondi", "glebe"];

fn days_of(from: &str, to: &str) -> Vec<NaiveDate> {
    d(from)
        .iter_days()
        .take_while(|day| *day <= d(to))
        .collect()
}

/// Registered; the first check pins the fleet; period 0's `on` goes out.
async fn live() -> Fx {
    let fx = Fx::seed(SWITCHBACK).await;
    fx.register().await;
    check(
        &fx,
        &FakeDays::new(FLEET),
        &Scan(None),
        utc("2026-02-20T00:00:00Z"),
    )
    .await;
    fx.tick(switch_at("2026-03-02")).await;
    fx.finish_levers("done").await;
    fx
}

#[tokio::test]
async fn the_first_check_pins_the_fleet_once_and_no_control_pool() {
    let fx = live().await;
    let grown: Vec<&str> = FLEET.iter().copied().chain(["redfern"]).collect();
    check(
        &fx,
        &FakeDays::new(&grown),
        &Scan(None),
        switch_at("2026-03-03"),
    )
    .await;
    let run = fx.run().await;
    assert_eq!(
        store::fleet(&run),
        Some(vec!["bondi".to_string(), "glebe".to_string()])
    );
    assert_eq!(
        store::control_pool(&run),
        None,
        "a switchback has no control arm (C3)"
    );
    fx.tick(switch_at("2026-03-03")).await;
    assert_eq!(
        fx.flips().await.len(),
        2,
        "a store opened mid-run is not flipped"
    );
}

#[tokio::test]
async fn an_empty_fleet_read_is_not_pinned_and_retries() {
    let fx = Fx::seed(SWITCHBACK).await;
    fx.register().await;
    check(
        &fx,
        &FakeDays::new(&[]),
        &Scan(None),
        utc("2026-02-20T00:00:00Z"),
    )
    .await;
    let run = fx.run().await;
    assert_eq!(
        store::fleet(&run),
        None,
        "an empty fleet is never pinned: it would be pinned for the whole run"
    );
    let problems = store::observations(&run).problems;
    assert!(
        problems.iter().any(|p| p.contains(
            "no rows for store_days.beverages_revenue in the last 28 days; fleet not pinned yet"
        )),
        "{problems:?}"
    );
    // The next hourly check retries, and the fleet it reads then is pinned.
    check(
        &fx,
        &FakeDays::new(FLEET),
        &Scan(None),
        utc("2026-02-20T02:00:00Z"),
    )
    .await;
    assert_eq!(
        store::fleet(&fx.run().await),
        Some(vec!["bondi".to_string(), "glebe".to_string()])
    );
}

#[tokio::test]
async fn on_periods_land_and_an_off_period_that_leaks_is_recorded_on_its_day() {
    let fx = live().await;
    fx.tick(switch_at("2026-03-09")).await;
    fx.finish_levers("done").await;
    let mut days = FakeDays::new(FLEET)
        .set(PROMO, "bondi", &[("2026-03-02", 1.0)])
        .set(PROMO, "glebe", &[("2026-03-02", 0.0), ("2026-03-03", 1.0)]);
    for day in days_of("2026-03-09", "2026-03-15") {
        let s = day.to_string();
        days = days.set(PROMO, "glebe", &[(s.as_str(), 0.0)]).set(
            PROMO,
            "bondi",
            &[(s.as_str(), if s == "2026-03-13" { 1.0 } else { 0.0 })],
        );
    }
    check(&fx, &days, &Scan(None), switch_at("2026-03-16")).await;
    let on: Vec<_> = fx
        .flips()
        .await
        .into_iter()
        .filter(|f| f.target_state == "on")
        .collect();
    assert!(on.iter().all(|f| f.status == "confirmed"), "{on:?}");
    let bondi = fx.flip("bondi", "off").await;
    assert_eq!(bondi.status, "unconfirmed");
    assert_eq!(
        bondi.confirmed_on,
        Some(d("2026-03-13")).map(|d| d.and_hms_opt(0, 0, 0).unwrap()),
        "the leak day (C3)"
    );
    assert!(
        bondi
            .reason
            .unwrap()
            .contains("`when_off` failed at `bondi` on 2026-03-13")
    );
    assert_eq!(
        fx.flip("glebe", "off").await.status,
        "confirmed",
        "held from the end of grace through the period"
    );
    let record = store::observations(&fx.run().await).period_outcomes;
    assert_eq!(
        record["bondi@2026-03-09"],
        FlipOutcome::LeakedToControl {
            on_day: d("2026-03-13").into()
        }
    );
    assert_eq!(
        record["glebe@2026-03-02"],
        FlipOutcome::Landed {
            on_day: d("2026-03-03").into()
        }
    );
}

#[tokio::test]
async fn an_off_period_waits_on_a_day_not_yet_arrived() {
    let fx = live().await;
    fx.tick(switch_at("2026-03-09")).await;
    fx.finish_levers("done").await;
    // 14 March never arrived for glebe.
    let mut days = FakeDays::new(FLEET);
    for s in ["2026-03-11", "2026-03-12", "2026-03-13", "2026-03-15"] {
        days = days.set(PROMO, "glebe", &[(s, 0.0)]);
    }
    check(&fx, &days, &Scan(None), switch_at("2026-03-20")).await;
    assert_eq!(
        fx.flip("glebe", "off").await.status,
        "dispatched",
        "neither a leak nor clean"
    );
    assert!(
        !store::observations(&fx.run().await)
            .period_outcomes
            .contains_key("glebe@2026-03-09")
    );
}

#[tokio::test]
async fn a_cancelled_period_is_recorded_as_read_back_saw_it() {
    // The driver was down from before the first period until 17 March: periods
    // 0 and 1 are cancelled, never sent (Task 9a). Read-back still reads them.
    let fx = Fx::seed(SWITCHBACK).await;
    fx.register().await;
    check(
        &fx,
        &FakeDays::new(FLEET),
        &Scan(None),
        utc("2026-02-20T00:00:00Z"),
    )
    .await;
    fx.tick(switch_at("2026-03-17")).await;
    let days = FakeDays::new(FLEET)
        // bondi's lever was on anyway on 3 March; glebe's never was.
        .set(PROMO, "bondi", &[("2026-03-02", 0.0), ("2026-03-03", 1.0)])
        .set(
            PROMO,
            "glebe",
            &[
                ("2026-03-02", 0.0),
                ("2026-03-03", 0.0),
                ("2026-03-04", 0.0),
            ],
        );
    check(
        &fx,
        &days,
        &Scan(None),
        switch_at("2026-03-17") + Duration::hours(1),
    )
    .await;
    let record = store::observations(&fx.run().await).period_outcomes;
    assert_eq!(
        record["bondi@2026-03-02"],
        FlipOutcome::Landed {
            on_day: d("2026-03-03").into()
        }
    );
    assert_eq!(
        record["glebe@2026-03-02"],
        FlipOutcome::NeverLanded,
        "never flipped, never seen on"
    );
    assert_eq!(
        fx.flip("glebe", "on").await.status,
        "cancelled",
        "a cancelled row keeps its status"
    );
}

#[tokio::test]
async fn a_switchbacks_coverage_is_its_last_period() {
    let fx = live().await;
    let days = FakeDays::new(FLEET).fill(TARGET, "glebe", d("2026-03-23"), d("2026-03-29"), 10.0);
    check(
        &fx,
        &days,
        &Scan(None),
        switch_at("2026-03-30") + Duration::hours(6),
    )
    .await;
    assert_eq!(store::observations(&fx.run().await).coverage, Some(1.0));
}

#[tokio::test]
async fn a_row_cancelled_by_an_abort_is_not_read_back_as_a_period() {
    let fx = live().await;
    fx.exec(
        "UPDATE experiment_flips SET status = 'cancelled', reason = 'run ended before dispatch' \
         WHERE unit = 'glebe'",
        [],
    )
    .await;
    // Both units seen with the lever off through grace: a sent row is NeverLanded.
    let days = FakeDays::new(FLEET)
        .set(
            PROMO,
            "bondi",
            &[
                ("2026-03-02", 0.0),
                ("2026-03-03", 0.0),
                ("2026-03-04", 0.0),
            ],
        )
        .set(
            PROMO,
            "glebe",
            &[
                ("2026-03-02", 0.0),
                ("2026-03-03", 0.0),
                ("2026-03-04", 0.0),
            ],
        );
    check(&fx, &days, &Scan(None), switch_at("2026-03-10")).await;
    let record = store::observations(&fx.run().await).period_outcomes;
    assert!(
        !record.contains_key("glebe@2026-03-02"),
        "an abandoned period is no period: {record:?}"
    );
    assert_eq!(
        record["bondi@2026-03-02"],
        FlipOutcome::NeverLanded,
        "a sent row is still read"
    );
}

#[tokio::test]
async fn a_settled_periods_outcome_is_on_the_run_before_the_check_saves() {
    // The check settles a flip (dispatched → confirmed) as it reads it, but saves the
    // observations only at its end. A failure in between must not lose the outcome: the
    // next check reads only `dispatched` rows, so a settled period is never read again.
    let fx = live().await;
    let run = fx.run().await;
    let c = store::pinned_config(&fx.db, &run).await.unwrap();
    let flips = store::flips_of(&fx.db, run.id).await.unwrap();
    let fleet = store::fleet(&run).expect("pinned");
    let inp = ReadBackInputs {
        run: &run,
        c: &c,
        flips: &flips,
        today: d("2026-03-05"),
        now: d("2026-03-05").into(),
        membership: None,
        fleet: Some(&fleet),
    };
    let days = FakeDays::new(FLEET)
        .set(PROMO, "bondi", &[("2026-03-02", 1.0)])
        .set(PROMO, "glebe", &[("2026-03-03", 1.0)]);
    let mut obs = store::observations(&run);
    let sb = c.design.switchback.as_ref().unwrap();
    read_periods(&fx.db, &inp, sb, &days, &mut obs)
        .await
        .unwrap();
    assert_eq!(fx.flip("bondi", "on").await.status, "confirmed");
    // The check dies here: `obs` is never saved.
    drop(obs);
    let record = store::observations(&fx.run().await).period_outcomes;
    assert_eq!(
        record.get("bondi@2026-03-02"),
        Some(&FlipOutcome::Landed {
            on_day: d("2026-03-02").into()
        }),
        "{record:?}"
    );
    assert!(record.contains_key("glebe@2026-03-02"), "{record:?}");
}
