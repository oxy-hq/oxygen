use chrono::Duration;
use oxy_app::server::experiment::dispatch::{self, RunCtx};
use oxy_app::server::experiment::tick::tick_experiments;
use serde_json::json;

use super::fixture::*;

fn per_wave() -> String {
    DESIGN.replace("  on_close: off", "  per: wave\n  on_close: off")
}

async fn registered(design: &str) -> Fx {
    let fx = Fx::seed(design).await;
    fx.register().await;
    fx
}

#[tokio::test]
async fn per_wave_sends_one_lever_run_carrying_the_waves_units() {
    let fx = registered(&per_wave()).await;
    fx.tick(switch_at("2026-03-02")).await;
    let flips = fx.flips().await;
    assert_eq!(
        flips.len(),
        2,
        "one row per unit: compliance stays per unit"
    );
    assert!(
        flips
            .iter()
            .all(|f| f.status == "dispatched" && f.task_id == flips[0].task_id)
    );
    assert_eq!(
        fx.wave_runs("on").await,
        vec![
            json!({ "units": ["bondi", "newtown"], "state": "on", "experiment": NAME, "wave": 0 })
        ]
    );
}

#[tokio::test]
async fn double_tick_sends_one_task_per_wave() {
    let fx = registered(&per_wave()).await;
    let at = switch_at("2026-03-02");
    fx.make_due(at).await;
    let (a, b) = tokio::join!(tick_experiments(&fx.db, at), tick_experiments(&fx.db, at));
    assert_eq!(a + b, 1, "exactly one replica claims the run");
    fx.tick(at + Duration::minutes(1)).await;
    fx.tick(at + Duration::minutes(2)).await;
    assert_eq!(fx.flips().await.len(), 2);
    assert_eq!(fx.wave_runs("on").await.len(), 1);
}

#[tokio::test]
async fn an_orphaned_wave_is_sent_whole_as_one_task() {
    let fx = registered(&per_wave()).await;
    // A replica recorded the wave (one transaction) and died before sending it.
    fx.orphan(&["bondi", "newtown"], "on", 0, "2026-03-02")
        .await;
    fx.tick(switch_at("2026-03-02")).await;
    let flips = fx.flips().await;
    assert!(
        flips
            .iter()
            .all(|f| f.status == "dispatched" && f.task_id == flips[0].task_id)
    );
    assert_eq!(fx.wave_runs("on").await.len(), 1, "re-sent whole");
    fx.tick(switch_at("2026-03-02") + Duration::minutes(5))
        .await;
    assert_eq!(fx.wave_runs("on").await.len(), 1, "never duplicated");
}

#[tokio::test]
async fn two_senders_racing_on_one_wave_send_it_once() {
    let fx = registered(&per_wave()).await;
    fx.orphan(&["bondi", "newtown"], "on", 0, "2026-03-02")
        .await;
    let (run, c) = (fx.run().await, fx.design().await);
    let ctx = RunCtx {
        run: &run,
        c: &c,
        local: d("2026-03-02").into(),
        now: switch_at("2026-03-02"),
    };
    let (a, b) = tokio::join!(
        dispatch::dispatch_pending(&fx.db, &ctx),
        dispatch::dispatch_pending(&fx.db, &ctx)
    );
    // A deadlock may abort one sender; it rolls back whole, so count only winners.
    let sent: usize = [a, b]
        .into_iter()
        .filter_map(Result::ok)
        .map(|p| p.sent)
        .sum();
    assert_eq!(sent, 2, "each row once");
    assert_eq!(fx.wave_runs("on").await.len(), 1, "one task");
    assert!(fx.flips().await.iter().all(|f| f.task_id.is_some()));
}

#[tokio::test]
async fn a_failed_wave_run_fails_every_row_with_one_notice() {
    let fx = registered(&per_wave()).await;
    fx.tick(switch_at("2026-03-02")).await;
    fx.finish_levers("failed").await;
    fx.tick(switch_at("2026-03-02") + Duration::minutes(5))
        .await;
    assert!(fx.flips().await.iter().all(|f| f.status == "failed"));
    let notices = fx.notices().await;
    assert_eq!(notices.len(), 1, "one notice per lever run, not per unit");
    assert!(
        notices[0]
            .body
            .as_deref()
            .unwrap()
            .contains("`bondi`, `newtown`")
    );
}

#[tokio::test]
async fn a_groups_lever_carries_its_pinned_members_and_waits_for_them() {
    let fx = registered(GROUPED).await;
    fx.tick(switch_at("2026-03-02")).await;
    assert!(
        fx.flips().await.iter().all(|f| f.status == "pending"),
        "no membership pinned: a group's lever cannot be told its members"
    );
    let membership: &[(&str, &[&str])] = &[
        ("east", &["bondi", "coogee"]),
        ("west", &["parramatta"]),
        ("north", &["manly"]),
        ("south", &["glebe"]),
    ];
    fx.pin(&["north", "south"], Some(membership)).await;
    fx.tick(switch_at("2026-03-02") + Duration::minutes(5))
        .await;
    assert_eq!(
        fx.wave_runs("on").await,
        vec![json!({
            "units": ["east", "west"], "state": "on", "experiment": GROUPED_NAME, "wave": 0,
            "members": { "east": ["bondi", "coogee"], "west": ["parramatta"] },
        })]
    );
}
