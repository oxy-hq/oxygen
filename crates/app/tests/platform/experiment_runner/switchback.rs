use chrono::Duration;
use serde_json::json;

use super::fixture::*;

async fn registered() -> Fx {
    let fx = Fx::seed(SWITCHBACK).await;
    fx.register().await;
    fx
}

/// A period boundary: tick on its first instant, then the worker finishes.
async fn boundary(fx: &Fx, day: &str) {
    fx.tick(switch_at(day)).await;
    fx.finish_levers("done").await;
}

#[tokio::test]
async fn a_switchback_waits_for_its_fleet_then_flips_it_due_on_the_period() {
    let fx = registered().await;
    fx.tick(switch_at("2026-03-02")).await;
    assert_eq!(fx.run().await.state, "live");
    assert!(
        fx.flips().await.is_empty(),
        "no fleet pinned yet, nothing to flip"
    );
    fx.pin_fleet(&["bondi", "glebe"]).await;
    fx.tick(switch_at("2026-03-02") + Duration::hours(2)).await;
    let flips = fx.flips().await;
    assert_eq!(flips.len(), 2);
    assert!(flips.iter().all(
        |f| f.due_at == d("2026-03-02").and_hms_opt(0, 0, 0).unwrap()
            && f.wave == 0
            && f.status == "dispatched"
    ));
    assert_eq!(
        fx.wave_runs("on").await,
        vec![
            json!({ "units": ["bondi", "glebe"], "state": "on", "experiment": SWITCHBACK_NAME, "wave": 0 })
        ]
    );
}

/// Index Review Focus 6.
#[tokio::test]
async fn switchback_flips_same_unit_on_twice_on_distinct_days() {
    let fx = registered().await;
    fx.pin_fleet(&["bondi", "glebe"]).await;
    for day in ["2026-03-02", "2026-03-09", "2026-03-16", "2026-03-23"] {
        boundary(&fx, day).await;
        fx.tick(switch_at(day) + Duration::minutes(1)).await; // a repeated tick on that date
    }
    let mut ons: Vec<_> = fx
        .flips()
        .await
        .into_iter()
        .filter(|f| f.unit == "bondi" && f.target_state == "on")
        .collect();
    ons.sort_by_key(|f| f.due_at);
    assert_eq!(
        ons.iter().map(|f| f.due_at.date()).collect::<Vec<_>>(),
        vec![d("2026-03-02"), d("2026-03-23")]
    );
    assert_ne!(
        ons[0].task_id, ons[1].task_id,
        "each on period is its own lever run"
    );
    assert_eq!(
        fx.flips().await.len(),
        8,
        "two units × four periods, nothing twice"
    );
    assert_eq!(fx.wave_runs("on").await.len(), 2);
    assert_eq!(
        fx.wave_runs("off").await.len(),
        2,
        "off, off: each boundary sent once"
    );
}

/// Review Focus 4, the switchback clause: an ended period is never replayed.
#[tokio::test]
async fn a_clock_jump_over_switchback_periods_cancels_ended_periods_and_sends_only_the_current_one()
{
    let fx = registered().await;
    fx.pin_fleet(&["bondi", "glebe"]).await;
    // The driver was down from before the first period until 17 March: periods
    // 0 (on, 2-8 March) and 1 (off, 9-15 March) are over; period 2 is current.
    fx.tick(switch_at("2026-03-17")).await;
    let flips = fx.flips().await;
    assert_eq!(
        flips.len(),
        6,
        "every elapsed boundary is recorded, for the record"
    );
    for f in &flips {
        if f.due_at < d("2026-03-16").and_hms_opt(0, 0, 0).unwrap() {
            assert_eq!(
                (f.status.as_str(), f.reason.as_deref()),
                ("cancelled", Some("period ended before dispatch")),
                "{f:?}"
            );
            assert!(f.task_id.is_none());
        } else {
            assert_eq!(f.status, "dispatched", "{f:?}");
        }
    }
    assert_eq!(
        fx.wave_runs("on").await.len(),
        0,
        "an ended on period is never replayed"
    );
    assert_eq!(
        fx.wave_runs("off").await.len(),
        1,
        "only the current period's flip goes out"
    );
    fx.tick(switch_at("2026-03-17") + Duration::minutes(5))
        .await;
    assert_eq!(
        fx.wave_runs("off").await.len(),
        1,
        "and a cancelled row stays cancelled"
    );
}

#[tokio::test]
async fn a_period_flip_waits_for_the_previous_periods_run() {
    let fx = registered().await;
    fx.pin_fleet(&["bondi", "glebe"]).await;
    fx.tick(switch_at("2026-03-02")).await; // the on run stays `running`
    fx.tick(switch_at("2026-03-09")).await;
    let offs: Vec<_> = fx
        .flips()
        .await
        .into_iter()
        .filter(|f| f.target_state == "off")
        .collect();
    assert_eq!(offs.len(), 2, "recorded on its date");
    assert!(
        offs.iter()
            .all(|f| f.status == "pending" && f.task_id.is_none()),
        "an off behind a running on could finish first and leave the lever on"
    );
    fx.finish_levers("done").await;
    fx.tick(switch_at("2026-03-09") + Duration::minutes(5))
        .await;
    assert_eq!(fx.wave_runs("off").await.len(), 1);
}

/// A scheduled `off` cancelled because its period ended was never sent: it
/// must not stand in for the abort's `off`, or the lever stays on.
#[tokio::test]
async fn a_cancelled_scheduled_off_does_not_suppress_an_abort_off() {
    let fx = registered().await;
    fx.pin_fleet(&["bondi", "glebe"]).await;
    fx.tick(switch_at("2026-03-02")).await;
    fx.finish_levers("done").await;
    // Period 1's off, dated after the last on, was cancelled unsent.
    fx.exec(
        "INSERT INTO experiment_flips (id, run_id, unit, target_state, due_at, wave, status, reason, created_at) \
         SELECT gen_random_uuid(), run_id, unit, 'off', DATE '2026-03-09', 1, 'cancelled', \
                'period ended before dispatch', created_at \
         FROM experiment_flips WHERE target_state = 'on'",
        [],
    )
    .await;
    fx.exec(
        "UPDATE experiment_runs SET abort_reason = 'owner call' WHERE workspace_id = $1",
        [fx.ws.into()],
    )
    .await;
    fx.tick(switch_at("2026-03-04")).await;
    assert_eq!(fx.run().await.state, "aborted");
    let offs: Vec<_> = fx
        .flips()
        .await
        .into_iter()
        .filter(|f| f.target_state == "off" && f.status != "cancelled")
        .collect();
    assert_eq!(offs.len(), 2, "each unit gets its terminal off: {offs:?}");
    assert!(offs.iter().all(
        |f| f.due_at == d("2026-03-04").and_hms_opt(0, 0, 0).unwrap() && f.status == "dispatched"
    ));
}
