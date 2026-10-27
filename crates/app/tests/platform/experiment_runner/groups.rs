use chrono::Duration;
use oxy_app::server::experiment::check::members::treated_groups;
use oxy_app::server::experiment::check::read_back::ReadBackInputs;
use oxy_app::server::experiment::store;
use oxy_experiment::compliance::FlipOutcome;
use std::collections::BTreeMap;

use super::check::{FakeDays, Scan, check};
use super::fixture::*;

const PROMO: &str = "store_days.promo_active";
const REGIONS: &[&str] = &["east", "west", "north", "south"];

fn regions() -> FakeDays {
    FakeDays::new(REGIONS)
        .group("east", &["bondi", "coogee"])
        .group("west", &["parramatta"])
        .group("north", &["manly"])
        .group("south", &["glebe"])
}

/// Registered, the wave recorded at 2 March, the groups pinned by the first
/// check, and the wave sent with its members.
async fn live() -> Fx {
    let fx = Fx::seed(GROUPED).await;
    fx.register().await;
    fx.tick(switch_at("2026-03-02")).await;
    check(
        &fx,
        &regions(),
        &Scan(None),
        switch_at("2026-03-02") + Duration::hours(1),
    )
    .await;
    fx.tick(switch_at("2026-03-02") + Duration::hours(2)).await;
    fx
}

#[tokio::test]
async fn the_first_check_pins_every_groups_members_with_the_control_groups() {
    let fx = live().await;
    let run = fx.run().await;
    assert_eq!(
        store::control_pool(&run),
        Some(vec!["north".to_string(), "south".to_string()])
    );
    let pinned = store::group_membership(&run).expect("pinned").0;
    assert_eq!(
        pinned.keys().collect::<Vec<_>>(),
        vec!["east", "north", "south", "west"],
        "every group, treated and control"
    );
    assert_eq!(pinned["east"], vec!["bondi", "coogee"]);
    assert_eq!(
        fx.wave_runs("on").await[0]["members"]["east"],
        serde_json::json!(["bondi", "coogee"])
    );
}

/// Index Review Focus 8, PR 4's half.
#[tokio::test]
async fn group_member_reclassified_mid_run_stays_in_pinned_group() {
    let fx = live().await;
    // coogee is reclassified east -> north in the warehouse, and its promo is on.
    let moved = FakeDays::new(REGIONS)
        .group("east", &["bondi"])
        .group("west", &["parramatta"])
        .group("north", &["manly", "coogee"])
        .group("south", &["glebe"])
        .set(PROMO, "coogee", &[("2026-03-02", 1.0)])
        .set(PROMO, "bondi", &[("2026-03-03", 1.0)]);
    check(&fx, &moved, &Scan(None), switch_at("2026-03-05")).await;
    let run = fx.run().await;
    assert_eq!(
        store::group_membership(&run).unwrap().0["east"],
        vec!["bondi", "coogee"],
        "pinned, not re-read"
    );
    assert_eq!(
        store::control_pool(&run),
        Some(vec!["north".to_string(), "south".to_string()])
    );
    let obs = store::observations(&run);
    assert_eq!(
        obs.member_outcomes.get("coogee"),
        Some(&FlipOutcome::Landed {
            on_day: d("2026-03-02").into()
        }),
        "read back as the east member it was randomised as"
    );
    assert!(
        !obs.control_leaks.contains_key("coogee"),
        "never checked as a north control: {obs:?}"
    );
}

#[tokio::test]
async fn a_group_flip_settles_on_its_members_share_once_all_are_decided() {
    let fx = live().await;
    let days = regions()
        .set(PROMO, "bondi", &[("2026-03-02", 0.0), ("2026-03-03", 1.0)])
        .set(PROMO, "coogee", &[("2026-03-02", 1.0)])
        .set(
            PROMO,
            "parramatta",
            &[
                ("2026-03-02", 0.0),
                ("2026-03-03", 0.0),
                ("2026-03-04", 0.0),
            ],
        );
    check(&fx, &days, &Scan(None), switch_at("2026-03-05")).await;
    let east = fx.flip("east", "on").await;
    assert_eq!(
        (east.status.as_str(), east.confirmed_on),
        (
            "confirmed",
            Some(d("2026-03-02").and_hms_opt(0, 0, 0).unwrap())
        ),
        "2 of 2, earliest landing"
    );
    let west = fx.flip("west", "on").await;
    assert_eq!(west.status, "unconfirmed");
    assert!(west.reason.unwrap().contains("0 of 1 members of `west`"));
}

#[tokio::test]
async fn a_group_with_a_member_not_yet_read_stays_undecided() {
    let fx = live().await;
    // coogee's days have not arrived.
    let days = regions().set(PROMO, "bondi", &[("2026-03-02", 1.0)]);
    check(&fx, &days, &Scan(None), switch_at("2026-03-05")).await;
    assert_eq!(fx.flip("east", "on").await.status, "dispatched");
    let obs = store::observations(&fx.run().await);
    assert_eq!(
        obs.member_outcomes.keys().collect::<Vec<_>>(),
        vec!["bondi"]
    );
}

#[tokio::test]
async fn a_control_groups_members_are_watched_for_leaks() {
    let fx = live().await;
    let days = regions()
        .set(PROMO, "manly", &[("2026-03-02", 0.0), ("2026-03-03", 0.0)])
        .set(PROMO, "glebe", &[("2026-03-02", 0.0), ("2026-03-04", 1.0)]);
    check(&fx, &days, &Scan(None), switch_at("2026-03-05")).await;
    let obs = store::observations(&fx.run().await);
    assert_eq!(
        obs.control_leaks,
        BTreeMap::from([("glebe".to_string(), d("2026-03-04"))])
    );
}

#[tokio::test]
async fn a_settled_groups_member_outcomes_are_on_the_run_before_the_check_saves() {
    // Settling the group's flip and saving its members' outcomes must not be split by a
    // failure: once settled, the group is never read again.
    let fx = live().await;
    let run = fx.run().await;
    let c = store::pinned_config(&fx.db, &run).await.unwrap();
    let flips = store::flips_of(&fx.db, run.id).await.unwrap();
    let groups = store::group_membership(&run).expect("pinned");
    let inp = ReadBackInputs {
        run: &run,
        c: &c,
        flips: &flips,
        today: d("2026-03-05"),
        now: d("2026-03-05").into(),
        membership: Some(&groups),
        fleet: None,
    };
    let days = regions().set(PROMO, "bondi", &[("2026-03-02", 1.0)]).set(
        PROMO,
        "coogee",
        &[("2026-03-03", 1.0)],
    );
    let mut obs = store::observations(&run);
    treated_groups(&fx.db, &inp, &groups, &days, &mut obs)
        .await
        .unwrap();
    assert_eq!(fx.flip("east", "on").await.status, "confirmed");
    // The check dies here: `obs` is never saved.
    drop(obs);
    let members = store::observations(&fx.run().await).member_outcomes;
    assert_eq!(
        members.get("coogee"),
        Some(&FlipOutcome::Landed {
            on_day: d("2026-03-03").into()
        }),
        "{members:?}"
    );
    assert!(members.contains_key("bondi"), "{members:?}");
}
