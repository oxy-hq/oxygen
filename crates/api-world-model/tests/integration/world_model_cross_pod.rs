//! Cross-pod delivery for the world-model live feed.
//!
//! The bug this pins: the broadcast bus in `events.rs` is per process.
//! `POST /api/webhooks/toast/orders` is `FleetOk` and runs on a `serve`
//! replica; `GET /api/{ws}/world-model/events` was `IdeOnly` and proxied to the
//! ide. Publisher and subscriber were therefore never in the same process, so
//! every order ripple was dropped — the LIVE EVENTS panel stayed empty even
//! after #2816 stopped the webhook returning 401.
//!
//! These assertions are about the fan-out, not the classification, so they hold
//! whichever role the SSE route carries — which is what let it become `FleetOk`.
//!
//! Two invariants, both of which failed before this change:
//!
//! 1. **Publishing is durable.** `publish_order` must leave a row behind, not
//!    only shout into an in-process channel nobody on this pod is listening to.
//! 2. **A row written by another process reaches this process's subscribers.**
//!    That is what makes the fan-out cross-pod, and it is simulated here by
//!    inserting the row directly — exactly what a different replica's writer
//!    would have done.
//!
//! Spins up a Postgres testcontainer (or reuses `OXY_DATABASE_URL` in CI).

use entity::world_model_events;
use futures::StreamExt;
use oxy_api_world_model::events::{
    live_events, recent_events, spawn_world_model_tailer, subscribe,
};
use oxy_world_model::live_feed::{OrderEvent, publish_order};
use sea_orm::{ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

/// Poll until `f` returns `Some`, or give up. The writer and the tailer are
/// both background tasks, so every assertion here is necessarily eventual.
async fn eventually<T, F, Fut>(label: &str, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for: {label}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Per-test database, migrated and wired so that `establish_connection()` (used
/// by the writer, the tailer and the backfill) points at it.
///
/// Own database rather than the shared one on purpose: the tailer polls
/// `world_model_events` unfiltered, so on a shared schema one test's rows would
/// reach another test's subscriber and the assertions here would depend on
/// execution order.
async fn setup_db() -> sea_orm::DatabaseConnection {
    let (db, test_url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    // SAFETY: single-threaded test setup before any other env access. nextest
    // isolates each test in its own process, so pointing the process-wide
    // OnceCell at the per-test DB here is safe.
    unsafe {
        std::env::set_var("OXY_DATABASE_URL", &test_url);
        std::env::remove_var("OXY_DATABASE_AUTH_MODE");
    }
    db
}

fn order(order_id: &str) -> OrderEvent {
    OrderEvent {
        kind: "order_ripple",
        key: "restaurant-guid-test".to_string(),
        store_name: None,
        amount: 42.75,
        order_id: order_id.to_string(),
        ts: chrono::Utc::now(),
        location_id: None,
        location_name: None,
    }
}

#[tokio::test]
async fn publishing_an_order_leaves_a_durable_row() {
    let db = setup_db().await;
    let workspace_id = Uuid::new_v4();

    publish_order(workspace_id, order("order-durable-0001"));

    let row = eventually("the published order to be written", || async {
        world_model_events::Entity::find()
            .filter(world_model_events::Column::WorkspaceId.eq(workspace_id))
            .one(&db)
            .await
            .ok()
            .flatten()
    })
    .await;

    assert_eq!(
        row.payload["order_id"], "order-durable-0001",
        "the row must carry the event the caller published, not a placeholder"
    );
    assert_eq!(
        row.payload["type"], "order_ripple",
        "the serialised shape the SSE layer re-emits must survive the round trip"
    );
}

#[tokio::test]
async fn a_row_written_by_another_pod_reaches_this_pods_subscriber() {
    let db = setup_db().await;
    let workspace_id = Uuid::new_v4();

    // Subscribe first, then start the tailer, mirroring the SSE handler: a
    // viewer is already connected when an event lands.
    let mut rx = subscribe(workspace_id);
    spawn_world_model_tailer();

    // Give the tailer a moment to take its high-water mark, so the row below is
    // genuinely "new" rather than part of the startup snapshot it skips.
    tokio::time::sleep(Duration::from_millis(800)).await;

    // What a DIFFERENT replica's writer would have done. This process never
    // called publish_order, so nothing reaches the subscriber unless the row
    // itself is the carrier.
    world_model_events::Entity::insert(world_model_events::ActiveModel {
        workspace_id: ActiveValue::Set(workspace_id),
        payload: ActiveValue::Set(json!({
            "type": "order_ripple",
            "key": "restaurant-guid-test",
            "store_name": null,
            "amount": 42.75,
            "order_id": "order-from-another-pod",
            "ts": "2026-07-30T00:00:00Z",
        })),
        ..Default::default()
    })
    .exec(&db)
    .await
    .expect("insert the other pod's row");

    let received = tokio::time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .expect("subscriber never saw the other pod's event — fan-out is not cross-pod")
        .expect("bus closed");

    assert_eq!(
        received.payload["order_id"], "order-from-another-pod",
        "the event a different pod published must arrive verbatim"
    );
}

#[tokio::test]
async fn events_are_scoped_to_their_workspace() {
    // Bound but unused: the call is here for its side effect of pointing
    // `establish_connection()` at this test's own database.
    let _db = setup_db().await;
    let mine = Uuid::new_v4();
    let theirs = Uuid::new_v4();

    let mut rx = subscribe(mine);
    spawn_world_model_tailer();
    tokio::time::sleep(Duration::from_millis(800)).await;

    publish_order(theirs, order("order-other-workspace"));
    publish_order(mine, order("order-my-workspace"));

    let received = tokio::time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .expect("subscriber saw nothing")
        .expect("bus closed");

    // The other workspace's event was published FIRST, so if scoping were
    // broken this is the one that would arrive.
    assert_eq!(
        received.payload["order_id"], "order-my-workspace",
        "a subscriber must never receive another workspace's events"
    );
}

/// The overlap the subscribe-then-backfill order creates.
///
/// The handler subscribes before it reads history so nothing published in
/// between is lost. That trade turns a gap into an overlap: the tailer can fan a
/// row onto the bus *and* the backfill read can return that same row, and a
/// stream that simply concatenates the two sends it twice. On this feed a
/// duplicate is not cosmetic — the panel counts `orders/min`, so one order
/// arriving twice is one order counted twice.
#[tokio::test]
async fn an_event_in_the_backfill_is_not_replayed_by_the_live_stream() {
    let db = setup_db().await;
    let workspace_id = Uuid::new_v4();

    // A connected viewer, exactly as the handler sets one up.
    let rx = subscribe(workspace_id);
    spawn_world_model_tailer();
    tokio::time::sleep(Duration::from_millis(800)).await;

    // The overlapping row: the tailer fans it onto `rx` AND the backfill read
    // below returns it, because it lands before that read runs.
    publish_order(workspace_id, order("order-in-both-halves"));
    eventually("the overlapping row to be written", || async {
        world_model_events::Entity::find()
            .filter(world_model_events::Column::WorkspaceId.eq(workspace_id))
            .one(&db)
            .await
            .ok()
            .flatten()
    })
    .await;
    // Let the tailer put it on the bus, so it is genuinely buffered in `rx`.
    tokio::time::sleep(Duration::from_millis(800)).await;

    let (backfill, through_id) = recent_events(workspace_id).await;
    assert_eq!(
        backfill.len(),
        1,
        "precondition: the backfill must contain the overlapping row"
    );

    // A second, genuinely new row — it must still arrive, or the filter has
    // simply muted the live half instead of de-duplicating it.
    publish_order(workspace_id, order("order-after-the-backfill"));

    let mut stream = Box::pin(live_events(backfill, rx, through_id));
    let first = tokio::time::timeout(Duration::from_secs(15), stream.next())
        .await
        .expect("stream produced nothing")
        .expect("stream ended");
    let second = tokio::time::timeout(Duration::from_secs(15), stream.next())
        .await
        .expect("the event published after the backfill never arrived")
        .expect("stream ended");

    assert_eq!(
        first.payload["order_id"], "order-in-both-halves",
        "the backfill half comes first"
    );
    assert_eq!(
        second.payload["order_id"], "order-after-the-backfill",
        "the overlapping row must NOT be replayed from the bus; the next frame \
         has to be the row that landed after the backfill"
    );
}
