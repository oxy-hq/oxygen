//! The routes that read or delete an event's body, against an org with an
//! OLTP store of its own: every event takes the real road, published through
//! the definer function and relayed (`app_events_delivery`'s fixture), so its
//! body is in the org's store and nowhere else.

use axum::http::{Method, StatusCode};
use chrono::{Duration, Utc};
use entity::{app_event_deliveries as deliveries, app_events};
use oxy_oltp::events::{self as outbox, OutboxKey, OutboxTarget};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};

use super::call;
use crate::app_events_delivery::{
    BROKEN_JS, ECHO_JS, FUNCTION, Fx, completed, failed, outcomes, published, with_fx,
};
use crate::staging_functions::make_guest_staff;

/// The body of `event` in the org's store.
async fn body(fx: &Fx, event: &app_events::Model) -> Option<Value> {
    outbox::read_body(
        &fx.t.db,
        fx.t.org_id,
        &OutboxTarget::Production,
        &event.writer_role,
        event.id,
    )
    .await
    .expect("read the body")
}

async fn delete_body(fx: &Fx, event: &app_events::Model) {
    let key = OutboxKey {
        writer_role: event.writer_role.clone(),
        id: event.id,
    };
    let deleted = outbox::delete_bodies(&fx.t.db, fx.t.org_id, &OutboxTarget::Production, &[key])
        .await
        .expect("delete the body");
    assert_eq!(deleted, 1, "the body was in the org's store");
}

#[tokio::test]
async fn an_event_shows_its_body_from_the_orgs_store() {
    with_fx(|fx| async move {
        let listener = fx.listener(ECHO_JS, json!({})).await;
        let delivery = fx.deliver("statement:1", &listener).await;
        let event = fx.event(&delivery).await;
        let path = format!("/customer-apps/{}/events/{}", fx.t.org_slug, event.id);

        let (status, shown) = call(Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::OK, "{shown}");
        assert_eq!(shown["data"], published(), "read from the org's store");
        assert!(shown["dataUnavailable"].is_null(), "{shown}");
        // A list never reads a body.
        let chain = format!("/customer-apps/{}/events?root={}", fx.t.org_slug, event.id);
        let (status, listed) = call(Method::GET, &chain, None).await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        assert!(listed["events"][0].get("data").is_none(), "{listed}");

        delete_body(&fx, &event).await;
        let (status, shown) = call(Method::GET, &path, None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "a body gone never fails the route: {shown}"
        );
        assert!(shown["data"].is_null(), "{shown}");
        assert!(shown["dataUnavailable"].is_string(), "{shown}");
    })
    .await;
}

/// R41 point 4: a redelivered dead delivery runs again, its past attempts
/// kept, with a fresh budget of `maxAttempts`.
#[tokio::test]
async fn a_redelivered_dead_delivery_runs_again_with_a_fresh_budget_of_attempts() {
    with_fx(|fx| async move {
        // After the fixture: its database harness clears `OXY_OWNER`.
        make_guest_staff();
        let listener = fx
            .listener(BROKEN_JS, json!({ "retries": { "maxAttempts": 2 } }))
            .await;
        let delivery = fx.deliver("statement:1", &listener).await;
        failed(fx.run(&delivery).await);
        failed(fx.run(&fx.reload(&delivery).await).await);
        assert_eq!(fx.reload(&delivery).await.status, "dead");

        let path = format!(
            "/customer-apps/{}/events/{}/redeliver",
            fx.t.org_slug, delivery.event_id
        );
        let (status, answer) = call(Method::POST, &path, Some(json!({}))).await;
        assert_eq!(
            (status, answer["count"].clone()),
            (StatusCode::OK, json!(1)),
            "{answer}"
        );

        let again = fx.reload(&delivery).await;
        assert_eq!((again.id, again.status.as_str()), (delivery.id, "pending"));
        failed(fx.run(&again).await);
        assert_eq!(
            fx.reload(&delivery).await.status,
            "pending",
            "attempt 3 of 4"
        );
        failed(fx.run(&again).await);
        let dead = fx.reload(&delivery).await;
        assert_eq!((dead.status.as_str(), dead.attempts), ("dead", 4));
        let tries = fx.attempts(&delivery).await;
        let expected: Vec<(i32, Option<&str>)> = (1..=4).map(|n| (n, Some("error"))).collect();
        assert_eq!(
            outcomes(&tries),
            expected,
            "every attempt kept, two of each budget"
        );
        assert!(completed(fx.run(&again).await).starts_with("skipped: "));
    })
    .await;
}

#[tokio::test]
async fn replay_stays_within_retention() {
    with_fx(|fx| async move {
        // After the fixture: its database harness clears `OXY_OWNER`.
        make_guest_staff();
        let listener = fx.listener(ECHO_JS, json!({})).await;
        let recent = fx.deliver("recent", &listener).await;
        let old = fx.deliver("old", &listener).await;
        let gone = fx.deliver("gone", &listener).await;
        // The listener joined late: nothing was delivered to it.
        deliveries::Entity::delete_many()
            .filter(deliveries::Column::AppId.eq(listener.app_id))
            .exec(&fx.t.db)
            .await
            .expect("forget the deliveries");
        // Published before the retention window, though still stored.
        let mut aged: app_events::ActiveModel = fx.event(&old).await.into();
        aged.published_at = sea_orm::ActiveValue::Set((Utc::now() - Duration::days(40)).into());
        sea_orm::ActiveModelTrait::update(aged, &fx.t.db)
            .await
            .expect("age the event");
        delete_body(&fx, &fx.event(&gone).await).await;
        let replay = |dry_run: bool| {
            json!({
                "type": fx.event_type(),
                "since": (Utc::now() - Duration::days(60)).to_rfc3339(),
                "to": format!("{}/{FUNCTION}", listener.slug),
                "environment": "production",
                "dryRun": dry_run,
            })
        };
        let path = format!("/customer-apps/{}/events/replay", fx.t.org_slug);

        let (status, answer) = call(Method::POST, &path, Some(replay(true))).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert_eq!(
            answer["count"], 1,
            "the retained event with a body: {answer}"
        );
        assert_eq!(answer["skippedBodyGone"], 1, "{answer}");
        let created = || async {
            deliveries::Entity::find()
                .filter(deliveries::Column::AppId.eq(listener.app_id))
                .all(&fx.t.db)
                .await
                .expect("read the deliveries")
        };
        assert!(created().await.is_empty(), "a dry run creates nothing");

        let (status, answer) = call(Method::POST, &path, Some(replay(false))).await;
        assert_eq!(
            (status, answer["count"].clone()),
            (StatusCode::OK, json!(1)),
            "{answer}"
        );
        let made = created().await;
        assert_eq!(made.len(), 1, "{made:?}");
        assert_eq!(made[0].event_id, recent.event_id);
        assert_eq!(made[0].status, "pending");
        assert_eq!(completed(fx.run(&made[0]).await), "delivered (HTTP 200)");
    })
    .await;
}

#[tokio::test]
async fn purge_deletes_the_bodies_and_deletes_nothing_when_the_store_cannot_be_reached() {
    with_fx(|fx| async move {
        // After the fixture: its database harness clears `OXY_OWNER`.
        make_guest_staff();
        let listener = fx.listener(ECHO_JS, json!({})).await;
        let delivery = fx.deliver("statement:1", &listener).await;
        let event = fx.event(&delivery).await;
        let path = format!("/customer-apps/{}/events/purge", fx.t.org_slug);
        let purge = json!({ "type": fx.event_type(), "key": "statement:1" });

        let host = fx.set_store_host("127.0.0.1:1").await;
        let (status, answer) = call(Method::POST, &path, Some(purge.clone())).await;
        fx.set_store_host(&host).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{answer}");
        assert_eq!(answer["code"], "events_store_unreachable", "{answer}");
        assert!(
            app_events::Entity::find_by_id(event.id)
                .one(&fx.t.db)
                .await
                .expect("read the event")
                .is_some(),
            "the event is kept"
        );
        assert!(body(&fx, &event).await.is_some(), "and its body");

        let (status, answer) = call(Method::POST, &path, Some(purge)).await;
        assert_eq!(
            (status, answer["count"].clone()),
            (StatusCode::OK, json!(1)),
            "{answer}"
        );
        assert!(body(&fx, &event).await.is_none(), "the body is deleted");
        assert!(
            app_events::Entity::find_by_id(event.id)
                .one(&fx.t.db)
                .await
                .expect("read the event")
                .is_none(),
            "and the event"
        );
    })
    .await;
}
