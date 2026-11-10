//! The inbox's filters, its `before` paging, each item's place in the tree,
//! and a read-all scoped by the same filter.

use axum::http::StatusCode;
use oxy_api_notifications::filter::FilterParams;
use oxy_api_notifications::handlers::read_all;
use oxy_api_notifications::inbox::{InboxQuery, item as one_item, page};
use oxy_api_notifications::inbox_view::Links;
use oxy_api_notifications::send::thread_key;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::fixture::{Signal, World, read_at, seed, signal};
use crate::common::Schema;
use crate::oltp_fx::with_fx_on;

/// The member's inbox, newest first:
/// e `oxy.run_failed` (Oxygen › runs), d `old_kind` (general, read),
/// c `shift_note` (general), b `temp_alarm` (operations/food-safety, thread),
/// a `list_overdue` (operations, thread) — and one of the other member's.
struct Seeded {
    a: Uuid,
    b: Uuid,
    c: Uuid,
    d: Uuid,
    e: Uuid,
    thread: String,
}

async fn seed_inbox(db: &DatabaseConnection, org: Uuid, w: &World) -> Seeded {
    let thread = thread_key(Some(w.app.id), "store:clovis");
    let app = Some(w.app.id);
    let mine = |kind, ago, thread: Option<&String>, read, app| Signal {
        org,
        user: w.member.id,
        app,
        kind,
        thread: thread.cloned(),
        ago,
        read,
    };
    let a = signal(db, mine("list_overdue", 50, Some(&thread), false, app)).await;
    let b = signal(db, mine("temp_alarm", 40, Some(&thread), false, app)).await;
    let c = signal(db, mine("shift_note", 30, None, false, app)).await;
    let d = signal(db, mine("old_kind", 20, None, true, app)).await;
    let e = signal(db, mine("oxy.run_failed", 10, None, false, None)).await;
    let theirs = Signal {
        user: w.other.id,
        ..mine("list_overdue", 5, None, false, app)
    };
    signal(db, theirs).await;
    Seeded {
        a,
        b,
        c,
        d,
        e,
        thread,
    }
}

fn query() -> InboxQuery {
    InboxQuery {
        unread_only: false,
        limit: None,
        before: None,
        org_id: None,
        app_id: None,
        source: None,
        category: None,
        kind: None,
        thread: None,
    }
}

async fn ids(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    q: InboxQuery,
) -> Result<(Vec<Uuid>, u64, Option<Uuid>), StatusCode> {
    let r = page(db, user, &q, &Links::default()).await?;
    Ok((
        r.items.iter().map(|i| i.id).collect(),
        r.unread,
        r.next_before,
    ))
}

#[tokio::test]
async fn notification_categories_before_pages_through_without_skipping_or_repeating() {
    with_fx_on(Schema::All, |fx| async move {
        let w = seed(&fx.db, fx.org_id).await;
        let s = seed_inbox(&fx.db, fx.org_id, &w).await;
        let (db, me) = (&fx.db, &w.member);
        let q = |before| InboxQuery {
            limit: Some(2),
            before,
            ..query()
        };
        let first = ids(db, me, q(None)).await.expect("page 1");
        assert_eq!(first, (vec![s.e, s.d], 4, Some(s.d)));
        let second = ids(db, me, q(Some(s.d))).await.expect("page 2");
        assert_eq!(second, (vec![s.c, s.b], 4, Some(s.b)));
        let last = ids(db, me, q(Some(s.b))).await.expect("page 3");
        assert_eq!(last, (vec![s.a], 4, None), "no older page: null");
        // Somebody else's item is no cursor, and says nothing about itself.
        let theirs = ids(db, &w.other, query()).await.expect("theirs").0[0];
        assert_eq!(
            ids(db, me, q(Some(theirs))).await,
            Err(StatusCode::BAD_REQUEST)
        );
    })
    .await;
}

#[tokio::test]
async fn notification_categories_filters_narrow_by_source_subtree_kind_and_thread() {
    with_fx_on(Schema::All, |fx| async move {
        let w = seed(&fx.db, fx.org_id).await;
        let s = seed_inbox(&fx.db, fx.org_id, &w).await;
        let (db, me) = (&fx.db, &w.member);
        let app = Some(w.app.id.to_string());
        let by = |source: Option<String>, category: Option<&str>| InboxQuery {
            source,
            category: category.map(str::to_string),
            ..query()
        };
        let got = |q| async { ids(db, me, q).await.expect("page") };
        assert_eq!(
            got(by(app.clone(), Some("operations"))).await.0,
            vec![s.b, s.a]
        );
        assert_eq!(
            got(by(app.clone(), Some("operations/food-safety"))).await.0,
            vec![s.b]
        );
        // `general` holds the kind the catalogue places nowhere and the one it
        // never heard of.
        let general = got(by(app.clone(), Some("general"))).await;
        assert_eq!(
            (general.0, general.1),
            (vec![s.d, s.c], 1),
            "unread follows the filter"
        );
        assert_eq!(got(by(Some("oxygen".into()), None)).await.0, vec![s.e]);
        assert_eq!(
            got(by(Some("oxygen".into()), Some("runs"))).await.0,
            vec![s.e]
        );
        assert!(got(by(app.clone(), Some("finance"))).await.0.is_empty());
        let kind = InboxQuery {
            kind: Some("shift_note".into()),
            ..query()
        };
        assert_eq!(got(kind).await.0, vec![s.c]);
        let thread = InboxQuery {
            thread: Some(s.thread.clone()),
            ..query()
        };
        assert_eq!(got(thread).await.0, vec![s.b, s.a]);

        for bad in [
            by(None, Some("operations")),
            by(Some("nope".into()), None),
            by(app.clone(), Some("Bad Path")),
        ] {
            assert_eq!(ids(db, me, bad).await, Err(StatusCode::BAD_REQUEST));
        }
    })
    .await;
}

#[tokio::test]
async fn notification_categories_each_item_carries_its_place_level_thread_and_actions() {
    with_fx_on(Schema::All, |fx| async move {
        let w = seed(&fx.db, fx.org_id).await;
        let s = seed_inbox(&fx.db, fx.org_id, &w).await;
        let links = Links {
            href_base: String::new(),
            admin_base: Some("https://app.example".into()),
        };
        let r = page(&fx.db, &w.member, &query(), &links).await.expect("page");
        let item = |id| r.items.iter().find(|i| i.id == id).expect("listed");
        let alarm = serde_json::to_value(item(s.b)).unwrap();
        assert_eq!(alarm["category"]["path"], "operations/food-safety");
        assert_eq!(
            alarm["category"]["labels"],
            serde_json::json!(["Store operations", "Food safety"])
        );
        assert_eq!(alarm["kind_label"], "Temperature alarm");
        assert_eq!(alarm["level"], "critical");
        assert_eq!(alarm["thread"], s.thread.as_str());
        assert_eq!(
            alarm["actions"],
            serde_json::json!([{ "id": "ack", "label": "Acknowledge", "destructive": false, "runs_function": true }])
        );
        let run = serde_json::to_value(item(s.e)).unwrap();
        assert_eq!(run["kind_label"], "Run failed");
        assert_eq!(run["category"]["labels"], serde_json::json!(["Automations and pipelines"]));
        assert_eq!(run["level"], "time_sensitive");
        assert_eq!(run["app"], serde_json::Value::Null);
        let old = serde_json::to_value(item(s.d)).unwrap();
        assert_eq!((old["category"]["path"].as_str(), old["kind_label"].as_str()), (Some("general"), Some("Old kind")));
        assert_eq!(old["url"], serde_json::Value::Null, "no link, no url");

        // `GET /api/notifications/{id}`: the same item, mine only.
        let one = one_item(&fx.db, &w.member, s.b, &links).await.expect("mine");
        assert_eq!(serde_json::to_value(&one).unwrap(), alarm);
        assert_eq!(
            one_item(&fx.db, &w.other, s.b, &links).await.err(),
            Some(StatusCode::NOT_FOUND),
            "somebody else's is not found, not forbidden"
        );
    })
    .await;
}

#[tokio::test]
async fn notification_categories_a_scoped_read_all_leaves_the_rest_unread() {
    with_fx_on(Schema::All, |fx| async move {
        let w = seed(&fx.db, fx.org_id).await;
        let s = seed_inbox(&fx.db, fx.org_id, &w).await;
        let scoped = FilterParams {
            source: Some(w.app.id.to_string()),
            category: Some("operations".into()),
            ..Default::default()
        };
        assert_eq!(read_all(&fx.db, &w.member, &scoped).await, Ok(2));
        assert!(read_at(&fx.db, s.a).await.is_some());
        assert!(read_at(&fx.db, s.b).await.is_some());
        assert!(
            read_at(&fx.db, s.c).await.is_none(),
            "general is not operations"
        );
        assert!(
            read_at(&fx.db, s.e).await.is_none(),
            "Oxygen is not the app"
        );
        let (_, unread, _) = ids(&fx.db, &w.other, query()).await.expect("theirs");
        assert_eq!(unread, 1, "nobody else's inbox moves");
        let bad = FilterParams {
            category: Some("operations".into()),
            ..Default::default()
        };
        assert_eq!(
            read_all(&fx.db, &w.member, &bad).await,
            Err(StatusCode::BAD_REQUEST)
        );
    })
    .await;
}
