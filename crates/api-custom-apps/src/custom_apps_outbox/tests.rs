//! The outbox's object and listing, against the filesystem store.

use chrono::{DateTime, Duration, TimeZone, Utc};
use oxy_app_core::custom_app_environment::AppEnvironment;
use serde_json::{Map, json};
use uuid::Uuid;

use super::{Message, PREFIX, captured_at_of_key, list_newest_first, newest_keys, silo, write};
use crate::custom_apps_storage::{ListPage, RetentionPolicy, Silo, StorageError, StoredObject};

const APP: Uuid = Uuid::from_u128(7);

fn dev() -> AppEnvironment {
    AppEnvironment::parse("dev-a1").expect("a sandbox name")
}

fn message(subject: &str) -> Message {
    let mut details = Map::new();
    details.insert("cc".into(), json!(["controller@customer.example"]));
    Message {
        kind: "email",
        to: json!(["cfo@customer.example"]),
        subject: Some(subject.to_string()),
        title: None,
        body: Some("posted".to_string()),
        invocation_id: Uuid::from_u128(9),
        function: "send".to_string(),
        captured_at: String::new(),
        details,
    }
}

fn at(seconds: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 3, 12, 0, 0).unwrap() + Duration::seconds(seconds)
}

/// The test store: a directory of its own, as the build store's tests do
/// (nextest runs each test in a process, so the variable is the test's).
fn local_store() {
    let dir = std::env::temp_dir().join(format!("oxy-outbox-{}", Uuid::new_v4()));
    // SAFETY: set before any storage call, in a process of its own.
    unsafe {
        std::env::remove_var("OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET");
        std::env::set_var("OXY_STATE_DIR", dir);
    }
}

async fn keep(silo: &Silo, message: Message, seconds: i64) -> String {
    let (pathname, bytes) = message
        .encode(at(seconds), Uuid::new_v4())
        .expect("encodes");
    write(silo, &pathname, bytes, &RetentionPolicy::default())
        .await
        .expect("written")
}

#[test]
fn an_object_is_named_by_its_inverted_time_and_carries_the_fields() {
    let id = Uuid::from_u128(5);
    let (pathname, bytes) = message("JE posted").encode(at(0), id).expect("encodes");
    // 9999999999999 - 1793707200000 (2026-11-03T12:00:00Z in milliseconds).
    assert_eq!(
        pathname,
        format!("{PREFIX}8206292799999-2026-11-03T12:00:00.000Z-{id}.json")
    );
    let object: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
    assert_eq!(object["kind"], "email");
    assert_eq!(object["to"], json!(["cfo@customer.example"]));
    assert_eq!(object["subject"], "JE posted");
    assert_eq!(object["body"], "posted");
    assert_eq!(object["invocation_id"], Uuid::from_u128(9).to_string());
    assert_eq!(object["function"], "send");
    assert_eq!(object["captured_at"], "2026-11-03T12:00:00.000Z");
    assert_eq!(object["cc"], json!(["controller@customer.example"]));
    assert!(
        object.get("title").is_none(),
        "a field a kind lacks is absent"
    );
}

#[test]
fn an_outbox_silo_reads_no_fallback_and_is_the_environments_own() {
    let outbox = silo(APP, &dev());
    assert_eq!(outbox.fallback(), None);
    assert_eq!(outbox.prefix(), Silo::for_environment(APP, &dev()).prefix());
}

/// Newest first, `limit` of them, and `has_more` says there are older ones.
#[tokio::test]
async fn a_listing_is_newest_first_and_bounded() {
    local_store();
    let outbox = silo(APP, &dev());
    for (subject, seconds) in [("second", 10), ("first", 0), ("third", 20)] {
        keep(&outbox, message(subject), seconds).await;
    }
    let page = list_newest_first(&outbox, 2, None).await.expect("lists");
    let subjects: Vec<_> = page.messages.iter().map(|m| m["subject"].clone()).collect();
    assert_eq!(subjects, [json!("third"), json!("second")]);
    assert!(page.has_more);
    let all = list_newest_first(&outbox, 50, None).await.expect("lists");
    assert_eq!(all.messages.len(), 3);
    assert!(!all.has_more);
    assert_eq!(all.messages[2]["subject"], "first");
}

/// Another environment's outbox, and a production object under the same
/// relative key, are not this environment's mail.
#[tokio::test]
async fn a_listing_reads_the_environments_silo_alone() {
    local_store();
    let mine = silo(APP, &dev());
    let neighbour = silo(
        APP,
        &AppEnvironment::parse("dev-b2").expect("a sandbox name"),
    );
    let production = Silo::production(APP);
    keep(&mine, message("mine"), 0).await;
    keep(&neighbour, message("neighbour's"), 1).await;
    keep(&production, message("production's"), 2).await;
    let page = list_newest_first(&mine, 50, None).await.expect("lists");
    assert_eq!(page.messages.len(), 1);
    assert_eq!(page.messages[0]["subject"], "mine");
}

#[tokio::test]
async fn an_empty_outbox_lists_nothing() {
    local_store();
    let page = list_newest_first(&silo(APP, &dev()), 50, None)
        .await
        .expect("lists");
    assert!(page.messages.is_empty());
    assert!(!page.has_more);
}

/// The time is read back from the name storage kept: `:` turned into `_`.
#[tokio::test]
async fn a_key_carries_the_time_it_was_captured() {
    local_store();
    let key = keep(&silo(APP, &dev()), message("m"), 7).await;
    assert_eq!(captured_at_of_key(&key), Some(at(7)));
    for not_ours in [
        "",
        "x.json",
        "customer-app-storage/a/_outbox/not-a-time-at-all-zzzzzz.json",
        // The old, time-first form is nobody's.
        "_outbox/2026-11-03T12_00_00.000Z-0000.json",
        "_outbox/820629279999x-2026-11-03T12_00_00.000Z-0000.json",
        "_outbox/8206292799999-not-a-time-at-all-zzzzz-0000.json",
    ] {
        assert_eq!(captured_at_of_key(not_ours), None, "{not_ours:?}");
    }
}

/// `since` drops what was captured before it — and before the limit counts:
/// two kept messages with the limit at two are both returned, however many
/// older ones there are, and `has_more` does not count the older ones.
#[tokio::test]
async fn a_listing_since_an_instance_drops_earlier_messages_before_the_limit() {
    local_store();
    let outbox = silo(APP, &dev());
    for seconds in [0, 1, 2] {
        keep(&outbox, message(&format!("old-{seconds}")), seconds).await;
    }
    for seconds in [10, 11] {
        keep(&outbox, message(&format!("new-{seconds}")), seconds).await;
    }
    let page = list_newest_first(&outbox, 2, Some(at(10)))
        .await
        .expect("lists");
    let subjects: Vec<_> = page.messages.iter().map(|m| m["subject"].clone()).collect();
    assert_eq!(
        subjects,
        [json!("new-11"), json!("new-10")],
        "at the boundary counts"
    );
    assert!(!page.has_more, "older instances' messages are not 'more'");
    let later = list_newest_first(&outbox, 50, Some(at(11)))
        .await
        .expect("lists");
    assert_eq!(later.messages.len(), 1);
    let none = list_newest_first(&outbox, 50, Some(at(99)))
        .await
        .expect("lists");
    assert!(none.messages.is_empty());
}

/// Keys sort ascending as the store lists them, so a later capture sorts
/// first, in any month or year, and the name stays fixed-width.
#[test]
fn a_later_capture_sorts_before_an_earlier_one() {
    let name = |seconds: i64| {
        let (pathname, _) = message("m")
            .encode(at(seconds), Uuid::from_u128(1))
            .expect("encodes");
        pathname
    };
    let mut names = [name(0), name(1), name(-86_400 * 400), name(86_400 * 4000)];
    let widths: std::collections::HashSet<_> = names.iter().map(String::len).collect();
    assert_eq!(widths.len(), 1, "fixed width: {names:?}");
    names.sort();
    let times: Vec<_> = names
        .iter()
        .map(|n| captured_at_of_key(n).expect("a time"))
        .collect();
    assert!(
        times.windows(2).all(|pair| pair[0] > pair[1]),
        "ascending names are newest first: {times:?}"
    );
}

/// A store double: `total` keys (newest first, as a store lists them) served
/// `size` at a time by offset cursor, counting the listings asked of it.
struct Store {
    keys: Vec<String>,
    calls: std::cell::Cell<usize>,
}

impl Store {
    fn of(seconds: impl IntoIterator<Item = i64>) -> Self {
        let mut keys: Vec<String> = seconds
            .into_iter()
            .map(|s| {
                message("m")
                    .encode(at(s), Uuid::new_v4())
                    .expect("encodes")
                    .0
            })
            .collect();
        keys.sort();
        Self {
            keys,
            calls: 0.into(),
        }
    }

    async fn page(&self, size: usize, cursor: Option<String>) -> Result<ListPage, StorageError> {
        self.calls.set(self.calls.get() + 1);
        let from: usize = cursor.map_or(0, |c| c.parse().expect("an offset"));
        let end = (from + size).min(self.keys.len());
        let objects = self.keys[from..end]
            .iter()
            .map(|key| StoredObject {
                key: key.clone(),
                size: 1,
                content_type: None,
                last_modified: None,
            })
            .collect();
        let has_more = end < self.keys.len();
        Ok(ListPage {
            objects,
            cursor: has_more.then(|| end.to_string()),
            has_more,
        })
    }
}

/// A page of `limit` over a long outbox costs one listing, not a walk of the
/// whole prefix, and says there is more.
#[tokio::test]
async fn a_page_of_limit_touches_one_storage_page() {
    let store = Store::of(0..5000);
    let keys = newest_keys(50, None, |cursor| store.page(51, cursor))
        .await
        .expect("lists");
    assert_eq!(keys.len(), 51, "limit + 1: the extra decides has_more");
    assert_eq!(store.calls.get(), 1);
    assert_eq!(captured_at_of_key(&keys[0]), Some(at(4999)), "newest first");
}

/// Short store pages are followed only as far as the page needs.
#[tokio::test]
async fn the_walk_stops_once_it_holds_limit_plus_one_keys() {
    let store = Store::of(0..100);
    let keys = newest_keys(25, None, |cursor| store.page(10, cursor))
        .await
        .expect("lists");
    assert_eq!(keys.len(), 26);
    assert_eq!(store.calls.get(), 3, "10 + 10 + 6 of 100 keys");
    let all = newest_keys(500, None, |cursor| store.page(40, cursor))
        .await
        .expect("lists");
    assert_eq!(
        all.len(),
        100,
        "an outbox shorter than the limit is read out"
    );
}

/// With `since`, the first older key ends the walk: nothing after it is
/// newer. Keys that carry no time are skipped, not mistaken for the end.
#[tokio::test]
async fn since_ends_the_walk_at_the_first_older_key() {
    let mut store = Store::of(0..5000);
    store.keys.insert(1, "_outbox/legacy-key.json".to_string());
    let keys = newest_keys(10, Some(at(4990)), |cursor| store.page(4, cursor))
        .await
        .expect("lists");
    let times: Vec<_> = keys.iter().filter_map(|k| captured_at_of_key(k)).collect();
    let expected: Vec<_> = (4990..5000).rev().map(at).collect();
    assert_eq!(times, expected, "exactly the instance's messages");
    assert_eq!(
        store.calls.get(),
        3,
        "the 4000-odd older keys are not listed"
    );
}

/// Newest first and `has_more` over more keys than `limit`, end to end
/// against the filesystem store, across several storage pages.
#[tokio::test]
async fn a_long_outbox_pages_newest_first_with_has_more() {
    local_store();
    let outbox = silo(APP, &dev());
    for seconds in 0..7 {
        keep(&outbox, message(&format!("m-{seconds}")), seconds).await;
    }
    let page = list_newest_first(&outbox, 3, None).await.expect("lists");
    let subjects: Vec<_> = page.messages.iter().map(|m| m["subject"].clone()).collect();
    assert_eq!(subjects, [json!("m-6"), json!("m-5"), json!("m-4")]);
    assert!(page.has_more);
    let exact = list_newest_first(&outbox, 7, None).await.expect("lists");
    assert_eq!(exact.messages.len(), 7);
    assert!(!exact.has_more, "exactly limit messages is not 'more'");
}
