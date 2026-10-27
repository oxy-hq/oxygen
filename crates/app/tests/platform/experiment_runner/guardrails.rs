use chrono::{Duration, Utc};
use oxy_app::server::experiment::store::{self, GuardrailState};
use oxy_app::server::experiment::tick::tick_experiments;

use super::check::{FakeDays, Scan, check};
use super::fixture::*;

const LABOR: &str = "store_days.labor_cost_pct";
const ALL: &[&str] = &[
    "bondi",
    "newtown",
    "manly",
    "surry_hills",
    "glebe",
    "paddington",
];
const TRIGGER: &str = "guardrails:\n  - criteria: { type: trigger, when: \"{{store_days.labor_cost_pct}} > 0.34\" }\n    action: abort\n";

async fn live(design: &str) -> Fx {
    let fx = Fx::seed(design).await;
    fx.register().await;
    fx.tick(switch_at("2026-03-02")).await;
    fx
}

fn with_anomaly() -> String {
    DESIGN.replace(
        "  - criteria: { type: trigger",
        "  - criteria: { type: anomaly, measure: store_days.refund_rate, significance: 0.95 }\n    action: abort\n  - criteria: { type: trigger",
    )
}

#[tokio::test]
async fn a_trigger_breach_while_live_aborts_the_run_on_the_next_tick() {
    let fx = live(DESIGN).await;
    fx.finish_levers("done").await;
    let at = switch_at("2026-03-05") + Duration::hours(12);
    let days = FakeDays::new(ALL).set(
        LABOR,
        "bondi",
        &[("2026-03-03", 0.30), ("2026-03-04", 0.40)],
    );
    check(&fx, &days, &Scan(None), at).await;

    let run = fx.run().await;
    let breach = run.guardrail_breach.clone().expect("a breach");
    assert!(breach.contains("held at `bondi` on 2026-03-04"), "{breach}");
    assert!(
        run.next_check_at.with_timezone(&Utc) <= at,
        "the next tick is pulled to now"
    );

    assert_eq!(tick_experiments(&fx.db, at).await, 1, "due without forcing");
    assert_eq!(fx.run().await.state, "aborted");
    assert_eq!(fx.lever_runs("bondi", "off").await, 1);
    let notices = fx.notices().await;
    assert!(
        notices[0]
            .body
            .as_deref()
            .unwrap()
            .starts_with("guardrail breached: ")
    );
}

#[tokio::test]
async fn a_trigger_never_reads_todays_partly_loaded_day() {
    let fx = live(DESIGN).await;
    let at = switch_at("2026-03-05") + Duration::hours(12);
    // Today (5 March) has loaded a breaching value; yesterday's is fine.
    let days = FakeDays::new(ALL)
        .set(
            LABOR,
            "bondi",
            &[("2026-03-04", 0.30), ("2026-03-05", 0.50)],
        )
        .set(
            LABOR,
            "newtown",
            &[("2026-03-04", 0.30), ("2026-03-05", 0.50)],
        );
    check(&fx, &days, &Scan(None), at).await;
    let run = fx.run().await;
    assert!(
        run.guardrail_breach.is_none(),
        "today is not evidence: {:?}",
        run.guardrail_breach
    );
    assert_eq!(
        store::observations(&run).guardrails[0].status,
        GuardrailState::Ok
    );

    // Yesterday breaches: that one is read.
    let days = FakeDays::new(ALL).set(
        LABOR,
        "bondi",
        &[("2026-03-04", 0.40), ("2026-03-05", 0.10)],
    );
    check(&fx, &days, &Scan(None), at + Duration::hours(1)).await;
    let breach = fx.run().await.guardrail_breach.expect("a breach");
    assert!(breach.contains("held at `bondi` on 2026-03-04"), "{breach}");
}

#[tokio::test]
async fn a_breach_names_the_one_unit_whose_own_values_crossed() {
    let fx = live(DESIGN).await;
    let days = FakeDays::new(ALL)
        .set(LABOR, "bondi", &[("2026-03-04", 0.30)])
        .set(LABOR, "newtown", &[("2026-03-04", 0.41)]);
    check(
        &fx,
        &days,
        &Scan(None),
        switch_at("2026-03-05") + Duration::hours(12),
    )
    .await;
    let run = fx.run().await;
    assert!(run.guardrail_breach.unwrap().contains("at `newtown`"));
    let trigger = &store::observations(&fx.run().await).guardrails[0];
    let per_unit: Vec<_> = trigger
        .units
        .iter()
        .map(|u| (u.unit.as_str(), u.status))
        .collect();
    assert_eq!(
        per_unit,
        vec![
            ("bondi", GuardrailState::Ok),
            ("newtown", GuardrailState::Breached)
        ]
    );
}

#[tokio::test]
async fn a_trigger_reads_the_last_day_that_arrived_never_a_missing_one() {
    let fx = live(DESIGN).await;
    // bondi's 4 March has not arrived; its 3 March is the reading. newtown has none.
    let days = FakeDays::new(ALL).set(LABOR, "bondi", &[("2026-03-03", 0.30)]);
    check(
        &fx,
        &days,
        &Scan(None),
        switch_at("2026-03-04") + Duration::hours(12),
    )
    .await;
    let run = fx.run().await;
    assert!(
        run.guardrail_breach.is_none(),
        "a missing day is not a breach"
    );
    let trigger = &store::observations(&run).guardrails[0];
    assert_eq!(
        trigger.status,
        GuardrailState::NotChecked,
        "no reading is never healthy"
    );
    assert_eq!(
        trigger.detail.as_deref(),
        Some("1 of 2 treated units not checked")
    );
    assert_eq!(
        trigger.units[0].status,
        GuardrailState::Ok,
        "bondi, on the day that arrived"
    );
}

#[tokio::test]
async fn an_anomaly_breach_is_recorded_from_the_scan_and_names_the_unit() {
    let fx = live(&with_anomaly()).await;
    let detail = "observed 0.2100, expected 0.0400 (band 0.0100 to 0.0700)";
    check(
        &fx,
        &FakeDays::new(ALL),
        &Scan(Some(detail.into())),
        switch_at("2026-03-06"),
    )
    .await;
    let run = fx.run().await;
    let breach = run.guardrail_breach.clone().expect("a breach");
    assert!(
        breach.starts_with("anomaly on `store_days.refund_rate` at `bondi` on 2026-03-02: "),
        "{breach}"
    );
    assert!(breach.ends_with(detail));
    let statuses = store::observations(&run).guardrails;
    assert_eq!(statuses[0].status, GuardrailState::Breached);
    assert_eq!(statuses[0].units.len(), 2, "one scan per treated unit");
    assert_eq!(
        statuses[1].status,
        GuardrailState::NotChecked,
        "the trigger had no reading"
    );
}

#[tokio::test]
async fn guardrails_are_judged_per_group_over_its_pinned_members() {
    let fx = Fx::seed(&format!("{GROUPED}{TRIGGER}")).await;
    fx.register().await;
    fx.tick(switch_at("2026-03-02")).await;
    let regions = FakeDays::new(&["east", "west", "north", "south"])
        .group("east", &["bondi", "coogee"])
        .group("west", &["parramatta"])
        .group("north", &["manly"])
        .group("south", &["glebe"]);
    check(
        &fx,
        &regions,
        &Scan(None),
        switch_at("2026-03-02") + Duration::hours(1),
    )
    .await;
    fx.tick(switch_at("2026-03-02") + Duration::hours(2)).await;
    let days = FakeDays::new(&["east", "west", "north", "south"])
        .set_over(LABOR, &["bondi", "coogee"], &[("2026-03-04", 0.40)])
        .set(LABOR, "parramatta", &[("2026-03-04", 0.20)]);
    check(
        &fx,
        &days,
        &Scan(None),
        switch_at("2026-03-05") + Duration::hours(12),
    )
    .await;
    let breach = fx.run().await.guardrail_breach.expect("a breach");
    assert!(
        breach.contains("held at `east` on 2026-03-04"),
        "the group, read over its members: {breach}"
    );
}

#[tokio::test]
async fn a_switchback_off_period_day_never_trips_a_guardrail() {
    let fx = Fx::seed(&format!("{SWITCHBACK}{TRIGGER}")).await;
    fx.register().await;
    fx.pin_fleet(&["bondi", "glebe"]).await;
    fx.tick(switch_at("2026-03-02")).await;
    fx.finish_levers("done").await;
    fx.tick(switch_at("2026-03-09")).await;
    // glebe crosses on 10 March, inside an off period; its on-period reading is fine.
    let days = FakeDays::new(&["bondi", "glebe"])
        .set(
            LABOR,
            "glebe",
            &[("2026-03-05", 0.20), ("2026-03-10", 0.50)],
        )
        .set(LABOR, "bondi", &[("2026-03-05", 0.20)]);
    check(
        &fx,
        &days,
        &Scan(None),
        switch_at("2026-03-10") + Duration::hours(12),
    )
    .await;
    let run = fx.run().await;
    assert!(run.guardrail_breach.is_none(), "{:?}", run.guardrail_breach);
    assert_eq!(
        store::observations(&run).guardrails[0].status,
        GuardrailState::Ok
    );
}

#[tokio::test]
async fn guardrails_are_not_evaluated_before_any_unit_is_live() {
    let fx = Fx::seed(&with_anomaly()).await;
    fx.register().await;
    check(
        &fx,
        &FakeDays::new(ALL),
        &Scan(Some("x".into())),
        switch_at("2026-02-20"),
    )
    .await;
    let run = fx.run().await;
    assert!(run.guardrail_breach.is_none());
    assert!(
        store::observations(&run).guardrails.is_empty(),
        "a registered run is not watched"
    );
}

#[tokio::test]
async fn a_clean_check_never_erases_a_breach() {
    let fx = live(DESIGN).await;
    let hot = FakeDays::new(ALL).set(LABOR, "bondi", &[("2026-03-04", 0.40)]);
    check(
        &fx,
        &hot,
        &Scan(None),
        switch_at("2026-03-05") + Duration::hours(12),
    )
    .await;
    let cool = FakeDays::new(ALL).set(LABOR, "bondi", &[("2026-03-05", 0.10)]);
    check(
        &fx,
        &cool,
        &Scan(None),
        switch_at("2026-03-06") + Duration::hours(12),
    )
    .await;
    assert!(fx.run().await.guardrail_breach.is_some());
}

#[tokio::test]
async fn a_breach_after_close_is_not_evaluated() {
    let fx = live(DESIGN).await;
    fx.exec(
        "UPDATE experiment_runs SET state = 'closing' WHERE workspace_id = $1",
        [fx.ws.into()],
    )
    .await;
    let hot = FakeDays::new(ALL).set(LABOR, "bondi", &[("2026-03-04", 0.40)]);
    check(
        &fx,
        &hot,
        &Scan(None),
        switch_at("2026-03-04") + Duration::hours(12),
    )
    .await;
    assert!(fx.run().await.guardrail_breach.is_none());
}

#[tokio::test]
async fn a_ratio_trigger_reads_numerator_over_denominator_per_unit_day() {
    // ACoS-shaped: cost / revenue above 0.34, written inline rather than as a measure.
    let design = DESIGN.replace(
        "{{store_days.labor_cost_pct}} > 0.34",
        "{{store_days.labor_cost}} / {{store_days.revenue}} > 0.34",
    );
    let fx = live(&design).await;
    let days = FakeDays::new(ALL)
        .set("store_days.labor_cost", "bondi", &[("2026-03-04", 30.0)])
        .set("store_days.revenue", "bondi", &[("2026-03-04", 100.0)])
        .set("store_days.labor_cost", "newtown", &[("2026-03-04", 40.0)])
        .set("store_days.revenue", "newtown", &[("2026-03-04", 100.0)]);
    check(
        &fx,
        &days,
        &Scan(None),
        switch_at("2026-03-05") + Duration::hours(12),
    )
    .await;
    let run = fx.run().await;
    let breach = run.guardrail_breach.expect("newtown's 0.40 crosses 0.34");
    assert!(
        breach.contains("at `newtown` on 2026-03-04 (value 0.4)"),
        "{breach}"
    );
    let trigger = &store::observations(&fx.run().await).guardrails[0];
    assert_eq!(
        trigger.units[0].status,
        GuardrailState::Ok,
        "bondi's 0.30 holds"
    );
}

#[tokio::test]
async fn a_ratio_day_with_a_zero_denominator_is_no_reading_not_a_breach() {
    let design = DESIGN.replace(
        "{{store_days.labor_cost_pct}} > 0.34",
        "{{store_days.labor_cost}} / {{store_days.revenue}} > 0.34",
    );
    let fx = live(&design).await;
    let days = FakeDays::new(ALL)
        .set(
            "store_days.labor_cost",
            "bondi",
            &[("2026-03-03", 10.0), ("2026-03-04", 50.0)],
        )
        .set(
            "store_days.revenue",
            "bondi",
            &[("2026-03-03", 100.0), ("2026-03-04", 0.0)],
        );
    check(
        &fx,
        &days,
        &Scan(None),
        switch_at("2026-03-05") + Duration::hours(12),
    )
    .await;
    let run = fx.run().await;
    assert!(run.guardrail_breach.is_none(), "{:?}", run.guardrail_breach);
    let trigger = &store::observations(&run).guardrails[0];
    assert_eq!(
        trigger.units[0].status,
        GuardrailState::Ok,
        "the last day with a reading (0.10) holds"
    );
}
