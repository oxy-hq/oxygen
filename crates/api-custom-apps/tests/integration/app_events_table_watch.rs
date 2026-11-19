//! Table changes as events (app events design §7), against the real org
//! store and queue, with the Airhouse read replaced by a fake DuckLake
//! ([`install_reader`]): the tracker's pass over a watched table, each
//! snapshot's event with its deliveries and cursor, and what a person can do
//! about a cursor that cannot move. The pure shapes are unit-tested in
//! `oxy_custom_apps::events::table` and `table_watch::{reader, loop_guard}`;
//! here is what they are wired to.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::{
    app_event_deliveries as deliveries, app_event_subscriptions as subs,
    app_event_table_cursors as cursor_rows, app_events, workspaces,
};
use futures::FutureExt as _;
use oxy_api_custom_apps::app_events::deliveries::{DeliveryStart, begin};
use oxy_api_custom_apps::app_events::table_watch::cursors::{self, BEHIND_HORIZON, BROKEN};
use oxy_api_custom_apps::app_events::table_watch::reader::{
    Columns, ReadError, SnapshotRange, TableReader, install_reader,
};
use oxy_api_custom_apps::app_events::table_watch::track::{SNAPSHOTS_PER_PASS, track_once};
use oxy_api_custom_apps::app_events::table_watch::{breaker, resync};
use oxy_custom_apps::events::table::{ChangePage, Changed, Row};
use oxy_oltp::events::{OutboxTarget, PLATFORM_WRITER, read_body};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app_events_publish::enable_app_events;
use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, seeded_tenant, throwaway_org};
use crate::custom_app_functions_shape_zoo_oltp::AppStore;

const FUNCTION: &str = "on-lines";
const ECHO_JS: &str =
    "export default async (req) => Response.json({ body: JSON.parse(req.body) });";
/// Writes the table it was woken by.
const LOOP_JS: &str = r#"
export default async (req, ctx) => {
  await ctx.airhouse.exec("INSERT INTO TABLE_PLACEHOLDER VALUES (1)");
  return Response.json({ ok: true });
};
"#;

/// A DuckLake the tracker reads: what each question answers, as the test set it.
#[derive(Default)]
pub(crate) struct World {
    oldest: i64,
    newest: i64,
    /// What `snapshot_at` answers.
    at: Option<i64>,
    /// `schema.table` to its columns; a missing key is a table that is not there.
    columns: BTreeMap<String, Columns>,
    /// The snapshots that inserted or deleted, as the catalog records them.
    changed: Vec<i64>,
    changes: BTreeMap<i64, Changed>,
    /// Every `columns` question, in order.
    asked: Vec<String>,
    /// What `table_id` answers.
    table_id: i64,
}

pub(crate) struct Fake(Mutex<World>);

fn cols(names: &[(&str, &str)]) -> Columns {
    names
        .iter()
        .map(|(n, t)| (n.to_string(), t.to_string()))
        .collect()
}

fn row(id: i64) -> Row {
    let mut r = Row::new();
    r.insert("id".into(), json!(id));
    r
}

fn rows_changed(inserted: &[i64], deleted: &[i64]) -> Changed {
    Changed {
        inserted: inserted.len() as u64,
        deleted: deleted.len() as u64,
        updated: 0,
        rows: Some((
            inserted.iter().map(|i| row(*i)).collect(),
            deleted.iter().map(|i| row(*i)).collect(),
        )),
    }
}

#[async_trait]
impl TableReader for Fake {
    async fn snapshots(&self, _: Uuid) -> Result<SnapshotRange, ReadError> {
        let w = self.0.lock().unwrap();
        Ok(SnapshotRange {
            oldest: w.oldest,
            newest: w.newest,
        })
    }
    async fn snapshot_at(&self, _: Uuid, _: DateTime<Utc>) -> Result<Option<i64>, ReadError> {
        Ok(self.0.lock().unwrap().at)
    }
    async fn columns(
        &self,
        _: Uuid,
        schema: &str,
        table: &str,
    ) -> Result<Option<Columns>, ReadError> {
        let mut w = self.0.lock().unwrap();
        let key = format!("{schema}.{table}");
        w.asked.push(key.clone());
        Ok(w.columns.get(&key).cloned())
    }
    async fn changed_snapshots(
        &self,
        _: Uuid,
        _: &str,
        _: &str,
        after: i64,
        upto: i64,
    ) -> Result<Vec<i64>, ReadError> {
        let w = self.0.lock().unwrap();
        Ok(w.changed
            .iter()
            .copied()
            .filter(|s| *s > after && *s <= upto)
            .collect())
    }
    async fn changes_at(
        &self,
        _: Uuid,
        _: &str,
        _: &str,
        snapshot: i64,
    ) -> Result<Changed, ReadError> {
        let w = self.0.lock().unwrap();
        Ok(w.changes
            .get(&snapshot)
            .cloned()
            .unwrap_or_else(|| rows_changed(&[snapshot], &[])))
    }
    async fn table_id(&self, _: Uuid, schema: &str, table: &str) -> Result<Option<i64>, ReadError> {
        let w = self.0.lock().unwrap();
        // An environment's own copy is another table, with an id of its own.
        let copy = if schema.contains("__") { 100 } else { 0 };
        Ok(w.columns
            .contains_key(&format!("{schema}.{table}"))
            .then_some(w.table_id + copy))
    }
    async fn change_page(
        &self,
        _: Uuid,
        _: &str,
        _: &str,
        _: i64,
        _: usize,
        _: usize,
    ) -> Result<ChangePage, ReadError> {
        Ok(ChangePage::default())
    }
}

/// Install a fake world; the handle to change it between passes.
pub(crate) fn world(setup: impl FnOnce(&mut World)) -> Arc<Fake> {
    let mut w = World {
        oldest: 0,
        newest: 10,
        at: Some(5),
        ..Default::default()
    };
    w.columns.insert(
        "bookkeeping.lines".into(),
        cols(&[("id", "INTEGER"), ("amt", "INTEGER")]),
    );
    w.changed = vec![7, 9];
    w.changes.insert(7, rows_changed(&[1, 2], &[]));
    w.changes.insert(9, rows_changed(&[], &[3]));
    setup(&mut w);
    let fake = Arc::new(Fake(Mutex::new(w)));
    install_reader(fake.clone());
    fake
}

/// An org with app events on, an OLTP store and a workspace, and app(s) whose
/// functions run on tables.
pub(crate) struct Tw {
    pub(crate) t: Tenant,
    pub(crate) workspace: Uuid,
    store: AppStore,
    slug: String,
    _root: tempfile::TempDir,
}

impl Tw {
    pub(crate) async fn new() -> Self {
        let t = throwaway_org(&seeded_tenant().await).await;
        enable_app_events(&t, &[t.org_id]).await;
        let tag = Uuid::new_v4().simple().to_string()[..8].to_string();
        let root = tempfile::tempdir().expect("workspace dir");
        let workspace = Uuid::new_v4();
        workspaces::ActiveModel {
            id: Set(workspace),
            name: Set("Tables".into()),
            org_id: Set(Some(t.org_id)),
            path: Set(Some(root.path().to_string_lossy().into_owned())),
            ..Default::default()
        }
        .insert(&t.db)
        .await
        .expect("seed the workspace");
        let slug = format!("tw-{tag}");
        let store = AppStore::new(&t, &slug).await;
        store.provision(workspace).await;
        Self {
            t,
            workspace,
            store,
            slug,
            _root: root,
        }
    }

    /// Publish app `slug` promoted, its one function running `js` on `on`.
    pub(crate) async fn watcher(
        &self,
        slug: &str,
        on: Value,
        js: &'static str,
        extra: Value,
    ) -> Uuid {
        let mut manifest =
            json!({ "on": on, "timeoutSeconds": 30, "airhouse": { "enabled": true } });
        manifest
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().cloned().unwrap_or_default());
        let function = FunctionSpec {
            name: FUNCTION,
            manifest,
            js,
        };
        crate::app_events_relay::publish_app(
            &self.t,
            slug,
            self.workspace,
            "1",
            &[function],
            json!({}),
        )
        .await
    }

    pub(crate) fn org(&self) -> Uuid {
        self.t.org_id
    }

    pub(crate) fn slug(&self) -> &str {
        &self.slug
    }

    async fn cursor(&self, environment: &str) -> cursor_rows::Model {
        cursors::of_environment(&self.t.db, self.org(), environment)
            .await
            .expect("read the cursors")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("a cursor in {environment}"))
    }

    async fn events(&self, event_type: &str) -> Vec<app_events::Model> {
        app_events::Entity::find()
            .filter(app_events::Column::OrgId.eq(self.org()))
            .filter(app_events::Column::Type.eq(event_type))
            .order_by_asc(app_events::Column::Key)
            .all(&self.t.db)
            .await
            .expect("read the events")
    }

    async fn deliveries(&self, event: &app_events::Model) -> Vec<deliveries::Model> {
        deliveries::Entity::find()
            .filter(deliveries::Column::EventId.eq(event.id))
            .all(&self.t.db)
            .await
            .expect("read the deliveries")
    }

    async fn subscription(&self, app: Uuid) -> subs::Model {
        subs::Entity::find()
            .filter(subs::Column::AppId.eq(app))
            .filter(subs::Column::Environment.eq("production"))
            .one(&self.t.db)
            .await
            .expect("read the subscription")
            .expect("the subscription")
    }

    fn key(&self, snapshot: i64) -> String {
        format!("{}:{snapshot}", self.workspace)
    }
}

pub(crate) async fn with_tw<F, Fut>(body: F)
where
    F: FnOnce(Arc<Tw>) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let tw = Arc::new(Tw::new().await);
    let outcome = std::panic::AssertUnwindSafe(Box::pin(body(tw.clone())))
        .catch_unwind()
        .await;
    tw.store.cleanup().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

const SPEC: &str = "wh:bookkeeping.lines";
const TYPE: &str = "table:wh:bookkeeping.lines";

fn on_lines() -> Value {
    json!({ "table": SPEC })
}

#[tokio::test]
async fn a_changed_snapshot_is_one_event_with_its_delivery_and_the_cursor_moves_past_it() {
    with_tw(|tw| async move {
        let _ = world(|_| {});
        let app = tw
            .watcher(&format!("{}-a", tw.slug), on_lines(), ECHO_JS, json!({}))
            .await;
        assert_eq!(
            tw.cursor("production").await.snapshot_id,
            None,
            "unplaced until the first pass"
        );

        let stats = track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        assert_eq!((stats.published, stats.deliveries), (2, 2));

        // Placed at the snapshot newest when it was made (5), then 7 and 9 changed;
        // 6, 8 and 10 changed nothing it holds (compaction, other tables) and publish none.
        let events = tw.events(TYPE).await;
        assert_eq!(
            events.iter().map(|e| e.key.clone()).collect::<Vec<_>>(),
            [tw.key(7), tw.key(9)]
        );
        assert!(
            events
                .iter()
                .all(|e| e.source_app_id.is_none() && e.writer_role == PLATFORM_WRITER)
        );
        let cursor = tw.cursor("production").await;
        assert_eq!(
            (cursor.snapshot_id, cursor.head_snapshot),
            (Some(10), Some(10))
        );
        assert_eq!(cursor.state, "active");

        // The body is in the org's store, not in Oxy's Postgres.
        let body = read_body(
            &tw.t.db,
            tw.org(),
            &OutboxTarget::Production,
            PLATFORM_WRITER,
            events[0].id,
        )
        .await
        .expect("read the body")
        .expect("the body is there");
        assert_eq!(body["snapshotId"], 7);
        assert_eq!(body["inserted"], 2);
        assert_eq!(body["rows"]["inserted"][1]["id"], 2);

        // What the listener is handed: type, source, dataschema and the body, read at run time.
        let delivery = tw.deliveries(&events[0]).await.remove(0);
        assert_eq!(delivery.app_id, app);
        let DeliveryStart::Run { envelope, .. } =
            begin(&tw.t.db, delivery.id).await.expect("begin")
        else {
            panic!("the delivery is runnable");
        };
        assert_eq!(envelope.event_type, TYPE);
        assert_eq!(envelope.source, format!("/tables/{SPEC}"));
        assert_eq!(envelope.dataschema, "oxy/table.change@1");
        assert_eq!(envelope.data["snapshotId"], 7);

        // Nothing new: nothing more.
        let again = track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        assert_eq!(again.published, 0);
        assert_eq!(tw.events(TYPE).await.len(), 2);
    })
    .await;
}

#[tokio::test]
async fn a_watch_starts_at_now_and_never_replays_what_came_before() {
    with_tw(|tw| async move {
        let _ = world(|w| w.at = Some(8));
        tw.watcher(&format!("{}-a", tw.slug), on_lines(), ECHO_JS, json!({}))
            .await;
        track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        let keys: Vec<String> = tw.events(TYPE).await.into_iter().map(|e| e.key).collect();
        assert_eq!(keys, [tw.key(9)], "7 came before the watch");
    })
    .await;
}

#[tokio::test]
async fn compaction_alone_publishes_nothing_and_still_moves_the_cursor() {
    with_tw(|tw| async move {
        let fake = world(|w| w.changed = Vec::new());
        tw.watcher(&format!("{}-a", tw.slug), on_lines(), ECHO_JS, json!({}))
            .await;
        track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        fake.0.lock().unwrap().newest = 30;
        let stats = track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        assert_eq!(stats.published, 0);
        assert!(tw.events(TYPE).await.is_empty());
        assert_eq!(tw.cursor("production").await.snapshot_id, Some(30));
    })
    .await;
}

#[tokio::test]
async fn two_passes_that_race_publish_a_snapshot_once() {
    with_tw(|tw| async move {
        let _ = world(|_| {});
        tw.watcher(&format!("{}-a", tw.slug), on_lines(), ECHO_JS, json!({}))
            .await;
        let (a, b) = tokio::join!(
            track_once(&tw.t.db, tw.org(), "production"),
            track_once(&tw.t.db, tw.org(), "production")
        );
        a.expect("pass a");
        b.expect("pass b");
        let events = tw.events(TYPE).await;
        assert_eq!(events.len(), 2, "one event per snapshot, never a copy");
        for event in &events {
            assert_eq!(
                tw.deliveries(event).await.len(),
                1,
                "one delivery per listener"
            );
        }
        assert_eq!(tw.cursor("production").await.snapshot_id, Some(10));
    })
    .await;
}

#[tokio::test]
async fn a_cursor_older_than_the_horizon_pages_and_publishes_nothing_until_resync() {
    with_tw(|tw| async move {
        let _ = world(|w| {
            w.oldest = 50;
            w.newest = 60;
            w.changed = vec![55];
        });
        let a = tw
            .watcher(&format!("{}-a", tw.slug), on_lines(), ECHO_JS, json!({}))
            .await;
        let b = tw
            .watcher(&format!("{}-b", tw.slug), on_lines(), ECHO_JS, json!({}))
            .await;
        let c = tw.cursor("production").await;
        cursors::place(&tw.t.db, c.id, None, 10, false)
            .await
            .expect("place");

        track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        let stuck = tw.cursor("production").await;
        assert_eq!(stuck.state, BEHIND_HORIZON);
        assert!(stuck.state_detail.unwrap().contains("resync"));
        assert!(tw.events(TYPE).await.is_empty());

        // Dry run: reports, changes nothing.
        let app = entity::apps::Entity::find_by_id(a)
            .one(&tw.t.db)
            .await
            .unwrap()
            .unwrap();
        let dry = resync::resync(&tw.t.db, &app, FUNCTION, "production", false, true)
            .await
            .expect("dry");
        assert_eq!((dry.tables[0].from, dry.tables[0].to), (Some(10), 60));
        assert_eq!(tw.cursor("production").await.state, BEHIND_HORIZON);

        // Resync with a backfill: the cursor at now, active, one event to `a` alone.
        let done = resync::resync(&tw.t.db, &app, FUNCTION, "production", true, false)
            .await
            .expect("resync");
        assert_eq!(done.queued, 1);
        let moved = tw.cursor("production").await;
        assert_eq!(
            (moved.state.as_str(), moved.snapshot_id),
            ("active", Some(60))
        );
        let events = tw.events(TYPE).await;
        assert_eq!(events.len(), 1, "the backfill alone: 55 was skipped");
        assert!(events[0].key.starts_with("backfill:"));
        let delivered = tw.deliveries(&events[0]).await;
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].app_id, a);
        assert_ne!(delivered[0].app_id, b, "the other listener is not woken");
        let DeliveryStart::Run { envelope, .. } =
            begin(&tw.t.db, delivered[0].id).await.expect("begin")
        else {
            panic!("runnable");
        };
        assert_eq!(
            envelope.data,
            json!({ "snapshotId": 60, "inserted": 0, "deleted": 0, "updated": 0, "backfill": true })
        );
    })
    .await;
}

#[tokio::test]
async fn a_column_removed_or_retyped_breaks_the_watch_until_it_holds_again() {
    with_tw(|tw| async move {
        let fake = world(|w| w.changed = Vec::new());
        tw.watcher(&format!("{}-a", tw.slug), on_lines(), ECHO_JS, json!({}))
            .await;
        track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        {
            let mut w = fake.0.lock().unwrap();
            w.newest = 20;
            w.changed = vec![15];
            w.columns.insert(
                "bookkeeping.lines".into(),
                cols(&[("id", "INTEGER"), ("amt", "VARCHAR")]),
            );
        }
        track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        let broken = tw.cursor("production").await;
        assert_eq!(broken.state, BROKEN);
        assert!(broken.state_detail.unwrap().contains("`amt` changed type"));
        assert!(
            tw.events(TYPE).await.is_empty(),
            "nothing is published while it is broken"
        );
        assert_eq!(broken.snapshot_id, Some(10), "and the cursor stays");

        fake.0.lock().unwrap().columns.insert(
            "bookkeeping.lines".into(),
            cols(&[("id", "INTEGER"), ("amt", "INTEGER"), ("extra", "TEXT")]),
        );
        track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        assert_eq!(
            tw.cursor("production").await.state,
            "active",
            "a column added keeps the contract"
        );
        assert_eq!(
            tw.events(TYPE).await.len(),
            1,
            "what changed while it was broken is delivered"
        );

        // A column removed on purpose: broken until resync forgets the old contract.
        fake.0
            .lock()
            .unwrap()
            .columns
            .insert("bookkeeping.lines".into(), cols(&[("id", "INTEGER")]));
        track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        assert_eq!(tw.cursor("production").await.state, BROKEN);
        let app = entity::apps::Entity::find()
            .filter(entity::apps::Column::OrgId.eq(tw.org()))
            .one(&tw.t.db)
            .await
            .unwrap()
            .unwrap();
        resync::resync(&tw.t.db, &app, FUNCTION, "production", false, false)
            .await
            .expect("resync");
        track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        assert_eq!(
            tw.cursor("production").await.state,
            "active",
            "the new columns are the contract"
        );
    })
    .await;
}

#[tokio::test]
async fn a_subscription_over_the_rate_is_paused_and_its_deliveries_held() {
    with_tw(|tw| async move {
        let total = breaker::MAX_DELIVERIES + 10;
        let fake = world(|w| {
            w.newest = 10 + total;
            w.changed = (11..=10 + total).collect();
            w.at = Some(10);
        });
        let app = tw
            .watcher(&format!("{}-a", tw.slug), on_lines(), ECHO_JS, json!({}))
            .await;
        for _ in 0..=(total as usize / SNAPSHOTS_PER_PASS) + 1 {
            track_once(&tw.t.db, tw.org(), "production")
                .await
                .expect("pass");
        }
        let sub = tw.subscription(app).await;
        assert!(sub.paused_at.is_some(), "the breaker tripped");
        assert!(sub.paused_reason.unwrap().contains("a loop"));
        let all = deliveries::Entity::find()
            .filter(deliveries::Column::AppId.eq(app))
            .all(&tw.t.db)
            .await
            .unwrap();
        let held = all.iter().filter(|d| d.status == "held").count();
        let pending = all.iter().filter(|d| d.status == "pending").count();
        assert_eq!(pending as i64, breaker::MAX_DELIVERIES);
        assert_eq!(
            held as i64, 10,
            "everything after the pause is recorded, held"
        );

        // resync resumes it, and a backfill reaches it: it is not held.
        let model = entity::apps::Entity::find_by_id(app)
            .one(&tw.t.db)
            .await
            .unwrap()
            .unwrap();
        let done = resync::resync(&tw.t.db, &model, FUNCTION, "production", true, false)
            .await
            .expect("resync");
        assert_eq!(done.queued, 1);
        assert!(tw.subscription(app).await.paused_at.is_none());
        let backfill = tw
            .events(TYPE)
            .await
            .into_iter()
            .find(|e| e.key.starts_with("backfill:"))
            .expect("the backfill event");
        assert_eq!(
            tw.deliveries(&backfill).await[0].status,
            "pending",
            "a backfill to a resumed subscription runs"
        );
        // The deliveries before the resync no longer count: one more change
        // does not pause it again.
        {
            let mut w = fake.0.lock().unwrap();
            w.newest += 1;
            let next = w.newest;
            w.changed = vec![next];
        }
        track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        assert!(
            tw.subscription(app).await.paused_at.is_none(),
            "resumed means the window starts over"
        );
    })
    .await;
}

#[tokio::test]
async fn a_cursor_goes_with_the_last_watch_and_a_new_watch_makes_one() {
    with_tw(|tw| async move {
        let _ = world(|_| {});
        let slug = format!("{}-a", tw.slug);
        tw.watcher(&slug, on_lines(), ECHO_JS, json!({})).await;
        assert_eq!(
            cursor_rows::Entity::find()
                .filter(cursor_rows::Column::OrgId.eq(tw.org()))
                .count(&tw.t.db)
                .await
                .unwrap(),
            2,
            "production and staging"
        );
        // The next build no longer watches it.
        let function = FunctionSpec {
            name: FUNCTION,
            manifest: json!({ "route": true }),
            js: ECHO_JS,
        };
        crate::app_events_relay::publish_app(
            &tw.t,
            &slug,
            tw.workspace,
            "2",
            &[function],
            json!({}),
        )
        .await;
        assert_eq!(
            cursor_rows::Entity::find()
                .filter(cursor_rows::Column::OrgId.eq(tw.org()))
                .count(&tw.t.db)
                .await
                .unwrap(),
            0
        );
    })
    .await;
}

#[tokio::test]
async fn staging_reads_its_own_copy_and_publishes_nothing_without_a_branch() {
    with_tw(|tw| async move {
        let table = format!("app_{}.lines", tw.slug.replace('-', "_"));
        let fake = world(|w| {
            let schema = table.split('.').next().unwrap().to_string();
            w.columns
                .insert(format!("{schema}.lines"), cols(&[("id", "INTEGER")]));
            w.columns.insert(
                format!("{schema}__staging.lines"),
                cols(&[("id", "INTEGER")]),
            );
        });
        let app = tw.watcher(
            &format!("{}-a", tw.slug),
            json!({ "table": table }),
            ECHO_JS,
            json!({}),
        )
        .await;
        // Switch off: nothing is read, nothing recorded, no place kept.
        track_once(&tw.t.db, tw.org(), "staging").await.expect("pass");
        assert!(fake.0.lock().unwrap().asked.is_empty(), "a staging watch with its switch off reads nothing");
        assert_eq!(tw.cursor("staging").await.snapshot_id, None);
        sea_orm::ConnectionTrait::execute_unprepared(
            &tw.t.db,
            &format!("UPDATE app_environments SET events_enabled = true WHERE app_id = '{app}' AND name = 'staging'"),
        )
        .await
        .expect("switch on");
        // The first pass after the switch places the cursor at now; a change
        // after that is what needs a branch to hold its body.
        track_once(&tw.t.db, tw.org(), "staging")
            .await
            .expect("place");
        {
            let mut w = fake.0.lock().unwrap();
            w.newest = 12;
            w.changed = vec![11];
        }
        let stats = track_once(&tw.t.db, tw.org(), "staging")
            .await
            .expect("pass");
        assert_eq!(
            stats.published, 0,
            "the org has no staging branch to hold the body"
        );
        let asked = fake.0.lock().unwrap().asked.clone();
        assert!(
            asked.iter().any(|a| a.ends_with("__staging.lines")),
            "the sibling was read first: {asked:?}"
        );
        let cursor = tw.cursor("staging").await;
        assert!(cursor.state_detail.unwrap().contains("no OLTP branch"));
    })
    .await;
}

#[tokio::test]
async fn a_function_woken_by_a_table_may_not_write_it() {
    with_tw(|tw| async move {
        let table = format!("app_{}.lines", tw.slug.replace('-', "_"));
        let _ = world(|w| {
            w.columns.insert(table.clone(), cols(&[("id", "INTEGER")]));
            w.changed = vec![9];
        });
        let js: &'static str = Box::leak(
            LOOP_JS
                .replace("TABLE_PLACEHOLDER", &table)
                .into_boxed_str(),
        );
        tw.watcher(
            &format!("{}-a", tw.slug),
            json!({ "table": table }),
            js,
            json!({ "airhouse": { "enabled": true } }),
        )
        .await;
        track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        let event = tw.events(&format!("table:{table}")).await.remove(0);
        let delivery = tw.deliveries(&event).await.remove(0);
        let run = crate::app_events_delivery::run_delivery(&tw.t.db, &delivery).await;
        let reloaded = deliveries::Entity::find_by_id(delivery.id)
            .one(&tw.t.db)
            .await
            .unwrap()
            .unwrap();
        let error = reloaded.last_error.unwrap_or_default();
        assert!(error.contains("EventLoop"), "{error} / {run:?}");
        assert_ne!(reloaded.status, "acked");
    })
    .await;
}

#[tokio::test]
async fn a_replaced_table_breaks_the_watch_until_resync_moves_it_onto_the_new_id() {
    with_tw(|tw| async move {
        let fake = world(|w| {
            w.changed = Vec::new();
            w.table_id = 3;
        });
        let app = tw
            .watcher(&format!("{}-a", tw.slug), on_lines(), ECHO_JS, json!({}))
            .await;
        track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        assert_eq!(tw.cursor("production").await.table_id, Some(3));
        {
            // CREATE OR REPLACE: the same name, a new id, new rows.
            let mut w = fake.0.lock().unwrap();
            w.table_id = 8;
            w.newest = 20;
            w.changed = vec![15];
        }
        track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        let broken = tw.cursor("production").await;
        assert_eq!(broken.state, BROKEN);
        assert!(broken.state_detail.unwrap().contains("replaced"));
        assert!(
            tw.events(TYPE).await.is_empty(),
            "nothing is delivered across a replace"
        );

        let model = entity::apps::Entity::find_by_id(app)
            .one(&tw.t.db)
            .await
            .unwrap()
            .unwrap();
        resync::resync(&tw.t.db, &model, FUNCTION, "production", false, false)
            .await
            .expect("resync");
        track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        let moved = tw.cursor("production").await;
        assert_eq!((moved.state.as_str(), moved.table_id), ("active", Some(8)));
    })
    .await;
}

#[tokio::test]
async fn apps_watching_one_table_share_a_cursor_and_a_resync_names_them() {
    with_tw(|tw| async move {
        let _ = world(|_| {});
        let a = tw
            .watcher(&format!("{}-a", tw.slug), on_lines(), ECHO_JS, json!({}))
            .await;
        let b = tw
            .watcher(&format!("{}-b", tw.slug), on_lines(), ECHO_JS, json!({}))
            .await;
        let model = |id| entity::apps::Entity::find_by_id(id).one(&tw.t.db);
        let a_model = model(a).await.unwrap().unwrap();
        let others = resync::sharers(&tw.t.db, &a_model, FUNCTION, "production")
            .await
            .expect("sharers");
        assert_eq!(others.iter().map(|o| o.id).collect::<Vec<_>>(), [b]);
        let alone = resync::sharers(&tw.t.db, &a_model, FUNCTION, "staging")
            .await
            .expect("sharers");
        assert_eq!(
            alone.len(),
            1,
            "staging has a subscription of the other app too"
        );
        let none = resync::sharers(&tw.t.db, &a_model, "no-such-function", "production")
            .await
            .expect("sharers");
        assert!(none.is_empty());
    })
    .await;
}

/// A log that purges the events it is asked to copy (as `oxyc events purge`
/// would between the mirror's read and its append), and remembers deletes.
struct PurgingLog {
    db: sea_orm::DatabaseConnection,
    deleted: Mutex<Vec<Uuid>>,
}

#[async_trait]
impl oxy_api_custom_apps::app_events::mirror::EventLog for PurgingLog {
    async fn append(
        &self,
        _: &sea_orm::DatabaseConnection,
        _: Uuid,
        rows: &[oxy_api_custom_apps::app_events::mirror::Logged],
    ) -> Result<(), oxy_api_custom_apps::app_events::mirror::SinkError> {
        for row in rows {
            app_events::Entity::delete_by_id(row.event_id)
                .exec(&self.db)
                .await
                .expect("purge");
        }
        Ok(())
    }
    async fn prune(
        &self,
        _: &sea_orm::DatabaseConnection,
        _: Uuid,
        _: i64,
    ) -> Result<(), oxy_api_custom_apps::app_events::mirror::SinkError> {
        Ok(())
    }
    async fn delete(
        &self,
        _: &sea_orm::DatabaseConnection,
        _: Uuid,
        ids: &[Uuid],
    ) -> Result<(), oxy_api_custom_apps::app_events::mirror::SinkError> {
        self.deleted.lock().unwrap().extend_from_slice(ids);
        Ok(())
    }
}

#[tokio::test]
async fn an_event_purged_while_it_is_being_copied_is_deleted_from_the_log_again() {
    with_tw(|tw| async move {
        let _ = world(|_| {});
        tw.watcher(&format!("{}-a", tw.slug), on_lines(), ECHO_JS, json!({}))
            .await;
        track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        let ids: Vec<Uuid> = tw.events(TYPE).await.into_iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), 2);
        let log = Arc::new(PurgingLog {
            db: tw.t.db.clone(),
            deleted: Mutex::new(Vec::new()),
        });
        oxy_api_custom_apps::app_events::mirror::install_log(log.clone());
        oxy_api_custom_apps::app_events::mirror::mirror_once(&tw.t.db)
            .await
            .expect("mirror");
        let mut deleted = log.deleted.lock().unwrap().clone();
        deleted.sort();
        let mut want = ids;
        want.sort();
        assert_eq!(
            deleted, want,
            "what the purge removed is not left in the log"
        );
    })
    .await;
}

/// A log that records what it is asked to copy.
struct RecordingLog(Mutex<Vec<Uuid>>);

#[async_trait]
impl oxy_api_custom_apps::app_events::mirror::EventLog for RecordingLog {
    async fn append(
        &self,
        _: &sea_orm::DatabaseConnection,
        _: Uuid,
        rows: &[oxy_api_custom_apps::app_events::mirror::Logged],
    ) -> Result<(), oxy_api_custom_apps::app_events::mirror::SinkError> {
        self.0
            .lock()
            .unwrap()
            .extend(rows.iter().map(|r| r.event_id));
        Ok(())
    }
    async fn prune(
        &self,
        _: &sea_orm::DatabaseConnection,
        _: Uuid,
        _: i64,
    ) -> Result<(), oxy_api_custom_apps::app_events::mirror::SinkError> {
        Ok(())
    }
    async fn delete(
        &self,
        _: &sea_orm::DatabaseConnection,
        _: Uuid,
        _: &[Uuid],
    ) -> Result<(), oxy_api_custom_apps::app_events::mirror::SinkError> {
        Ok(())
    }
}

#[tokio::test]
async fn events_of_orgs_with_the_flag_off_never_stall_the_mirror_for_the_others() {
    with_tw(|tw| async move {
        let _ = world(|_| {});
        tw.watcher(&format!("{}-a", tw.slug), on_lines(), ECHO_JS, json!({}))
            .await;
        track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        let ids: Vec<Uuid> = tw.events(TYPE).await.into_iter().map(|e| e.id).collect();
        // More than a batch of older events of an org that has the flag off.
        let other_org = Uuid::new_v4();
        crate::app_events_api::run_sql(
            &tw.t.db,
            "INSERT INTO app_events (id, org_id, workspace_id, environment, source_app_id, type, \
                key, data_bytes, writer_role, root_id, depth, published_at, relayed_at) \
             SELECT gen_random_uuid(), $1, $1, 'production', NULL, 'x/y', 'k' || g, 2, 'w', \
                gen_random_uuid(), 0, now() - interval '1 day', now() - interval '1 day' \
               FROM generate_series(1, 450) g",
            vec![other_org.into()],
        )
        .await;
        let log = Arc::new(RecordingLog(Mutex::new(Vec::new())));
        oxy_api_custom_apps::app_events::mirror::install_log(log.clone());
        oxy_api_custom_apps::app_events::mirror::mirror_once(&tw.t.db)
            .await
            .expect("mirror");
        let mut copied = log.0.lock().unwrap().clone();
        copied.sort();
        let mut want = ids;
        want.sort();
        assert_eq!(
            copied, want,
            "the enabled org's events, behind 450 of another's"
        );
    })
    .await;
}

#[tokio::test]
async fn the_environments_own_copy_appearing_is_not_a_replaced_table() {
    with_tw(|tw| async move {
        let table = format!("app_{}.lines", tw.slug.replace('-', "_"));
        let schema = table.split('.').next().unwrap().to_string();
        let fake = world(|w| {
            w.changed = Vec::new();
            w.table_id = 3;
            w.columns.insert(format!("{schema}.lines"), cols(&[("id", "INTEGER")]));
        });
        let app = tw
            .watcher(&format!("{}-a", tw.slug), json!({ "table": table }), ECHO_JS, json!({}))
            .await;
        sea_orm::ConnectionTrait::execute_unprepared(
            &tw.t.db,
            &format!("UPDATE app_environments SET events_enabled = true WHERE app_id = '{app}' AND name = 'staging'"),
        )
        .await
        .expect("switch on");
        // Placed, reading production's table (id 3).
        track_once(&tw.t.db, tw.org(), "staging").await.expect("place");
        track_once(&tw.t.db, tw.org(), "staging").await.expect("pass");
        assert_eq!(tw.cursor("staging").await.table_id, Some(3));
        // The environment's own copy appears: another table, another id.
        fake.0
            .lock()
            .unwrap()
            .columns
            .insert(format!("{schema}__staging.lines"), cols(&[("id", "INTEGER")]));
        track_once(&tw.t.db, tw.org(), "staging").await.expect("pass");
        let cursor = tw.cursor("staging").await;
        assert_eq!(cursor.state, "active", "{:?}", cursor.state_detail);
        assert_eq!(cursor.table_id, Some(103), "the copy's id is the new baseline");
        // A replace of the copy itself is still a break.
        fake.0.lock().unwrap().table_id = 4;
        track_once(&tw.t.db, tw.org(), "staging").await.expect("pass");
        assert_eq!(tw.cursor("staging").await.state, BROKEN);
    })
    .await;
}

#[tokio::test]
async fn more_events_of_flag_off_orgs_than_a_pass_reads_never_stall_the_mirror() {
    with_tw(|tw| async move {
        let _ = world(|_| {});
        tw.watcher(&format!("{}-a", tw.slug), on_lines(), ECHO_JS, json!({}))
            .await;
        track_once(&tw.t.db, tw.org(), "production")
            .await
            .expect("pass");
        let ids: Vec<Uuid> = tw.events(TYPE).await.into_iter().map(|e| e.id).collect();
        // Far more than one pass reads (20 pages of 200) of an org with the
        // flag off, all older than the enabled org's.
        let other = Uuid::new_v4();
        // The flag-off org owns an app, so it is listed and the flag decides.
        crate::app_events_api::run_sql(
            &tw.t.db,
            "INSERT INTO organizations (id, name, slug) VALUES ($1, 'Off', 'off-' || substr($1::text, 1, 8))",
            vec![other.into()],
        )
        .await;
        crate::app_events_api::run_sql(
            &tw.t.db,
            "INSERT INTO apps SELECT (jsonb_populate_record(NULL::apps, to_jsonb(a) || \
               jsonb_build_object('id', gen_random_uuid(), 'org_id', $1::uuid, \
                                  'slug', 'off-' || substr($1::text, 1, 8)))).* \
               FROM (SELECT * FROM apps WHERE org_id = $2 LIMIT 1) a",
            vec![other.into(), tw.org().into()],
        )
        .await;
        crate::app_events_api::run_sql(
            &tw.t.db,
            "INSERT INTO app_events (id, org_id, workspace_id, environment, source_app_id, type, \
                key, data_bytes, writer_role, root_id, depth, published_at, relayed_at) \
             SELECT gen_random_uuid(), $1, $1, 'production', NULL, 'x/y', 'k' || g, 2, 'w', \
                gen_random_uuid(), 0, now() - interval '1 day', now() - interval '1 day' \
               FROM generate_series(1, 4500) g",
            vec![other.into()],
        )
        .await;
        let log = Arc::new(RecordingLog(Mutex::new(Vec::new())));
        oxy_api_custom_apps::app_events::mirror::install_log(log.clone());
        oxy_api_custom_apps::app_events::mirror::mirror_once(&tw.t.db)
            .await
            .expect("mirror");
        let mut copied = log.0.lock().unwrap().clone();
        copied.sort();
        let mut want = ids;
        want.sort();
        assert_eq!(
            copied, want,
            "the enabled org's events, behind 4,500 of another's"
        );
    })
    .await;
}
