use chrono::Duration;
use oxy_app::server::experiment::dispatch::{self, RunCtx};
use oxy_app::server::experiment::tick::tick_experiments;
use oxy_experiment::timeline::{day_start_utc, today_in};
use serde_json::json;
use uuid::Uuid;

use super::fixture::*;

async fn registered() -> Fx {
    let fx = Fx::seed(DESIGN).await;
    fx.register().await;
    fx
}

#[tokio::test]
async fn nothing_moves_before_the_first_switch_instant() {
    let fx = registered().await;
    fx.tick(switch_at("2026-03-02") - Duration::seconds(1))
        .await;
    assert!(fx.flips().await.is_empty());
    assert_eq!(fx.run().await.state, "registered");
}

#[tokio::test]
async fn the_first_switch_dispatches_that_waves_on_flips_through_the_lever() {
    let fx = registered().await;
    fx.tick(switch_at("2026-03-02")).await;
    assert_eq!(fx.run().await.state, "live");
    let flips = fx.flips().await;
    let seen: Vec<_> = flips
        .iter()
        .map(|f| (f.unit.as_str(), f.target_state.as_str(), f.status.as_str()))
        .collect();
    assert_eq!(
        seen,
        vec![
            ("bondi", "on", "dispatched"),
            ("newtown", "on", "dispatched")
        ]
    );
    let head = fx.head().await;
    for f in &flips {
        assert_eq!(f.due_at, d("2026-03-02").and_hms_opt(0, 0, 0).unwrap());
        assert!(f.task_id.is_some());
        assert_eq!(
            f.task_id, f.agentic_run_id,
            "a Global seed's root task id is its run id"
        );
        assert_eq!(
            f.lever_revision_id,
            Some(head),
            "the revision the lever resolved at"
        );
    }
    let meta = fx.lever_metadata("bondi", "on").await;
    assert_eq!(meta["trigger"], "experiment");
    assert_eq!(meta["workflow_ref"], "automations/promo_on.automation.yml");
    assert_eq!(
        meta["variables"],
        json!({ "unit": "bondi", "state": "on", "experiment": NAME, "wave": 0 })
    );
    assert_eq!(meta["logical_date"], switch_at("2026-03-02").to_rfc3339());
}

#[tokio::test]
async fn each_flip_records_the_revision_its_lever_resolved_at() {
    let fx = registered().await;
    let r1 = fx.head().await;
    fx.tick(switch_at("2026-03-02")).await;
    // The lever automation is edited between waves; the design is not.
    let r2 = fx
        .promote(utc("2026-03-10T00:00:00Z"), DESIGN, LEVERS)
        .await;
    fx.tick(switch_at("2026-03-16")).await;
    assert_eq!(fx.flip("bondi", "on").await.lever_revision_id, Some(r1));
    assert_eq!(fx.flip("manly", "on").await.lever_revision_id, Some(r2));

    assert_eq!(
        fx.design().await.design.waves.len(),
        2,
        "the run stays pinned; only the lever moved"
    );
}

#[tokio::test]
async fn double_tick_dispatches_each_flip_once() {
    let fx = registered().await;
    let at = switch_at("2026-03-02");
    fx.make_due(at).await;
    let (a, b) = tokio::join!(tick_experiments(&fx.db, at), tick_experiments(&fx.db, at));
    assert_eq!(a + b, 1, "exactly one replica claims the run");
    // Repeated ticks hit the unique key and the stamp, and send nothing.
    fx.tick(at + Duration::minutes(1)).await;
    fx.tick(at + Duration::minutes(2)).await;
    assert_eq!(fx.flips().await.len(), 2);
    assert_eq!(fx.lever_runs("bondi", "on").await, 1);
    assert_eq!(fx.lever_runs("newtown", "on").await, 1);
}

#[tokio::test]
async fn orphaned_flip_row_is_reenqueued() {
    let fx = registered().await;
    let run = fx.run().await;
    // A replica recorded the row and died before its enqueue transaction.
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
    fx.tick(switch_at("2026-03-02")).await;
    let bondi = fx.flip("bondi", "on").await;
    assert_eq!(bondi.status, "dispatched");
    assert!(bondi.task_id.is_some());
    assert_eq!(fx.lever_runs("bondi", "on").await, 1, "re-enqueued");
    fx.tick(switch_at("2026-03-02") + Duration::minutes(5))
        .await;
    assert_eq!(fx.lever_runs("bondi", "on").await, 1, "never duplicated");
    assert_eq!(fx.flip("bondi", "on").await.task_id, bondi.task_id);
}

#[tokio::test]
async fn flip_dispatches_once_at_the_first_local_instant_of_a_dst_switch_day() {
    // America/Santiago springs forward at midnight on 2026-09-06: that day has
    // no 00:00, so its first instant is PR 2's to compute, not ours.
    let design = DESIGN
        .replace("Australia/Sydney", "America/Santiago")
        .replace("2026-03-02", "2026-09-06")
        .replace("2026-03-16", "2026-09-20");
    let fx = Fx::seed(&design).await;
    fx.register_at(utc("2026-08-10T00:00:00Z"))
        .await
        .expect("register");
    let tz = chrono_tz::America::Santiago;
    let first = day_start_utc(d("2026-09-06"), tz);
    assert_eq!(today_in(tz, first), d("2026-09-06"));

    fx.tick(first - Duration::minutes(2)).await;
    assert!(
        fx.flips().await.is_empty(),
        "the day has not begun in Santiago"
    );
    assert_eq!(
        fx.run().await.next_check_at.with_timezone(&chrono::Utc),
        first,
        "wakes on that instant"
    );

    oxy_app::server::experiment::tick::tick_experiments(&fx.db, first).await;
    assert_eq!(fx.lever_runs("bondi", "on").await, 1);
    fx.tick(first + Duration::hours(20)).await;
    assert_eq!(
        fx.lever_runs("bondi", "on").await,
        1,
        "once, on that calendar date"
    );
    assert_eq!(
        fx.flip("bondi", "on").await.due_at,
        d("2026-09-06").and_hms_opt(0, 0, 0).unwrap()
    );
}

#[tokio::test]
async fn a_tick_after_an_outage_dispatches_every_due_wave_once() {
    let fx = registered().await;
    fx.tick(switch_at("2026-03-20")).await;
    for (unit, wave) in [
        ("bondi", 0),
        ("newtown", 0),
        ("manly", 1),
        ("surry_hills", 1),
    ] {
        assert_eq!(fx.lever_runs(unit, "on").await, 1, "{unit}");
        assert_eq!(
            fx.lever_metadata(unit, "on").await["variables"]["wave"],
            wave
        );
    }
    assert_eq!(
        fx.flip("manly", "on").await.due_at,
        d("2026-03-16").and_hms_opt(0, 0, 0).unwrap(),
        "due on its own date"
    );
    fx.tick(switch_at("2026-03-21")).await;
    assert_eq!(fx.flips().await.len(), 4);
}

#[tokio::test]
async fn a_later_version_never_changes_the_pinned_run() {
    let fx = registered().await;
    let pinned = fx.run().await;
    let edited = DESIGN
        .replace("[bondi, newtown]", "[bondi, glebe]")
        .replace("2026-03-02", "2026-03-03");
    fx.promote(utc("2026-02-20T00:00:00Z"), &edited, LEVERS)
        .await;
    fx.tick(switch_at("2026-03-02")).await;
    let units: Vec<_> = fx.flips().await.into_iter().map(|f| f.unit).collect();
    assert_eq!(
        units,
        vec!["bondi", "newtown"],
        "the registered design, on its registered date"
    );
    assert_eq!(fx.lever_runs("glebe", "on").await, 0);
    let run = fx.run().await;
    assert_eq!(run.design_id, pinned.design_id);
}

async fn orphan_on(fx: &Fx, run_id: Uuid, unit: &str, day: &str) {
    fx.exec(
        "INSERT INTO experiment_flips (id, run_id, unit, target_state, due_at, wave, status, created_at) \
         VALUES ($1, $2, $3, 'on', $4, 0, 'pending', $5)",
        [
            Uuid::new_v4().into(),
            run_id.into(),
            unit.into(),
            d(day).into(),
            switch_at(day).fixed_offset().into(),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_senders_racing_on_one_orphan_send_it_once() {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};
    let fx = registered().await;
    let (run, c) = (fx.run().await, fx.design().await);
    orphan_on(&fx, run.id, "bondi", "2026-03-02").await;
    let ctx = RunCtx {
        run: &run,
        c: &c,
        local: d("2026-03-02").into(),
        now: switch_at("2026-03-02"),
    };
    // Hold the row's lock so both senders read it as `pending` and then queue
    // on the stamp; the one that follows must find the row already stamped.
    let held = fx.db.begin().await.expect("begin");
    held.execute_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT id FROM experiment_flips WHERE unit = 'bondi' FOR UPDATE",
    ))
    .await
    .expect("lock");
    let senders =
        futures::future::join_all((0..2).map(|_| dispatch::dispatch_pending(&fx.db, &ctx)));
    let release = async {
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        held.commit().await.expect("release");
    };
    let (sent, ()) = tokio::join!(senders, release);
    let sent: usize = sent.into_iter().map(|p| p.unwrap().sent).sum();
    assert_eq!(sent, 1, "exactly one sender stamps the row");
    assert_eq!(fx.lever_runs("bondi", "on").await, 1);
    assert!(fx.flip("bondi", "on").await.task_id.is_some());
}

#[tokio::test]
async fn a_late_on_after_an_abort_is_cancelled_not_sent() {
    // A tick stalled past its lease; another replica aborted the run; the
    // stalled one then records and tries to send a wave `on`.
    let fx = registered().await;
    fx.exec(
        "UPDATE experiment_runs SET state = 'aborted' WHERE workspace_id = $1",
        [fx.ws.into()],
    )
    .await;
    let (run, c) = (fx.run().await, fx.design().await);
    orphan_on(&fx, run.id, "bondi", "2026-03-02").await;
    let ctx = RunCtx {
        run: &run,
        c: &c,
        local: d("2026-03-02").into(),
        now: switch_at("2026-03-02"),
    };
    let sent = dispatch::dispatch_pending(&fx.db, &ctx).await.unwrap();
    assert_eq!(sent.sent, 0);
    assert_eq!(fx.lever_runs("bondi", "on").await, 0);
    let f = fx.flip("bondi", "on").await;
    assert_eq!(f.status, "cancelled");
    assert!(f.task_id.is_none());
}
