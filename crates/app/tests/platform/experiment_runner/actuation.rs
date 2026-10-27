use chrono::{Days, Duration, NaiveDate, Utc};
use oxy_app::server::experiment::dispatch::{self, RunCtx};
use oxy_app::server::experiment::status::{self as run_status, OwnerAbort};
use oxy_experiment::run::{AbortLevers, LeverState, RunAction, RunState, Transition};
use oxy_experiment::timeline::{day_start_utc, last_post_window_end_date};
use serde_json::json;
use uuid::Uuid;

use super::fixture::*;

async fn live() -> Fx {
    let fx = Fx::seed(DESIGN).await;
    fx.register().await;
    fx.tick(switch_at("2026-03-02")).await;
    fx
}

async fn request_abort(fx: &Fx, reason: &str) {
    fx.exec(
        "UPDATE experiment_runs SET abort_reason = $1 WHERE workspace_id = $2",
        [reason.into(), fx.ws.into()],
    )
    .await;
}

/// Put the run in `closing` since the design's exclusive end, with `coverage`.
async fn closing(fx: &Fx, coverage: Option<f64>) -> NaiveDate {
    let end = last_post_window_end_date(&fx.design().await).expect("an end");
    fx.exec(
        "UPDATE experiment_runs SET state = 'closing', closing_since = $1, observations = $2 \
         WHERE workspace_id = $3",
        [
            end.into(),
            json!({ "coverage": coverage }).into(),
            fx.ws.into(),
        ],
    )
    .await;
    end
}

fn sydney_start(day: NaiveDate) -> chrono::DateTime<Utc> {
    day_start_utc(day, chrono_tz::Australia::Sydney)
}

#[tokio::test]
async fn abort_turns_off_every_dispatched_unit_once() {
    let fx = live().await;
    let at = switch_at("2026-03-02");
    fx.finish_levers("done").await;
    request_abort(&fx, "owner call").await;
    fx.tick(at + Duration::hours(1)).await;
    assert_eq!(fx.run().await.state, "aborted");
    assert_eq!(fx.run().await.abort_levers.as_deref(), Some("off"));
    for unit in ["bondi", "newtown"] {
        assert_eq!(fx.lever_runs(unit, "off").await, 1, "{unit}");
    }
    let notices = fx.notices().await;
    assert_eq!(notices.len(), 1);
    assert_eq!(
        notices[0].body.as_deref(),
        Some("aborted on request: owner call")
    );

    fx.tick(at + Duration::hours(2)).await;
    fx.tick(switch_at("2026-03-16")).await;
    assert_eq!(fx.lever_runs("bondi", "off").await, 1, "once");
    assert_eq!(
        fx.lever_runs("manly", "on").await,
        0,
        "an aborted run starts no later wave"
    );
    assert_eq!(fx.notices().await.len(), 1);
}

async fn request_abort_keeping_levers(fx: &Fx, at: chrono::DateTime<Utc>) {
    let req = OwnerAbort {
        reason: "owner call",
        levers: AbortLevers::Keep,
    };
    run_status::request_abort(&fx.db, fx.ws, NAME, req, at)
        .await
        .unwrap();
}

#[tokio::test]
async fn an_abort_that_keeps_the_levers_sends_no_off_and_records_its_day() {
    let fx = live().await;
    let at = switch_at("2026-03-04");
    fx.finish_levers("done").await;
    request_abort_keeping_levers(&fx, at).await;
    fx.tick(at + Duration::hours(1)).await;
    let run = fx.run().await;
    assert_eq!(run.state, "aborted");
    assert_eq!(run.abort_levers.as_deref(), Some("keep"));
    assert_eq!(run.aborted_on, Some(d("2026-03-04")));
    for unit in ["bondi", "newtown"] {
        assert_eq!(fx.lever_runs(unit, "off").await, 0, "{unit} left on");
    }
    assert_eq!(
        fx.notices().await[0].body.as_deref(),
        Some("aborted on request, levers left as they were: owner call")
    );

    // A breach a running check records afterwards does not take the units back.
    fx.exec(
        "UPDATE experiment_runs SET guardrail_breach = 'late' WHERE workspace_id = $1",
        [fx.ws.into()],
    )
    .await;
    fx.tick(switch_at("2026-03-16")).await;
    assert_eq!(fx.lever_runs("bondi", "off").await, 0);
    assert_eq!(fx.lever_runs("manly", "on").await, 0, "no later wave");
}

#[tokio::test]
async fn a_breach_turns_the_levers_off_even_when_the_owner_asked_to_keep_them() {
    let fx = live().await;
    let at = switch_at("2026-03-04");
    fx.finish_levers("done").await;
    request_abort_keeping_levers(&fx, at).await;
    fx.exec(
        "UPDATE experiment_runs SET guardrail_breach = 'refund_rate spiked' WHERE workspace_id = $1",
        [fx.ws.into()],
    )
    .await;
    fx.tick(at + Duration::hours(1)).await;
    let run = fx.run().await;
    assert_eq!(run.state, "aborted");
    assert_eq!(
        run.abort_levers.as_deref(),
        Some("off"),
        "what the abort did"
    );
    for unit in ["bondi", "newtown"] {
        assert_eq!(fx.lever_runs(unit, "off").await, 1, "{unit}");
    }
}

#[tokio::test]
async fn abort_off_waits_for_the_units_on_run_to_finish() {
    let fx = live().await; // the `on` runs are still `running`
    let at = switch_at("2026-03-02") + Duration::minutes(1);
    request_abort(&fx, "owner call").await;
    fx.tick(at).await;
    assert_eq!(fx.run().await.state, "aborted");
    let off = fx.flip("bondi", "off").await;
    assert_eq!((off.status.as_str(), off.task_id), ("pending", None));
    assert_eq!(
        fx.lever_runs("bondi", "off").await,
        0,
        "an off behind a live on could finish first"
    );
    assert!(
        fx.run().await.next_check_at.with_timezone(&Utc) <= at + Duration::minutes(5),
        "a deferred off keeps the run on the short interval"
    );

    fx.finish_levers("done").await;
    fx.tick(at + Duration::minutes(10)).await;
    assert_eq!(fx.lever_runs("bondi", "off").await, 1);
    assert_eq!(fx.lever_runs("newtown", "off").await, 1);
}

#[tokio::test]
async fn abort_cancels_on_flips_that_never_left() {
    let fx = Fx::seed(DESIGN).await;
    let run = fx.register().await;
    fx.exec(
        "INSERT INTO experiment_flips (id, run_id, unit, target_state, due_at, wave, status, created_at) \
         VALUES ($1, $2, 'bondi', 'on', $3, 0, 'pending', $4)",
        [
            Uuid::new_v4().into(),
            run.id.into(),
            d("2026-03-02").into(),
            switch_at("2026-03-02").fixed_offset().into(),
        ],
    )
    .await;
    request_abort(&fx, "owner call").await;
    fx.tick(switch_at("2026-03-02")).await;
    assert_eq!(fx.run().await.state, "aborted");
    assert_eq!(fx.flip("bondi", "on").await.status, "cancelled");
    assert_eq!(fx.lever_runs("bondi", "on").await, 0);
    assert!(
        fx.flips().await.iter().all(|f| f.target_state == "on"),
        "nothing was on"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_terminal_off_is_recorded_once_across_midnight() {
    // A tick decided the abort late on 4 March and stalled past its lease; the
    // replica that took the run over decided it again on 5 March. The dates
    // differ, so the unique key does not meet them; the run-row lock and "no
    // off since the last on" do.
    let fx = live().await;
    fx.finish_levers("done").await;
    let (run, c) = (fx.run().await, fx.design().await);
    let decide = |unit: &'static str, day: &'static str| {
        let (run, c, db) = (run.clone(), c.clone(), fx.db.clone());
        async move {
            let ctx = RunCtx {
                run: &run,
                c: &c,
                local: d(day).into(),
                now: switch_at(day),
            };
            let off = RunAction::DispatchFlip {
                unit: unit.into(),
                state: LeverState::Off,
                wave: 0,
                due_at: d(day).into(),
            };
            let t = Transition {
                next: RunState::Aborted,
                actions: vec![off],
            };
            dispatch::apply(&db, &ctx, &t).await.expect("apply");
        }
    };
    // Sequential decisions first, then 16 racers (below).
    decide("bondi", "2026-03-04").await;
    decide("bondi", "2026-03-05").await;
    // Many replicas deciding at once, each on its own day: only the run-row
    // lock keeps all but one out.
    let days: Vec<String> = (6..=21).map(|n| format!("2026-03-{n:02}")).collect();
    let racers: Vec<_> = days
        .iter()
        .map(|day| tokio::spawn(decide("newtown", Box::leak(day.clone().into_boxed_str()))))
        .collect();
    for r in racers {
        r.await.expect("racer");
    }
    let offs: Vec<_> = fx
        .flips()
        .await
        .into_iter()
        .filter(|f| f.target_state == "off")
        .collect();
    let of = |u: &str| offs.iter().filter(|f| f.unit == u).count();
    assert_eq!((of("bondi"), of("newtown")), (1, 1), "{offs:?}");
    let bondi = offs.iter().find(|f| f.unit == "bondi").unwrap();
    assert_eq!(
        bondi.due_at,
        d("2026-03-04").and_hms_opt(0, 0, 0).unwrap(),
        "the first decision stands"
    );
}

#[tokio::test]
async fn closing_enqueues_the_estimate_exactly_once() {
    let fx = live().await;
    let end = closing(&fx, Some(0.95)).await;
    fx.tick(sydney_start(end + Days::new(1))).await;
    fx.tick(sydney_start(end + Days::new(1)) + Duration::minutes(5))
        .await;
    assert_eq!(fx.queued_kind("experiment_estimate").await, 1);
    assert_eq!(fx.run().await.state, "closing");
}

#[tokio::test]
async fn closing_waits_for_coverage_then_estimates_with_one_notice() {
    let fx = live().await;
    let end = closing(&fx, None).await;
    fx.tick(sydney_start(end + Days::new(3))).await;
    assert_eq!(fx.queued_kind("experiment_estimate").await, 0);
    fx.tick(sydney_start(end + Days::new(14))).await;
    fx.tick(sydney_start(end + Days::new(15))).await;
    assert_eq!(fx.queued_kind("experiment_estimate").await, 1);
    let notices = fx.notices().await;
    assert_eq!(notices.len(), 1);
    assert!(
        notices[0]
            .body
            .as_deref()
            .unwrap()
            .contains("coverage_floor 0.90")
    );
}

/// Both waves flipped and their lever runs finished, then the run closes ON
/// the exclusive end: the tick moves it `live -> closing`.
async fn closed(fx: &Fx) -> NaiveDate {
    let end = last_post_window_end_date(&fx.design().await).expect("an end");
    fx.tick(switch_at("2026-03-16")).await;
    fx.finish_levers("done").await;
    fx.tick(sydney_start(end)).await;
    assert_eq!(fx.run().await.state, "closing");
    end
}

#[tokio::test]
async fn entering_closing_turns_the_levers_off_before_any_estimate_exists() {
    let fx = live().await;
    let end = closed(&fx).await;
    for unit in ["bondi", "newtown", "manly", "surry_hills"] {
        assert_eq!(fx.lever_runs(unit, "off").await, 1, "{unit}: on_close: off");
    }
    assert!(fx.run().await.estimate_task_id.is_none());
    assert_eq!(fx.queued_kind("experiment_estimate").await, 0);
    // A stuck closing neither repeats nor drops them.
    fx.tick(sydney_start(end + Days::new(1))).await;
    assert_eq!(fx.lever_runs("bondi", "off").await, 1);
}

#[tokio::test]
async fn a_close_off_waits_for_the_units_running_on_then_goes_out_once() {
    let fx = live().await;
    let end = last_post_window_end_date(&fx.design().await).expect("an end");
    fx.tick(switch_at("2026-03-16")).await; // wave 2's `on`s: still running
    fx.tick(sydney_start(end)).await;
    assert_eq!(fx.run().await.state, "closing");
    let off = fx.flip("bondi", "off").await;
    assert_eq!((off.status.as_str(), off.task_id), ("pending", None));
    assert_eq!(fx.lever_runs("bondi", "off").await, 0, "behind a live on");

    fx.finish_levers("done").await;
    fx.tick(sydney_start(end) + Duration::minutes(10)).await;
    fx.tick(sydney_start(end) + Duration::minutes(20)).await;
    for unit in ["bondi", "newtown", "manly", "surry_hills"] {
        assert_eq!(fx.lever_runs(unit, "off").await, 1, "{unit}: sent once");
    }
}

#[tokio::test]
async fn an_abort_racing_the_move_to_closing_wins_and_each_unit_is_turned_off_once() {
    let fx = live().await;
    fx.finish_levers("done").await;
    let end = last_post_window_end_date(&fx.design().await).expect("an end");
    request_abort(&fx, "owner call").await;
    fx.tick(sydney_start(end)).await; // the day the run would have closed
    assert_eq!(fx.run().await.state, "aborted");
    fx.tick(sydney_start(end) + Duration::minutes(10)).await;
    for unit in ["bondi", "newtown"] {
        assert_eq!(
            fx.lever_runs(unit, "off").await,
            1,
            "{unit}: one off, not two"
        );
    }
    let offs = fx
        .flips()
        .await
        .into_iter()
        .filter(|f| f.target_state == "off")
        .count();
    assert_eq!(offs, 2, "no close off beside the abort off");
}

/// The retry after the backoff, and the one notice when retries run out, are
/// PR 5's `experiment_results::estimate_retry`.
#[tokio::test]
async fn a_failed_estimate_leaves_the_levers_off_and_shows_in_status() {
    let fx = live().await;
    let end = closed(&fx).await;
    fx.exec(
        "UPDATE experiment_runs SET observations = $1 WHERE workspace_id = $2",
        [json!({ "coverage": 1.0 }).into(), fx.ws.into()],
    )
    .await;
    fx.tick(sydney_start(end + Days::new(1))).await;
    let estimate = fx.run().await.estimate_task_id.expect("estimate enqueued");
    let at = sydney_start(end + Days::new(2));
    assert_eq!(
        serde_json::to_value(fx.status_at(at).await.run.expect("a run").estimate).unwrap()["state"],
        "running"
    );
    fx.exec(
        "UPDATE agentic_runs SET task_status = 'failed', error_message = 'warehouse down', \
         updated_at = $2 WHERE id = $1",
        [estimate.to_string().into(), at.fixed_offset().into()],
    )
    .await;
    fx.tick(at + Duration::minutes(5)).await;
    fx.tick(at + Duration::minutes(10)).await;
    assert_eq!(fx.run().await.state, "closing");
    assert_eq!(
        fx.queued_kind("experiment_estimate").await,
        1,
        "not re-enqueued inside the backoff"
    );
    for unit in ["bondi", "newtown", "manly", "surry_hills"] {
        assert_eq!(fx.lever_runs(unit, "off").await, 1, "{unit} stays off");
    }
    assert!(
        !fx.notices()
            .await
            .iter()
            .any(|n| n.body.as_deref().is_some_and(|b| b.contains("estimate"))),
        "no notice while a retry is pending"
    );
    let json = serde_json::to_value(fx.status_at(at).await.run.expect("a run")).unwrap();
    assert_eq!(json["estimate"]["state"], "failed");
    assert_eq!(json["estimate"]["task_status"], "failed");
    assert_eq!(json["estimate"]["error"], "warehouse down");
    assert_eq!(json["estimate"]["attempts"], 1);
}

#[tokio::test]
async fn a_reported_run_sends_nothing_new_its_offs_went_at_closing() {
    let fx = live().await;
    let end = closed(&fx).await;
    fx.exec(
        "UPDATE experiment_runs SET observations = $1 WHERE workspace_id = $2",
        [json!({ "coverage": 1.0 }).into(), fx.ws.into()],
    )
    .await;
    fx.tick(sydney_start(end + Days::new(1))).await;
    let estimate = fx.run().await.estimate_task_id.expect("estimate enqueued");
    // PR 5's executor finishes the job only after writing the result row.
    fx.exec(
        "UPDATE agentic_runs SET task_status = 'done' WHERE id = $1",
        [estimate.to_string().into()],
    )
    .await;
    fx.tick(sydney_start(end + Days::new(2))).await;
    assert_eq!(fx.run().await.state, "reported");
    fx.tick(sydney_start(end + Days::new(3))).await;
    for unit in ["bondi", "newtown", "manly", "surry_hills"] {
        assert_eq!(fx.lever_runs(unit, "off").await, 1, "{unit}");
    }
}

#[tokio::test]
async fn an_abort_recorded_after_the_tick_read_the_run_beats_its_move_to_closing() {
    use oxy_app::server::experiment::store::{self, Persist};
    let fx = live().await;
    let run = fx.run().await;
    // The tick read a live run, then the owner aborted before it persisted.
    request_abort(&fx, "stop it").await;
    let now = utc("2026-03-03T00:00:00Z");
    let moved = store::persist(
        &fx.db,
        run.id,
        &Persist {
            prev: RunState::Live,
            next: RunState::Closing,
            abort_levers: None,
            local: now.date_naive().into(),
            next_check_at: now + Duration::hours(1),
            claimed_until: now + Duration::minutes(5),
            now,
        },
    )
    .await
    .unwrap();
    assert!(!moved, "the abort wins: the stale transition is refused");
    assert_eq!(fx.run().await.state, "live");
}
