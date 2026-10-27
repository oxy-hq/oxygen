use chrono::Duration;
use oxy_app::server::experiment::list::list_items;
use oxy_app::server::experiment::status::{
    AbortOutcome, DesignKind, ExperimentStatus, OwnerAbort, request_abort, run_status,
};
use oxy_experiment::run::{AbortLevers, LeverState};

use super::check::{FakeDays, Scan, check};
use super::fixture::*;

const ALL: &[&str] = &[
    "bondi",
    "newtown",
    "manly",
    "surry_hills",
    "glebe",
    "paddington",
];

async fn live() -> Fx {
    let fx = Fx::seed(DESIGN).await;
    fx.register().await;
    fx.tick(switch_at("2026-03-02")).await;
    fx
}

#[tokio::test]
async fn status_of_a_live_run_reports_flips_and_compliance_so_far() {
    let fx = live().await;
    let newtown = fx.flip("newtown", "on").await.agentic_run_id.unwrap();
    fx.exec(
        "UPDATE agentic_runs SET task_status = 'failed' WHERE id = $1",
        [newtown.to_string().into()],
    )
    .await;
    fx.tick(switch_at("2026-03-02") + Duration::minutes(5))
        .await;
    let days = FakeDays::new(ALL)
        .set("store_days.promo_active", "bondi", &[("2026-03-02", 1.0)])
        .fill(
            "store_days.promo_active",
            "glebe",
            d("2026-03-02"),
            d("2026-03-03"),
            0.0,
        )
        .fill(
            "store_days.promo_active",
            "paddington",
            d("2026-03-02"),
            d("2026-03-03"),
            0.0,
        );
    let at = switch_at("2026-03-03") + Duration::hours(6);
    check(&fx, &days, &Scan(None), at).await;
    // A driver tick at `at` re-arms the row, so the run is advancing as of `at`.
    fx.tick(at).await;

    let body = run_status(&fx.db, fx.ws, NAME, at)
        .await
        .unwrap()
        .expect("a status");
    assert_eq!(body.status, ExperimentStatus::Live);
    let run = body.run.as_ref().expect("a run");
    assert_eq!(
        (run.flips.sent, run.flips.confirmed, run.flips.failed),
        (2, 1, 1)
    );
    assert_eq!(run.failed_flips[0].unit, "newtown");
    let compliance = run.compliance.as_ref().expect("a treated flip is decided");
    assert_eq!(
        (compliance.c_treated, compliance.c_control_leak),
        (0.5, 0.0)
    );
    assert!(compliance.below_floor);
    assert_eq!(compliance.restated_mde, Some(35.0));
    assert_eq!(
        run.next_switch_on,
        Some(d("2026-03-16")).map(oxy_experiment::Moment::from)
    );
    assert_eq!(run.registered_by, fx.owner);
    assert!(!run.not_advancing);

    // The body is the contract the web plan reads: every field, by name.
    let json = serde_json::to_value(&body).unwrap();
    assert_eq!(body.lever_revisions, vec![fx.head().await]);
    for key in [
        "name",
        "status",
        "registration_refusal",
        "lever_revisions",
        "run",
    ] {
        assert!(json.get(key).is_some(), "{key}");
    }
    for key in [
        "state",
        "design_id",
        "registered_at",
        "registered_by",
        "next_switch_on",
        "flips",
        "failed_flips",
        "compliance",
        "coverage",
        "guardrails",
        "guardrail_breach",
        "abort_reason",
        "checked_at",
        "problems",
        "next_check_at",
        "not_advancing",
        "design",
        "periods",
        "assign_by",
        "group_membership",
        "estimate",
    ] {
        assert!(json["run"].get(key).is_some(), "run.{key}");
    }
    assert_eq!(json["status"], "live");
    assert_eq!(json["run"]["design"], "waves");
    assert_eq!(
        json["run"]["estimate"],
        serde_json::json!({ "state": "none" })
    );
    assert!(json["run"]["periods"].is_null() && json["run"]["assign_by"].is_null());
    let trigger = &json["run"]["guardrails"][0];
    assert_eq!(
        trigger["units"].as_array().map(Vec::len),
        Some(1),
        "per treated unit: bondi is live, newtown failed"
    );
    assert_eq!(trigger["units"][0]["unit"], "bondi");
}

#[tokio::test]
async fn status_of_a_switchback_shows_its_periods_and_their_progress() {
    let fx = Fx::seed(SWITCHBACK).await;
    fx.register().await;
    let at = switch_at("2026-03-02") + Duration::hours(1);
    fx.tick(switch_at("2026-03-02")).await;
    let waiting = run_status(&fx.db, fx.ws, SWITCHBACK_NAME, at)
        .await
        .unwrap()
        .unwrap()
        .run
        .unwrap();
    assert!(
        waiting
            .problems
            .iter()
            .any(|p| p.contains("fleet is not pinned")),
        "{:?}",
        waiting.problems
    );

    fx.pin_fleet(&["bondi", "glebe"]).await;
    fx.tick(at).await;
    let body = run_status(&fx.db, fx.ws, SWITCHBACK_NAME, at)
        .await
        .unwrap()
        .unwrap();
    let run = body.run.as_ref().unwrap();
    assert_eq!(run.design, DesignKind::Switchback);
    let periods = run
        .periods
        .as_ref()
        .expect("a switchback lists its periods");
    assert_eq!(periods.len(), 4);
    assert_eq!(
        (periods[0].state, periods[0].flips.sent),
        (LeverState::On, 2)
    );
    assert_eq!(periods[1].flips.sent, 0);
    let json = serde_json::to_value(&body).unwrap();
    assert_eq!(json["run"]["design"], "switchback");
    assert_eq!(json["run"]["periods"][1]["state"], "off");
    assert_eq!(json["run"]["periods"][1]["from"], "2026-03-09");
}

#[tokio::test]
async fn status_of_a_grouped_run_shows_assign_by_and_the_pinned_membership() {
    let fx = Fx::seed(GROUPED).await;
    fx.register().await;
    let membership: &[(&str, &[&str])] = &[
        ("east", &["bondi", "coogee"]),
        ("west", &["parramatta"]),
        ("north", &["manly"]),
        ("south", &["glebe"]),
    ];
    fx.pin(&["north", "south"], Some(membership)).await;
    let body = run_status(&fx.db, fx.ws, GROUPED_NAME, utc("2026-02-20T00:00:00Z"))
        .await
        .unwrap()
        .unwrap();
    let run = body.run.as_ref().unwrap();
    assert_eq!(run.assign_by.as_deref(), Some("store.region"));
    assert_eq!(
        run.group_membership.as_ref().unwrap().0["east"],
        vec!["bondi", "coogee"]
    );
    let json = serde_json::to_value(&body).unwrap();
    assert_eq!(
        json["run"]["group_membership"]["south"],
        serde_json::json!(["glebe"])
    );
}

#[tokio::test]
async fn lever_revisions_show_the_lever_changed_mid_run() {
    let fx = live().await;
    let r1 = fx.head().await;
    let r2 = fx
        .promote(utc("2026-03-10T00:00:00Z"), DESIGN, LEVERS)
        .await;
    fx.tick(switch_at("2026-03-16")).await;
    let body = run_status(&fx.db, fx.ws, NAME, switch_at("2026-03-17"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(body.lever_revisions, vec![r1, r2]);
    assert_eq!(
        serde_json::to_value(&body).unwrap()["lever_revisions"][1],
        r2.to_string()
    );
}

#[tokio::test]
async fn a_design_promoted_while_live_is_neither_shown_nor_acted_on_by_the_run() {
    let fx = live().await;
    fx.promote(
        utc("2026-03-05T00:00:00Z"),
        &DESIGN.replace("grace_days: 2", "grace_days: 5"),
        LEVERS,
    )
    .await;
    let at = switch_at("2026-03-05");
    let run = run_status(&fx.db, fx.ws, NAME, at)
        .await
        .unwrap()
        .unwrap()
        .run
        .unwrap();
    let items = list_items(&fx.db, fx.ws, at).await.unwrap();
    assert_eq!(items[0].design_id, run.design_id);
    assert!(
        items[0].design_frozen_at.is_some(),
        "the run's design is frozen"
    );
    assert_eq!(items[0].definition["lever"]["read_back"]["grace_days"], 2);
    fx.tick(switch_at("2026-03-16")).await;
    assert_eq!(fx.lever_runs("manly", "on").await, 1);
}

#[tokio::test]
async fn status_is_not_advancing_when_no_driver_claims_the_run() {
    let fx = Fx::seed(DESIGN).await;
    let run = fx.register().await;
    let due = run.next_check_at.with_timezone(&chrono::Utc);
    let fresh = run_status(&fx.db, fx.ws, NAME, due + Duration::seconds(30))
        .await
        .unwrap()
        .unwrap();
    assert!(!fresh.run.unwrap().not_advancing);
    let stale = run_status(&fx.db, fx.ws, NAME, due + Duration::seconds(61))
        .await
        .unwrap()
        .unwrap();
    assert!(stale.run.unwrap().not_advancing);
}

#[tokio::test]
async fn status_of_an_unregistered_design_reports_the_refusal_and_an_unknown_name_is_none() {
    let fx = Fx::seed(DESIGN).await;
    let body = run_status(&fx.db, fx.ws, NAME, utc("2026-02-28T00:00:00Z"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(body.status, ExperimentStatus::RegistrationRefused);
    assert!(
        body.registration_refusal
            .unwrap()
            .contains("registration deadline")
    );
    assert!(body.run.is_none());
    assert!(
        run_status(&fx.db, fx.ws, "nope", utc("2026-02-28T00:00:00Z"))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn abort_is_recorded_for_a_live_run_and_refused_after_close() {
    let fx = live().await;
    let at = switch_at("2026-03-04");
    assert_eq!(
        request_abort(&fx.db, fx.ws, NAME, off("owner call"), at)
            .await
            .unwrap(),
        AbortOutcome::Requested
    );
    assert_eq!(
        request_abort(&fx.db, fx.ws, NAME, off("second thoughts"), at)
            .await
            .unwrap(),
        AbortOutcome::Requested
    );
    let run = fx.run().await;
    assert_eq!(
        run.abort_reason.as_deref(),
        Some("owner call"),
        "the first reason stands"
    );
    assert!(
        run.next_check_at.with_timezone(&chrono::Utc) <= at,
        "pulled to now: the tick actuates"
    );
    assert_eq!(run.state, "live", "recording an abort flips nothing");

    fx.exec(
        "UPDATE experiment_runs SET state = 'closing' WHERE workspace_id = $1",
        [fx.ws.into()],
    )
    .await;
    assert_eq!(
        request_abort(&fx.db, fx.ws, NAME, off("late"), at)
            .await
            .unwrap(),
        AbortOutcome::NotAbortable {
            state: "closing".into()
        }
    );
    assert_eq!(
        request_abort(&fx.db, fx.ws, "nope", off("x"), at)
            .await
            .unwrap(),
        AbortOutcome::NoRun
    );
}

fn off(reason: &str) -> OwnerAbort<'_> {
    OwnerAbort {
        reason,
        levers: AbortLevers::Off,
    }
}

#[tokio::test]
async fn the_first_abort_request_decides_what_happens_to_the_levers() {
    let fx = live().await;
    let at = switch_at("2026-03-04");
    let keep = OwnerAbort {
        reason: "owner call",
        levers: AbortLevers::Keep,
    };
    request_abort(&fx.db, fx.ws, NAME, keep, at).await.unwrap();
    request_abort(&fx.db, fx.ws, NAME, off("second thoughts"), at)
        .await
        .unwrap();
    let run = fx.run().await;
    assert_eq!(run.abort_levers.as_deref(), Some("keep"), "with its reason");
    let body = run_status(&fx.db, fx.ws, NAME, at).await.unwrap().unwrap();
    assert_eq!(body.run.unwrap().abort_levers, Some(AbortLevers::Keep));
}

#[tokio::test]
async fn the_list_reports_registration_and_run_states() {
    let fx = Fx::seed(DESIGN).await;
    let items = list_items(&fx.db, fx.ws, utc("2026-02-10T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(items[0].name, NAME);
    assert_eq!(items[0].status, ExperimentStatus::Registrable);
    assert_eq!(items[0].next_switch_on.as_deref(), Some("2026-03-02"));
    fx.register().await;
    fx.tick(switch_at("2026-03-02")).await;
    let items = list_items(&fx.db, fx.ws, switch_at("2026-03-03"))
        .await
        .unwrap();
    assert_eq!(items[0].status, ExperimentStatus::Live);
    assert_eq!(items[0].next_switch_on.as_deref(), Some("2026-03-16"));

    let late = Fx::seed(DESIGN).await;
    let items = list_items(&late.db, late.ws, utc("2026-02-28T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(items[0].status, ExperimentStatus::RegistrationRefused);
    assert!(
        items[0]
            .registration_refusal
            .as_deref()
            .unwrap()
            .contains("registration deadline")
    );
}

#[tokio::test]
async fn a_lever_fixed_after_the_deadline_is_refused_in_status_and_list_and_never_registers() {
    // The lever is refused before the deadline and back only after the
    // 2026-02-26T13:00Z deadline.
    let fx = Fx::seed_with_levers(DESIGN, &["promo_start", "promo_off"]).await;
    fx.promote_bare(utc("2026-02-28T00:00:00Z"), LEVERS).await;
    let now = utc("2026-02-28T01:00:00Z");

    let body = run_status(&fx.db, fx.ws, NAME, now).await.unwrap().unwrap();
    assert_eq!(body.status, ExperimentStatus::RegistrationRefused);
    let reason = body.registration_refusal.unwrap();
    assert!(
        reason.contains("not before the registration deadline"),
        "{reason}"
    );
    let items = list_items(&fx.db, fx.ws, now).await.unwrap();
    assert_eq!(items[0].status, ExperimentStatus::RegistrationRefused);
    assert_eq!(
        items[0].registration_refusal.as_deref(),
        Some(reason.as_str())
    );
    assert!(fx.register_at(now).await.is_err());
    assert!(fx.run_opt().await.is_none(), "a refusal is never a run");
}

/// Statements `list_items` sends, counted on a handle of its own.
async fn list_queries(fx: &Fx, at: chrono::DateTime<chrono::Utc>) -> (usize, usize) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let count = Arc::new(AtomicUsize::new(0));
    let mut counted = fx.db.clone();
    let seen = count.clone();
    counted.set_metric_callback(move |_| {
        seen.fetch_add(1, Ordering::Relaxed);
    });
    let items = list_items(&counted, fx.ws, at).await.expect("list");
    (count.load(Ordering::Relaxed), items.len())
}

#[tokio::test]
async fn the_list_costs_the_same_for_two_experiments_or_six() {
    let fx = Fx::seed(DESIGN).await;
    fx.register().await;
    let at = utc("2026-02-02T00:00:00Z");
    let copy = |n: usize| DESIGN.replace(NAME, &format!("{NAME}_{n}"));
    seed_design(&fx.db, fx.ws, fx.owner, &copy(1)).await;
    let (two, listed) = list_queries(&fx, at).await;
    assert_eq!(listed, 2);
    for n in 2..=5 {
        seed_design(&fx.db, fx.ws, fx.owner, &copy(n)).await;
    }
    let (six, listed) = list_queries(&fx, at).await;
    assert_eq!(listed, 6);
    assert_eq!(
        six, two,
        "a statement per experiment: {two} for two, {six} for six"
    );
}
