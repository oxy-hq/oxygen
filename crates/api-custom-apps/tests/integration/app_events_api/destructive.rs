//! Discard, purge and the events switch (each audited), who may call any
//! route, and what the org's flag turns off.

use axum::http::{Method, StatusCode};
use chrono::{Duration, Utc};
use entity::{app_environments, app_events};
use sea_orm::EntityTrait;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{Fix, demote_guest};
use crate::staging_functions::{create_sandbox, make_guest_staff};

/// Every route, as `(method, path, body)`, for `f` and `event`.
fn every_route(f: &Fix, event: Uuid) -> Vec<(Method, String, Option<Value>)> {
    let org = format!("/customer-apps/{}", f.org());
    let app = format!("{org}/{}", f.listener_slug);
    let since = (Utc::now() - Duration::days(1)).to_rfc3339();
    let mut routes = reads(f, event);
    routes.extend([
        (
            Method::POST,
            format!("{org}/events/{event}/redeliver"),
            Some(json!({})),
        ),
        (
            Method::POST,
            format!("{org}/events/redeliver"),
            Some(json!({ "to": f.to(), "status": "dead" })),
        ),
        (
            Method::POST,
            format!("{org}/events/replay"),
            Some(
                json!({ "type": f.event_type(), "since": since, "to": f.to(),
                         "environment": "production" }),
            ),
        ),
        (
            Method::POST,
            format!("{org}/events/discard"),
            Some(json!({ "until": Utc::now().to_rfc3339() })),
        ),
        (
            Method::POST,
            format!("{org}/events/purge"),
            Some(json!({ "type": f.event_type() })),
        ),
        (
            Method::PUT,
            format!("{app}/environments/staging/events"),
            Some(json!({ "enabled": true })),
        ),
    ]);
    routes
}

/// The read routes.
fn reads(f: &Fix, event: Uuid) -> Vec<(Method, String, Option<Value>)> {
    let org = format!("/customer-apps/{}", f.org());
    let app = format!("{org}/{}", f.listener_slug);
    let find = format!(
        "{org}/events/find?type={}&key=statement%3A1",
        f.event_type().replace('/', "%2F")
    );
    vec![
        (Method::GET, format!("{org}/events/types"), None),
        (Method::GET, format!("{org}/events/{event}"), None),
        (Method::GET, format!("{org}/events?root={event}"), None),
        (Method::GET, find, None),
        (Method::GET, format!("{app}/events/feed"), None),
        (Method::GET, format!("{app}/events/dead"), None),
    ]
}

#[tokio::test]
async fn discard_marks_pending_as_discarded_and_audits() {
    let f = Fix::new().await;
    // After the fixture: its database harness clears `OXY_OWNER`.
    make_guest_staff();
    let pending = f.publish("pending").await;
    let held = f.publish("held").await;
    f.set_delivery(held, "held", 0).await;
    let acked = f.publish("acked").await;
    f.set_delivery(acked, "acked", 1).await;
    let later = f.publish("later").await;
    f.run(
        "UPDATE app_events SET published_at = now() + interval '1 hour' WHERE id = $1",
        vec![later.into()],
    )
    .await;
    let until = (Utc::now() + Duration::seconds(5)).to_rfc3339();

    let (status, body) = f
        .send(
            Method::POST,
            &format!("/customer-apps/{}/events/discard", f.org()),
            json!({ "type": f.event_type(), "until": until }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["count"], 2, "{body}");
    assert_eq!(f.delivery(pending).await.status, "discarded");
    assert_eq!(f.delivery(held).await.status, "discarded");
    assert_eq!(
        f.delivery(acked).await.status,
        "acked",
        "settled: left alone"
    );
    assert_eq!(
        f.delivery(later).await.status,
        "pending",
        "published after `until`"
    );
    let audits = f.audits("app.events_discarded").await;
    assert_eq!(audits.len(), 1, "{audits:?}");
    assert_eq!(audits[0]["count"], 2, "{audits:?}");
    assert_eq!(audits[0]["type"], f.event_type());
}

#[tokio::test]
async fn purge_deletes_events_and_audits() {
    let f = Fix::new().await;
    // After the fixture: its database harness clears `OXY_OWNER`.
    make_guest_staff();
    let purged = f.publish("statement:1").await;
    let kept = f.publish("statement:2").await;

    let (status, body) = f
        .send(
            Method::POST,
            &format!("/customer-apps/{}/events/purge", f.org()),
            json!({ "type": f.event_type(), "key": "statement:1" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["count"], 1, "{body}");
    let db = f.db();
    let exists = |id| async move {
        app_events::Entity::find_by_id(id)
            .one(db)
            .await
            .expect("read the event")
            .is_some()
    };
    assert!(!exists(purged).await, "the event is deleted");
    assert!(exists(kept).await, "another key is kept");
    assert_eq!(f.delivery(kept).await.status, "pending");
    let audits = f.audits("app.events_purged").await;
    assert_eq!(audits.len(), 1, "{audits:?}");
    assert_eq!(audits[0]["count"], 1, "{audits:?}");
    assert_eq!(audits[0]["key"], "statement:1");
}

#[tokio::test]
async fn the_switch_refuses_production() {
    let f = Fix::new().await;
    // After the fixture: its database harness clears `OXY_OWNER`.
    make_guest_staff();
    let switch = |environment: &str| {
        format!(
            "/customer-apps/{}/{}/environments/{environment}/events",
            f.org(),
            f.listener_slug
        )
    };
    let (status, body) = f
        .send(
            Method::PUT,
            &switch("production"),
            json!({ "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "production_has_no_events_switch", "{body}");
    let (status, body) = f
        .send(
            Method::PUT,
            &switch("dev-nobody"),
            json!({ "enabled": true }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    create_sandbox(&f.t, f.listener, "a1", None).await;
    let (status, body) = f
        .send(Method::PUT, &switch("dev-a1"), json!({ "enabled": true }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (body["environment"].clone(), body["enabled"].clone()),
        (json!("dev-a1"), json!(true))
    );
    let row = app_environments::Entity::find_by_id((f.listener, "dev-a1".to_string()))
        .one(f.db())
        .await
        .expect("read the sandbox")
        .expect("the sandbox");
    assert!(row.events_enabled, "the switch is on");
    let audits = f.audits("app.environment.events").await;
    assert_eq!(audits.len(), 1, "only the flip is audited: {audits:?}");
    assert_eq!(audits[0]["enabled"], true);
}

#[tokio::test]
async fn a_non_staff_caller_gets_403() {
    let f = Fix::new().await;
    let event = f.publish("statement:1").await;
    f.set_delivery(event, "dead", 1).await;
    demote_guest(&f.t).await;

    for (method, path, body) in every_route(&f, event) {
        let (status, answer) = super::call(method.clone(), &path, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {answer}");
    }
    assert_eq!(f.delivery(event).await.status, "dead", "nothing changed");
}

#[tokio::test]
async fn writes_answer_409_while_the_flag_is_off_and_reads_still_work() {
    let f = Fix::with_flag(false).await;
    make_guest_staff();
    let event = f.publish("statement:1").await;
    f.set_delivery(event, "dead", 1).await;

    let routes = every_route(&f, event);
    let (read, write) = routes.split_at(reads(&f, event).len());
    for (method, path, body) in read {
        let (status, answer) = super::call(method.clone(), path, body.clone()).await;
        assert_eq!(status, StatusCode::OK, "{method} {path}: {answer}");
    }
    for (method, path, body) in write {
        let (status, answer) = super::call(method.clone(), path, body.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT, "{method} {path}: {answer}");
        assert_eq!(
            answer["code"], "events_not_enabled",
            "{method} {path}: {answer}"
        );
    }
    assert_eq!(f.delivery(event).await.status, "dead", "nothing changed");
}
