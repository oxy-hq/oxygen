//! The client contract (R23): `sdk/cli/src/commands/events.ts` is the client,
//! and each key it reads from a reply is pinned here, so a rename on either
//! side fails a test rather than printing `-`.

use axum::http::{Method, StatusCode};
use chrono::{Duration, Utc};
use serde_json::{Value, json};

use super::{Chain, Fix};
use crate::staging_functions::{create_sandbox, make_guest_staff};

/// The keys the client reads from a `types` entry.
const TYPE_KEYS: &[&str] = &[
    "type",
    "publisher",
    "schemaRevision",
    "lastPublishedAt",
    "listeners",
];
/// …and from each of its listeners.
const LISTENER_KEYS: &[&str] = &[
    "listener",
    "environment",
    "pending",
    "retrying",
    "dead",
    "held",
    "oldestPendingAgeSeconds",
    "lastSuccessAt",
    "lastFailureAt",
    "lastFailureOutcome",
    "inFlight",
    "buildId",
    "reconciledAt",
];
/// The keys `printEvent` reads from an event (`show`, `find`, each of `?root=`'s).
const EVENT_KEYS: &[&str] = &[
    "id",
    "type",
    "key",
    "environment",
    "publishedAt",
    "deliveries",
];
/// …from the single-event reply beyond those: the chain, `--chain`'s root.
const SHOW_KEYS: &[&str] = &["rootId", "parents", "children", "data", "dataUnavailable"];
/// …from each delivery.
const DELIVERY_KEYS: &[&str] = &["listener", "environment", "status", "attempts"];
/// …from each attempt (`attemptLine`).
const ATTEMPT_KEYS: &[&str] = &["attempt", "outcome", "startedAt", "invocationId", "error"];
/// …from each related event in the chain.
const RELATED_KEYS: &[&str] = &["id", "type"];
/// …from a feed item (`feedLine`), every kind; a delivery's also names its listener.
const FEED_KEYS: &[&str] = &["at", "kind", "type", "eventId"];
const FEED_DELIVERY_KEYS: &[&str] = &["listener", "status"];
/// …from a dead group, and each of its deliveries.
const GROUP_KEYS: &[&str] = &["signature", "count", "deliveries"];
const DEAD_KEYS: &[&str] = &["eventId", "listener", "attempts"];

fn has_keys(value: &Value, keys: &[&str], what: &str) {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("{what} is an object: {value}"));
    for key in keys {
        assert!(object.contains_key(*key), "{what} lacks `{key}`: {value}");
    }
}

fn first<'a>(value: &'a Value, key: &str) -> &'a Value {
    value[key]
        .as_array()
        .and_then(|list| list.first())
        .unwrap_or_else(|| panic!("a non-empty `{key}`: {value}"))
}

async fn ok(method: Method, path: &str, body: Option<Value>) -> Value {
    let (status, answer) = super::call(method.clone(), path, body).await;
    assert_eq!(status, StatusCode::OK, "{method} {path}: {answer}");
    answer
}

#[tokio::test]
async fn every_reply_carries_the_keys_the_cli_reads() {
    let f = Fix::new().await;
    // After the fixture: its database harness clears `OXY_OWNER`.
    make_guest_staff();
    let org = format!("/customer-apps/{}", f.org());
    let app = format!("{org}/{}", f.listener_slug);
    let since = (Utc::now() - Duration::minutes(5)).to_rfc3339();
    let root = f.publish("root").await;
    let event = f
        .publish_in(
            "statement:1",
            Some(Chain {
                parent: root,
                root,
                depth: 1,
            }),
        )
        .await;
    f.set_delivery(event, "dead", 1).await;
    f.attempt(event, 1, Some("error"), Some("boom"), 10).await;

    let types = ok(Method::GET, &format!("{org}/events/types"), None).await;
    let entry = first(&types, "types");
    has_keys(entry, TYPE_KEYS, "a type");
    has_keys(first(entry, "listeners"), LISTENER_KEYS, "a listener");

    let show = ok(Method::GET, &format!("{org}/events/{event}"), None).await;
    has_keys(&show, EVENT_KEYS, "an event");
    has_keys(&show, SHOW_KEYS, "the event route's reply");
    let delivery = first(&show, "deliveries");
    has_keys(delivery, DELIVERY_KEYS, "a delivery");
    has_keys(first(delivery, "attempts"), ATTEMPT_KEYS, "an attempt");
    has_keys(first(&show, "parents"), RELATED_KEYS, "a parent");

    let find = format!(
        "{org}/events/find?type={}&key=statement%3A1",
        f.event_type().replace('/', "%2F")
    );
    let found = ok(Method::GET, &find, None).await;
    has_keys(&found, EVENT_KEYS, "a found event");
    has_keys(&found, SHOW_KEYS, "the find route's reply");

    let chain = ok(Method::GET, &format!("{org}/events?root={root}"), None).await;
    has_keys(&chain, &["rootId", "events"], "the chain");
    has_keys(first(&chain, "events"), EVENT_KEYS, "an event of the chain");

    let feed_path = format!(
        "{app}/events/feed?since={}",
        since.replace(':', "%3A").replace('+', "%2B")
    );
    let feed = ok(Method::GET, &feed_path, None).await;
    has_keys(&feed, &["items", "cursor"], "the feed");
    let item = first(&feed, "items");
    has_keys(item, FEED_KEYS, "a feed item");
    has_keys(item, FEED_DELIVERY_KEYS, "a delivery item");

    let dead = ok(Method::GET, &format!("{app}/events/dead"), None).await;
    let group = first(&dead, "groups");
    has_keys(group, GROUP_KEYS, "a dead group");
    has_keys(first(group, "deliveries"), DEAD_KEYS, "a dead delivery");

    for (path, body) in [
        (
            format!("{org}/events/{event}/redeliver"),
            json!({ "dryRun": true }),
        ),
        (
            format!("{org}/events/redeliver"),
            json!({ "to": f.to(), "status": "dead", "dryRun": true }),
        ),
        (
            format!("{org}/events/replay"),
            json!({ "type": f.event_type(), "since": since, "to": f.to(),
                    "environment": "production", "dryRun": true }),
        ),
        (
            format!("{org}/events/discard"),
            json!({ "type": "nobody/nothing", "until": Utc::now().to_rfc3339() }),
        ),
        (
            format!("{org}/events/purge"),
            json!({ "type": "nobody/nothing" }),
        ),
    ] {
        let answer = ok(Method::POST, &path, Some(body)).await;
        has_keys(&answer, &["count"], &path);
    }

    create_sandbox(&f.t, f.listener, "c1", None).await;
    let switched = ok(
        Method::PUT,
        &format!("{app}/environments/dev-c1/events"),
        Some(json!({ "enabled": true })),
    )
    .await;
    has_keys(&switched, &["environment", "enabled"], "the switch");
}
