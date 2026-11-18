//! The read routes: `types`, one event, `find`, `?root=`, `feed` and `dead`.

use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::Value;
use uuid::Uuid;

use super::{Chain, FUNCTION, Fix, at};

/// The entry of `body`'s `types` for `event_type`.
fn type_entry<'a>(body: &'a Value, event_type: &str) -> &'a Value {
    body["types"]
        .as_array()
        .unwrap_or_else(|| panic!("a `types` list: {body}"))
        .iter()
        .find(|t| t["type"] == event_type)
        .unwrap_or_else(|| panic!("{event_type} is listed: {body}"))
}

/// The listener `to` in `environment` of a `types` entry.
fn listener_entry<'a>(entry: &'a Value, to: &str, environment: &str) -> &'a Value {
    entry["listeners"]
        .as_array()
        .unwrap_or_else(|| panic!("a `listeners` list: {entry}"))
        .iter()
        .find(|l| l["listener"] == to && l["environment"] == environment)
        .unwrap_or_else(|| panic!("{to} in {environment} is listed: {entry}"))
}

fn ids(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap_or_else(|| panic!("a list: {list}"))
        .iter()
        .map(|e| e["id"].as_str().expect("an id").to_string())
        .collect()
}

fn encoded(text: &str) -> String {
    text.replace('/', "%2F")
        .replace(':', "%3A")
        .replace('+', "%2B")
}

#[tokio::test]
async fn types_lists_publisher_and_listeners() {
    let f = Fix::new().await;
    f.publish("statement:1").await;

    let (status, body) = f
        .get(&format!("/customer-apps/{}/events/types", f.org()))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entry = type_entry(&body, &f.event_type());
    assert_eq!(entry["publisher"], f.publisher_slug.as_str());
    assert_eq!(entry["schemaRevision"], 1, "{entry}");
    assert!(entry["lastPublishedAt"].is_string(), "{entry}");
    let listener = listener_entry(entry, &f.to(), "production");
    assert_eq!(listener["app"], f.listener_slug.as_str());
    assert_eq!(listener["function"], FUNCTION);
    assert_eq!(listener["pending"], 1, "{listener}");
}

#[tokio::test]
async fn types_reports_listener_health_with_the_oldest_pending_age() {
    let f = Fix::new().await;
    let stuck = f.publish("stuck").await;
    f.run(
        "UPDATE app_event_deliveries SET created_at = now() - interval '10 minutes' \
          WHERE event_id = $1",
        vec![stuck.into()],
    )
    .await;
    let retrying = f.publish("retrying").await;
    f.set_delivery(retrying, "pending", 1).await;
    f.attempt(retrying, 1, Some("error"), Some("boom"), 30)
        .await;
    let running = f.publish("running").await;
    f.set_delivery(running, "pending", 1).await;
    f.attempt(running, 1, None, None, 5).await;
    let dead = f.publish("dead").await;
    f.set_delivery(dead, "dead", 5).await;
    f.attempt(dead, 5, Some("timeout"), Some("ran past 30 s"), 20)
        .await;
    let held = f.publish("held").await;
    f.set_delivery(held, "held", 0).await;
    let acked = f.publish("acked").await;
    f.set_delivery(acked, "acked", 1).await;
    f.attempt(acked, 1, Some("ok"), None, 60).await;

    let (status, body) = f
        .get(&format!("/customer-apps/{}/events/types", f.org()))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let health = listener_entry(type_entry(&body, &f.event_type()), &f.to(), "production");
    assert_eq!(health["pending"], 3, "{health}");
    assert_eq!(
        health["retrying"], 2,
        "pending with at least one attempt: {health}"
    );
    assert_eq!(health["inFlight"], 1, "{health}");
    assert_eq!(health["dead"], 1, "{health}");
    assert_eq!(health["held"], 1, "{health}");
    let oldest = health["oldestPendingAgeSeconds"]
        .as_f64()
        .unwrap_or_else(|| panic!("an age: {health}"));
    assert!((600.0..3600.0).contains(&oldest), "{oldest}");
    let success = at(&health["lastSuccessAt"]);
    assert!(Utc::now() - success < Duration::minutes(2), "{success}");
    at(&health["lastFailureAt"]);
    assert_eq!(
        health["lastFailureOutcome"], "timeout",
        "the latest failure: {health}"
    );
}

#[tokio::test]
async fn types_reports_where_each_subscription_came_from() {
    let f = Fix::new().await;
    let row = f
        .db()
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT build_id, updated_at FROM app_event_subscriptions \
              WHERE app_id = $1 AND environment = 'production'",
            [f.listener.into()],
        ))
        .await
        .expect("read the subscription")
        .expect("the listener's subscription");
    let build: Uuid = row.try_get("", "build_id").expect("build_id");
    let reconciled: DateTime<Utc> = row.try_get("", "updated_at").expect("updated_at");

    let (status, body) = f
        .get(&format!("/customer-apps/{}/events/types", f.org()))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entry = type_entry(&body, &f.event_type());
    assert!(
        entry["lastPublishedAt"].is_null(),
        "nothing published: {entry}"
    );
    let listener = listener_entry(entry, &f.to(), "production");
    assert_eq!(listener["buildId"], build.to_string(), "{listener}");
    assert_eq!(at(&listener["reconciledAt"]), reconciled, "{listener}");
    assert_eq!(listener["pending"], 0);
    assert!(listener["oldestPendingAgeSeconds"].is_null(), "{listener}");
}

#[tokio::test]
async fn show_returns_the_causation_chain() {
    let f = Fix::new().await;
    let root = f.publish("root").await;
    let child = f
        .publish_in(
            "child",
            Some(Chain {
                parent: root,
                root,
                depth: 1,
            }),
        )
        .await;
    let grandchild = f
        .publish_in(
            "grandchild",
            Some(Chain {
                parent: child,
                root,
                depth: 2,
            }),
        )
        .await;
    f.set_delivery(child, "pending", 1).await;
    f.attempt(child, 1, Some("error"), Some("boom"), 10).await;

    let (status, body) = f
        .get(&format!("/customer-apps/{}/events/{child}", f.org()))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], child.to_string());
    assert_eq!(body["type"], f.event_type());
    assert_eq!(body["key"], "child");
    assert_eq!(body["rootId"], root.to_string());
    assert_eq!(body["causationId"], root.to_string());
    assert_eq!(ids(&body["parents"]), vec![root.to_string()]);
    assert_eq!(ids(&body["children"]), vec![grandchild.to_string()]);
    let delivery = &body["deliveries"][0];
    assert_eq!(delivery["listener"], f.to());
    assert_eq!(delivery["status"], "pending");
    let attempt = &delivery["attempts"][0];
    assert_eq!(attempt["attempt"], 1, "{attempt}");
    assert_eq!(attempt["outcome"], "error");
    assert_eq!(attempt["error"], "boom");
    // This org has no store: the body cannot be read, and the reply says so.
    assert!(body["data"].is_null(), "{body}");
    assert!(body["dataUnavailable"].is_string(), "{body}");

    let (status, body) = f
        .get(&format!(
            "/customer-apps/{}/events/{}",
            f.org(),
            Uuid::new_v4()
        ))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

#[tokio::test]
async fn find_by_key_returns_the_event() {
    let f = Fix::new().await;
    let id = f.publish("statement:7").await;
    let base = format!(
        "/customer-apps/{}/events/find?type={}",
        f.org(),
        encoded(&f.event_type())
    );
    for query in [
        "&key=statement%3A7",
        "&key=statement%3A7&environment=production",
    ] {
        let (status, body) = f.get(&format!("{base}{query}")).await;
        assert_eq!(status, StatusCode::OK, "{query}: {body}");
        assert_eq!(body["id"], id.to_string(), "{query}");
        assert!(
            body["deliveries"].is_array(),
            "the event route's body: {body}"
        );
    }
    let (status, body) = f.get(&format!("{base}&key=nope")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

#[tokio::test]
async fn the_root_query_returns_the_whole_chain() {
    let f = Fix::new().await;
    let root = f.publish("root").await;
    let chain = |parent, depth| {
        Some(Chain {
            parent,
            root,
            depth,
        })
    };
    let child = f.publish_in("child", chain(root, 1)).await;
    let grandchild = f.publish_in("grandchild", chain(child, 2)).await;
    f.publish("unrelated").await;
    for (event, minutes) in [(root, 3), (child, 2), (grandchild, 1)] {
        f.run(
            "UPDATE app_events SET published_at = now() - make_interval(mins => $2) WHERE id = $1",
            vec![event.into(), minutes.into()],
        )
        .await;
    }

    let (status, body) = f
        .get(&format!("/customer-apps/{}/events?root={root}", f.org()))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rootId"], root.to_string());
    assert_eq!(
        ids(&body["events"]),
        vec![root.to_string(), child.to_string(), grandchild.to_string()],
        "in publish order"
    );
}

#[tokio::test]
async fn the_feed_lists_publishes_and_deliveries_after_since() {
    let f = Fix::new().await;
    let old = f.publish("old").await;
    f.run(
        "UPDATE app_events SET relayed_at = now() - interval '1 hour' WHERE id = $1",
        vec![old.into()],
    )
    .await;
    f.run(
        "UPDATE app_event_deliveries SET updated_at = now() - interval '1 hour' \
          WHERE event_id = $1",
        vec![old.into()],
    )
    .await;
    let since = (Utc::now() - Duration::minutes(1)).to_rfc3339();
    let new = f.publish("new").await;
    let query = format!("?since={}&environment=production", encoded(&since));

    let feed = |app: &str| format!("/customer-apps/{}/{app}/events/feed{query}", f.org());
    let (status, body) = f.get(&feed(&f.publisher_slug)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().expect("items");
    assert_eq!(
        items.len(),
        1,
        "the publish since, not the older one: {body}"
    );
    assert_eq!(items[0]["kind"], "published");
    assert_eq!(items[0]["eventId"], new.to_string());
    assert!(at(&body["cursor"]) >= at(&items[0]["at"]), "{body}");

    let (status, body) = f.get(&feed(&f.listener_slug)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{body}");
    assert_eq!(items[0]["kind"], "delivery");
    assert_eq!(items[0]["eventId"], new.to_string());
    assert_eq!(items[0]["listener"], f.to());
    assert_eq!(items[0]["status"], "pending");
}

#[tokio::test]
async fn dead_groups_by_error_signature_and_lists_attempts() {
    let f = Fix::new().await;
    let twice = f.publish("twice").await;
    f.set_delivery(twice, "dead", 2).await;
    f.attempt(
        twice,
        1,
        Some("error"),
        Some("boom\n  at handler (fn.js:3)"),
        40,
    )
    .await;
    f.attempt(
        twice,
        2,
        Some("error"),
        Some("boom\n  at handler (fn.js:3)"),
        30,
    )
    .await;
    let once = f.publish("once").await;
    f.set_delivery(once, "dead", 1).await;
    f.attempt(
        once,
        1,
        Some("error"),
        Some("boom\n  at other (fn.js:9)"),
        20,
    )
    .await;
    let slow = f.publish("slow").await;
    f.set_delivery(slow, "dead", 1).await;
    f.attempt(slow, 1, Some("timeout"), Some("ran past 30 s"), 10)
        .await;
    f.publish("pending").await;

    let (status, body) = f
        .get(&format!(
            "/customer-apps/{}/{}/events/dead?environment=production",
            f.org(),
            f.listener_slug
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let groups = body["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 2, "{body}");
    let group = |signature: &str| {
        groups
            .iter()
            .find(|g| g["signature"] == signature)
            .unwrap_or_else(|| panic!("{signature}: {body}"))
    };
    let boom = group("error: boom");
    assert_eq!(boom["count"], 2);
    let mut events: Vec<String> = boom["deliveries"]
        .as_array()
        .expect("deliveries")
        .iter()
        .map(|d| d["eventId"].as_str().expect("eventId").to_string())
        .collect();
    events.sort();
    let mut expected = vec![twice.to_string(), once.to_string()];
    expected.sort();
    assert_eq!(events, expected);
    let both = boom["deliveries"]
        .as_array()
        .expect("deliveries")
        .iter()
        .find(|d| d["eventId"] == twice.to_string())
        .expect("twice");
    assert_eq!(both["attempts"].as_array().map(Vec::len), Some(2), "{both}");
    assert_eq!(group("timeout: ran past 30 s")["count"], 1);
}
