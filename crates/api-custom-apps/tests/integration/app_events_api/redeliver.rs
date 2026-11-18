//! Redelivery, one event's and in bulk: queued again on the same delivery,
//! counted by a dry run, spread out per listener.

use axum::http::{Method, StatusCode};
use chrono::{DateTime, Utc};
use oxy_api_custom_apps::app_events::retention::sweep_org;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::json;
use uuid::Uuid;

use super::Fix;
use crate::staging_functions::make_guest_staff;

/// The queue row named `run_id`: its status and when it may be claimed.
async fn task(f: &Fix, run_id: &str) -> (String, DateTime<Utc>) {
    let row = f
        .db()
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT queue_status, available_at FROM agentic_task_queue WHERE task_id = $1",
            [run_id.into()],
        ))
        .await
        .expect("read the task")
        .unwrap_or_else(|| panic!("the task {run_id}"));
    (
        row.try_get("", "queue_status").expect("queue_status"),
        row.try_get("", "available_at").expect("available_at"),
    )
}

/// The attempts recorded on `event`'s delivery.
async fn attempt_rows(f: &Fix, event: Uuid) -> i64 {
    f.db()
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*) AS n FROM app_event_delivery_attempts a \
               JOIN app_event_deliveries d ON d.id = a.delivery_id WHERE d.event_id = $1",
            [event.into()],
        ))
        .await
        .expect("count the attempts")
        .expect("a count")
        .try_get("", "n")
        .expect("n")
}

/// Every task queued for the listener app.
async fn listener_tasks(f: &Fix) -> i64 {
    f.db()
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*) AS n FROM agentic_task_queue WHERE spec::text LIKE $1",
            [format!("%{}%", f.listener).into()],
        ))
        .await
        .expect("count the tasks")
        .expect("a count")
        .try_get("", "n")
        .expect("n")
}

#[tokio::test]
async fn redeliver_requeues_a_dead_delivery() {
    let f = Fix::new().await;
    // After the fixture: its database harness clears `OXY_OWNER`.
    make_guest_staff();
    let event = f.publish("statement:1").await;
    f.set_delivery(event, "dead", 5).await;
    f.attempt(event, 5, Some("error"), Some("boom"), 30).await;
    let before = f.delivery(event).await;
    let path = format!("/customer-apps/{}/events/{event}/redeliver", f.org());

    // A listener with no delivery of this event is nothing to redeliver.
    let (status, body) = f
        .send(Method::POST, &path, json!({ "to": "nobody/fn" }))
        .await;
    assert_eq!(
        (status, body["count"].clone()),
        (StatusCode::OK, json!(0)),
        "{body}"
    );

    let (status, body) = f.send(Method::POST, &path, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["count"], 1, "{body}");
    let after = f.delivery(event).await;
    assert_eq!(after.id, before.id, "the same delivery");
    assert_eq!(after.event_id, event, "the same event");
    assert_eq!(after.status, "pending");
    assert_eq!(after.attempts, 5, "its past attempts still count as made");
    assert_eq!(
        attempt_rows(&f, event).await,
        1,
        "every past attempt is kept"
    );
    let run_id = after.run_id.clone().expect("a new task");
    assert_ne!(Some(run_id.clone()), before.run_id, "a new task");
    assert_eq!(task(&f, &run_id).await.0, "queued");

    // Pending now: nothing more to redeliver.
    let (status, body) = f.send(Method::POST, &path, json!({})).await;
    assert_eq!(
        (status, body["count"].clone()),
        (StatusCode::OK, json!(0)),
        "{body}"
    );
}

#[tokio::test]
async fn a_dry_run_redeliver_writes_nothing_and_counts() {
    let f = Fix::new().await;
    // After the fixture: its database harness clears `OXY_OWNER`.
    make_guest_staff();
    let event = f.publish("statement:1").await;
    f.set_delivery(event, "dead", 5).await;
    let before = f.delivery(event).await;
    let tasks = listener_tasks(&f).await;

    let single = format!("/customer-apps/{}/events/{event}/redeliver", f.org());
    let (status, body) = f
        .send(Method::POST, &single, json!({ "dryRun": true }))
        .await;
    assert_eq!(
        (status, body["count"].clone()),
        (StatusCode::OK, json!(1)),
        "{body}"
    );
    let bulk = format!("/customer-apps/{}/events/redeliver", f.org());
    let (status, body) = f
        .send(
            Method::POST,
            &bulk,
            json!({ "to": f.to(), "status": "dead", "dryRun": true }),
        )
        .await;
    assert_eq!(
        (status, body["count"].clone()),
        (StatusCode::OK, json!(1)),
        "{body}"
    );

    assert_eq!(f.delivery(event).await, before, "the delivery is untouched");
    assert_eq!(listener_tasks(&f).await, tasks, "no task was queued");
}

#[tokio::test]
async fn bulk_redeliver_is_spread_out() {
    let f = Fix::new().await;
    // After the fixture: its database harness clears `OXY_OWNER`.
    make_guest_staff();
    let mut events = Vec::new();
    for n in 0..30 {
        let event = f.publish(&format!("statement:{n}")).await;
        f.set_delivery(event, "dead", 5).await;
        events.push(event);
    }
    let bulk = format!("/customer-apps/{}/events/redeliver", f.org());

    let (status, body) = f
        .send(
            Method::POST,
            &bulk,
            json!({ "to": "elsewhere/fn", "status": "dead" }),
        )
        .await;
    assert_eq!(
        (status, body["count"].clone()),
        (StatusCode::OK, json!(0)),
        "{body}"
    );
    let (status, body) = f
        .send(
            Method::POST,
            &bulk,
            json!({ "to": f.to(), "status": "sleeping" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = f
        .send(
            Method::POST,
            &bulk,
            json!({ "to": f.to(), "status": "dead" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["count"], 30, "{body}");
    let mut available = Vec::new();
    for event in events {
        let delivery = f.delivery(event).await;
        assert_eq!(delivery.status, "pending");
        available.push(task(&f, &delivery.run_id.expect("a task")).await.1);
    }
    let first = available.iter().min().expect("tasks");
    let last = available.iter().max().expect("tasks");
    let span = (*last - *first).num_milliseconds();
    assert!(
        span >= 2_900,
        "30 tasks at 10 a second span 2.9 s: {span} ms"
    );
}

/// A redelivered `expired` delivery is pending again from its redelivery:
/// retention's expiry measures from then, so the next sweep leaves it for its
/// task to run.
#[tokio::test]
async fn a_redelivered_expired_delivery_is_not_expired_again_before_it_runs() {
    let f = Fix::new().await;
    make_guest_staff();
    let event = f.publish("statement:1").await;
    f.run(
        "UPDATE app_event_deliveries SET created_at = now() - interval '31 days' \
          WHERE event_id = $1",
        vec![event.into()],
    )
    .await;
    let sweep = || async {
        sweep_org(f.db(), Utc::now(), f.t.org_id)
            .await
            .expect("sweep")
    };
    assert_eq!(sweep().await.expired, 1);
    assert_eq!(f.delivery(event).await.status, "expired");

    let path = format!("/customer-apps/{}/events/{event}/redeliver", f.org());
    let (status, body) = f.send(Method::POST, &path, json!({})).await;
    assert_eq!(
        (status, body["count"].clone()),
        (StatusCode::OK, json!(1)),
        "{body}"
    );

    assert_eq!(
        sweep().await.expired,
        0,
        "redelivered just now: not 30 days unfinished"
    );
    let pending = f.delivery(event).await;
    assert_eq!(pending.status, "pending");
    let run_id = pending.run_id.expect("its task");
    assert_eq!(
        task(&f, &run_id).await.0,
        "queued",
        "its task is not cancelled"
    );
}
