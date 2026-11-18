//! Retention and delivery-status sync (app events design §6.9), on the
//! database: the pass is given an explicit `now` and limited to the test's own
//! org (`sweep_org`), so a concurrent test's rows are never touched.
//!
//! Events and deliveries are stored through `fan_out`, the platform half of
//! the relay, so a production delivery is `pending` with a real task row
//! named by its `run_id`; ages are then set directly. These orgs have no OLTP
//! store, so retention has no body to delete; `app_events_relay` covers an
//! event's body living and dying with it in a real store.

use agentic_core::delegation::TaskOutcome;
use chrono::{DateTime, Duration, Utc};
use entity::{app_event_deliveries as deliveries, app_event_schemas, app_events, workspaces};
use oxy_api_custom_apps::app_events::relay::RelayedEvent;
use oxy_api_custom_apps::app_events::relay::testing::fan_out;
use oxy_api_custom_apps::app_events::retention::{
    SweepStats, retire_environment_events, sweep_once, sweep_org,
};
use oxy_api_custom_apps::app_events::schema::{live_revision, revision_for};
use oxy_api_custom_apps::custom_apps_publish::{OrgRef, PublishInput, publish};
use oxy_api_custom_apps::custom_apps_sandboxes::{TeardownReason, ops};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use oxy_custom_apps::events::{OxyEvent, event_id, qualified_type};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, QueryFilter, Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    FunctionSpec, Tenant, publish_build, seeded_tenant, throwaway_org,
};
use crate::env_schedules::publish_sandbox;
use crate::sandbox_publish::{app_row, sandbox};
use crate::staging_functions::make_guest_staff;

const EVENT: &str = "statement.uploaded";
const JS: &str = "export default async () => Response.json({});";

fn schema() -> Value {
    json!({ "type": "object", "properties": { "period": { "type": "string" } } })
}

/// An org with a workspace, a publisher declaring [`EVENT`] and a listener of
/// it, both in production.
struct Fix {
    t: Tenant,
    workspace: Uuid,
    publisher: Uuid,
    slug: String,
    _root: tempfile::TempDir,
}

impl Fix {
    /// With app events on for the org.
    async fn new() -> Self {
        Self::with_flag(true).await
    }

    async fn with_flag(on: bool) -> Self {
        let t = throwaway_org(&seeded_tenant().await).await;
        if on {
            crate::app_events_publish::enable_app_events(&t, &[t.org_id]).await;
        }
        let root = tempfile::tempdir().expect("workspace dir");
        let workspace = Uuid::new_v4();
        workspaces::ActiveModel {
            id: Set(workspace),
            name: Set("Retention".into()),
            org_id: Set(Some(t.org_id)),
            path: Set(Some(root.path().to_string_lossy().into_owned())),
            ..Default::default()
        }
        .insert(&t.db)
        .await
        .expect("seed the workspace");
        let tag = Uuid::new_v4().simple().to_string()[..8].to_string();
        let slug = format!("ret-pub-{tag}");
        let publisher = publish_app(
            &t,
            &slug,
            workspace,
            "pub-1",
            &[],
            json!({ "events": { EVENT: { "schema": schema() } } }),
        )
        .await;
        let on = qualified_type(&slug, EVENT);
        let function = FunctionSpec {
            name: "on-event",
            manifest: json!({ "on": [on] }),
            js: JS,
        };
        publish_app(
            &t,
            &format!("ret-lis-{tag}"),
            workspace,
            "lis-1",
            &[function],
            json!({}),
        )
        .await;
        Self {
            t,
            workspace,
            publisher,
            slug,
            _root: root,
        }
    }

    fn db(&self) -> &DatabaseConnection {
        &self.t.db
    }

    /// Store a production event `key` and fan it out: one `pending`
    /// delivery to the listener, with its task queued. The delivery is
    /// returned.
    async fn deliver(&self, key: &str, revision: Option<Uuid>, error: Option<&str>) -> Uuid {
        let event_type = qualified_type(&self.slug, EVENT);
        let id = event_id(self.t.org_id, "production", &event_type, key);
        let event = RelayedEvent {
            envelope: OxyEvent {
                specversion: "1.0".into(),
                id,
                dataschema: format!("{event_type}@1"),
                event_type,
                source: format!("/apps/{}", self.slug),
                time: Utc::now(),
                data: Value::Null,
                key: key.into(),
                environment: "production".into(),
                invocation: None,
                actor: None,
                causationid: None,
                rootid: id,
                depth: 0,
            },
            org_id: self.t.org_id,
            workspace_id: self.workspace,
            source_app_id: Some(self.publisher),
            // This org has no OLTP store: no body to delete.
            writer_role: "app_writer_x".into(),
            data_bytes: 21,
            schema_revision_id: revision,
            delay_seconds: 0,
            relay_error: error.map(str::to_string),
            directed: None,
            only_listener: None,
        };
        fan_out(self.db(), &[event]).await.expect("fan out");
        id
    }

    async fn sweep(&self, now: DateTime<Utc>) -> SweepStats {
        sweep_org(self.db(), now, self.t.org_id)
            .await
            .expect("sweep")
    }

    async fn delivery(&self, event: Uuid) -> deliveries::Model {
        deliveries::Entity::find()
            .filter(deliveries::Column::EventId.eq(event))
            .one(self.db())
            .await
            .expect("read the delivery")
            .expect("the event's delivery")
    }

    async fn event_exists(&self, event: Uuid) -> bool {
        app_events::Entity::find_by_id(event)
            .one(self.db())
            .await
            .expect("read the event")
            .is_some()
    }

    async fn queue_status(&self, run_id: &str) -> String {
        self.db()
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT queue_status FROM agentic_task_queue WHERE task_id = $1",
                [run_id.into()],
            ))
            .await
            .expect("read the task")
            .expect("the delivery's task")
            .try_get("", "queue_status")
            .expect("queue_status")
    }

    async fn run(&self, sql: &str, args: Vec<sea_orm::Value>) {
        self.db()
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                args,
            ))
            .await
            .expect("run the statement");
    }

    async fn age_event(&self, event: Uuid, days: i64) {
        let at = Utc::now() - Duration::days(days);
        self.run(
            "UPDATE app_events SET published_at = $1 WHERE id = $2",
            vec![at.into(), event.into()],
        )
        .await;
    }

    async fn age_delivery(&self, event: Uuid, days: i64) {
        let at = Utc::now() - Duration::days(days);
        self.run(
            "UPDATE app_event_deliveries SET created_at = $1 WHERE event_id = $2",
            vec![at.into(), event.into()],
        )
        .await;
    }

    async fn set_task(&self, run_id: &str, status: &str, error: Option<&str>) {
        self.run(
            "UPDATE agentic_task_queue SET queue_status = $1 WHERE task_id = $2",
            vec![status.into(), run_id.into()],
        )
        .await;
        if let Some(error) = error {
            self.run(
                "UPDATE agentic_runs SET error_message = $1 WHERE id = $2",
                vec![error.into(), run_id.into()],
            )
            .await;
        }
    }
}

async fn publish_app(
    t: &Tenant,
    slug: &str,
    workspace: Uuid,
    build: &str,
    functions: &[FunctionSpec],
    extra: Value,
) -> Uuid {
    publish(PublishInput {
        org_ref: Some(OrgRef::Id(t.org_id)),
        app_slug: slug.to_string(),
        project_id: workspace,
        branch: None,
        build_id: build.to_string(),
        name: None,
        promote: true,
        tarball: crate::sandbox_publish::bundle(slug, functions, extra, &[]),
        manifest: None,
        source_repo: None,
        commit_sha: None,
        published_by: Some(t.guest_id),
        published_by_email: Some(LOCAL_GUEST_EMAIL.to_string()),
        publisher: Some(oxy_server_authz::Caller::without_credential(
            t.guest_id,
            LOCAL_GUEST_EMAIL,
        )),
        machine_app_id: None,
        published_via: None,
        semantic_revision_id: None,
        confirm_schedule_removals: Vec::new(),
    })
    .await
    .unwrap_or_else(|e| panic!("publish {slug}: {e}"))
    .app_id
}

#[tokio::test]
async fn a_dead_task_marks_its_delivery_dead() {
    let f = Fix::new().await;
    let dead = f.deliver("dead", None, None).await;
    let failed = f.deliver("failed", None, None).await;
    let done = f.deliver("done", None, None).await;
    let waiting = f.deliver("waiting", None, None).await;
    let run = |d: &deliveries::Model| d.run_id.clone().expect("the delivery's run");
    f.set_task(&run(&f.delivery(dead).await), "dead", Some("retries spent"))
        .await;
    f.set_task(&run(&f.delivery(failed).await), "failed", None)
        .await;
    f.set_task(&run(&f.delivery(done).await), "completed", None)
        .await;

    let stats = f.sweep(Utc::now()).await;
    assert_eq!(
        stats,
        SweepStats {
            dead: 2,
            ..Default::default()
        }
    );

    let d = f.delivery(dead).await;
    assert_eq!(d.status, "dead");
    assert_eq!(d.last_error.as_deref(), Some("retries spent"));
    let d = f.delivery(failed).await;
    assert_eq!(d.status, "dead");
    assert_eq!(d.last_error.as_deref(), Some("task failed"));
    // A completed task is the handler's to ack; a waiting one is still live.
    assert_eq!(f.delivery(done).await.status, "pending");
    assert_eq!(f.delivery(waiting).await.status, "pending");

    // The status survives the task row going: it is the delivery's own.
    f.run(
        "DELETE FROM agentic_task_queue WHERE task_id = $1",
        vec![run(&f.delivery(dead).await).into()],
    )
    .await;
    assert_eq!(f.sweep(Utc::now()).await, SweepStats::default());
    assert_eq!(f.delivery(dead).await.status, "dead");
}

#[tokio::test]
async fn an_old_pending_delivery_expires_and_its_task_is_cancelled() {
    let f = Fix::new().await;
    let old = f.deliver("old", None, None).await;
    let young = f.deliver("young", None, None).await;
    let held = f.deliver("held", None, None).await;
    // The published event stays recent so the delivery's status can be read.
    f.age_delivery(old, 31).await;
    f.age_delivery(held, 31).await;
    f.run(
        "UPDATE app_event_deliveries SET status = 'held', run_id = NULL WHERE event_id = $1",
        vec![held.into()],
    )
    .await;
    let old_run = f.delivery(old).await.run_id.expect("run");
    let young_run = f.delivery(young).await.run_id.expect("run");
    assert_eq!(f.queue_status(&old_run).await, "queued");

    let stats = f.sweep(Utc::now()).await;
    assert_eq!(
        stats,
        SweepStats {
            expired: 2,
            ..Default::default()
        }
    );
    assert_eq!(f.delivery(old).await.status, "expired");
    assert_eq!(f.delivery(held).await.status, "expired");
    assert_eq!(f.queue_status(&old_run).await, "cancelled");
    assert_eq!(f.delivery(young).await.status, "pending");
    assert_eq!(f.queue_status(&young_run).await, "queued");
}

#[tokio::test]
async fn events_past_retention_are_deleted_with_their_deliveries() {
    let f = Fix::new().await;
    let old = f.deliver("old", None, None).await;
    let recent = f.deliver("recent", None, None).await;
    f.age_event(old, 31).await;
    f.age_event(recent, 29).await;
    // The old event's delivery is finished.
    f.run(
        "UPDATE app_event_deliveries SET status = 'acked' WHERE event_id = $1",
        vec![old.into()],
    )
    .await;
    let delivery = f.delivery(old).await.id;
    f.run(
        "INSERT INTO app_event_delivery_attempts (id, delivery_id, attempt, started_at, outcome) \
         VALUES ($1, $2, 1, now(), 'ok')",
        vec![Uuid::new_v4().into(), delivery.into()],
    )
    .await;

    let stats = f.sweep(Utc::now()).await;
    assert_eq!(
        stats,
        SweepStats {
            events_deleted: 1,
            ..Default::default()
        }
    );
    assert!(!f.event_exists(old).await);
    assert!(f.event_exists(recent).await);
    let left: Vec<_> = deliveries::Entity::find()
        .filter(deliveries::Column::Id.eq(delivery))
        .all(f.db())
        .await
        .expect("read deliveries");
    assert!(left.is_empty(), "the delivery went with its event");
    let attempts = f
        .db()
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*) AS n FROM app_event_delivery_attempts WHERE delivery_id = $1",
            [delivery.into()],
        ))
        .await
        .expect("count")
        .expect("a row")
        .try_get::<i64>("", "n")
        .expect("n");
    assert_eq!(attempts, 0, "and so did its attempts");
}

/// `retire_environment_events` is a sandbox teardown's part: asked for
/// production it refuses, before anything is touched.
#[tokio::test]
async fn retiring_production_is_refused() {
    let f = Fix::new().await;
    let id = f.deliver("k", None, None).await;
    let refused = retire_environment_events(f.db(), f.publisher, "production").await;
    let error = refused
        .expect_err("production is never retired")
        .to_string();
    assert!(error.contains("production"), "{error}");
    assert!(f.event_exists(id).await);
    assert_eq!(f.delivery(id).await.status, "pending");
}

/// R31: retention passes an org with `app_events` off by: none of its
/// deliveries is synced or expired, and none of its events deleted.
#[tokio::test]
async fn retention_skips_an_org_with_the_flag_off() {
    let f = Fix::with_flag(false).await;
    let old = f.deliver("old", None, None).await;
    f.age_event(old, 31).await;
    f.age_delivery(old, 31).await;

    assert_eq!(f.sweep(Utc::now()).await, SweepStats::default());
    assert!(f.event_exists(old).await);
    assert_eq!(f.delivery(old).await.status, "pending");
}

#[tokio::test]
async fn an_event_with_a_pending_delivery_is_kept_until_it_expires() {
    let f = Fix::new().await;
    let id = f.deliver("slow", None, None).await;
    // Relayed late: the event is old, its delivery is not.
    f.age_event(id, 31).await;

    assert_eq!(f.sweep(Utc::now()).await, SweepStats::default());
    assert!(f.event_exists(id).await, "kept while a delivery is pending");
    assert_eq!(f.delivery(id).await.status, "pending");

    // A month on, the delivery is unfinished at 30 days: it expires, and the
    // same pass then takes the event.
    let stats = f.sweep(Utc::now() + Duration::days(31)).await;
    assert_eq!(
        stats,
        SweepStats {
            expired: 1,
            events_deleted: 1,
            ..Default::default()
        }
    );
    assert!(!f.event_exists(id).await);
}

#[tokio::test]
async fn a_key_reused_after_retention_publishes_a_new_event() {
    let f = Fix::new().await;
    let id = f.deliver("statement:1", None, None).await;
    let first = f.delivery(id).await;
    f.age_event(id, 31).await;
    f.run(
        "UPDATE app_event_deliveries SET status = 'acked' WHERE event_id = $1",
        vec![id.into()],
    )
    .await;
    assert_eq!(f.sweep(Utc::now()).await.events_deleted, 1);
    assert!(!f.event_exists(id).await);

    // The same key derives the same id, and it is free again.
    let again = f.deliver("statement:1", None, None).await;
    assert_eq!(again, id);
    let events = app_events::Entity::find()
        .filter(app_events::Column::Id.eq(id))
        .all(f.db())
        .await
        .expect("read events");
    assert_eq!(events.len(), 1);
    assert!(events[0].published_at > Utc::now() - Duration::days(1));
    let second = f.delivery(id).await;
    assert_ne!(second.id, first.id, "a new delivery");
    assert_eq!(second.status, "pending");
}

#[tokio::test]
async fn unused_schema_revisions_are_deleted_and_live_ones_kept() {
    let f = Fix::new().await;
    let live = live_revision(f.db(), f.publisher, "production", EVENT)
        .await
        .expect("read the live revision")
        .expect("the declared revision");
    let other = |n: &str| json!({ "type": "object", "properties": { n: { "type": "string" } } });
    let unused = revision_for(f.db(), f.publisher, EVENT, &other("a"))
        .await
        .expect("record");
    let used = revision_for(f.db(), f.publisher, EVENT, &other("b"))
        .await
        .expect("record");
    let fresh = revision_for(f.db(), f.publisher, EVENT, &other("c"))
        .await
        .expect("record");
    // A stored event, delivered to nobody, names `used`.
    let event = f.deliver("old-shape", Some(used.id), Some("test")).await;
    // All but `fresh` were recorded a while ago.
    f.run(
        "UPDATE app_event_schemas SET created_at = now() - interval '1 day' WHERE id <> $1 AND app_id = $2",
        vec![fresh.id.into(), f.publisher.into()],
    )
    .await;
    let ids = || async {
        let mut ids: Vec<Uuid> = app_event_schemas::Entity::find()
            .filter(app_event_schemas::Column::AppId.eq(f.publisher))
            .all(f.db())
            .await
            .expect("read the revisions")
            .into_iter()
            .map(|s| s.id)
            .collect();
        ids.sort();
        ids
    };
    let sorted = |mut v: Vec<Uuid>| {
        v.sort();
        v
    };

    let stats = f.sweep(Utc::now()).await;
    assert_eq!(
        stats,
        SweepStats {
            schemas_deleted: 1,
            ..Default::default()
        }
    );
    assert_eq!(
        ids().await,
        sorted(vec![live.id, used.id, fresh.id]),
        "the unused one went; live, event-used and just-recorded stayed"
    );

    // Once the event is gone nothing retained uses `used`; `fresh` has aged.
    f.age_event(event, 31).await;
    let stats = f.sweep(Utc::now() + Duration::days(31)).await;
    assert_eq!(
        stats,
        SweepStats {
            events_deleted: 1,
            schemas_deleted: 2,
            ..Default::default()
        }
    );
    assert_eq!(ids().await, vec![live.id], "only the live revision is left");
    assert!(!ids().await.contains(&unused.id));
}

#[tokio::test]
async fn a_revision_only_an_older_build_declares_survives() {
    let f = Fix::new().await;
    let first = live_revision(f.db(), f.publisher, "production", EVENT)
        .await
        .expect("read the live revision")
        .expect("the declared revision");
    // A newer build changes the schema and goes live; build 1 is no longer
    // live but still a build a rollback can go back to.
    let changed = json!({ "type": "object", "properties": { "period": { "type": "string" }, "x": { "type": "string" } } });
    publish_app(
        &f.t,
        &f.slug,
        f.workspace,
        "pub-2",
        &[],
        json!({ "events": { EVENT: { "schema": changed } } }),
    )
    .await;
    let second = live_revision(f.db(), f.publisher, "production", EVENT)
        .await
        .expect("read the live revision")
        .expect("the declared revision");
    assert_ne!(first.id, second.id);
    // A revision no build declares, beside them.
    let orphan = revision_for(
        f.db(),
        f.publisher,
        EVENT,
        &json!({ "type": "object", "title": "orphan" }),
    )
    .await
    .expect("record");
    f.run(
        "UPDATE app_event_schemas SET created_at = now() - interval '2 days' WHERE app_id = $1",
        vec![f.publisher.into()],
    )
    .await;

    let stats = f.sweep(Utc::now()).await;
    assert_eq!(stats.schemas_deleted, 1, "{stats:?}");
    let left: Vec<Uuid> = app_event_schemas::Entity::find()
        .filter(app_event_schemas::Column::AppId.eq(f.publisher))
        .all(f.db())
        .await
        .expect("read")
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert!(left.contains(&first.id), "the older build's revision stays");
    assert!(left.contains(&second.id));
    assert!(!left.contains(&orphan.id), "no build declares the orphan");
}

#[tokio::test]
async fn a_reused_old_revision_survives_a_sweep_right_after() {
    let f = Fix::new().await;
    let schema = json!({ "type": "object", "title": "reused" });
    let row = revision_for(f.db(), f.publisher, EVENT, &schema)
        .await
        .expect("record");
    f.run(
        "UPDATE app_event_schemas SET created_at = now() - interval '2 days' WHERE id = $1",
        vec![row.id.into()],
    )
    .await;
    // A publish reuses it, and the sweep runs straight after.
    let again = revision_for(f.db(), f.publisher, EVENT, &schema)
        .await
        .expect("reuse");
    assert_eq!(again.id, row.id);
    assert_eq!(f.sweep(Utc::now()).await.schemas_deleted, 0);
    assert!(
        app_event_schemas::Entity::find_by_id(row.id)
            .one(f.db())
            .await
            .expect("read")
            .is_some()
    );
    // Left alone for a day, nothing uses it: it goes.
    let later = Utc::now() + Duration::days(1);
    assert_eq!(f.sweep(later).await.schemas_deleted, 1);
}

#[tokio::test]
async fn teardown_deletes_the_environments_events_and_deliveries() {
    crate::sandbox_teardown_task::use_scratch_homes();
    let t = seeded_tenant().await;
    let listen = FunctionSpec {
        name: "on-event",
        manifest: json!({ "on": ["other/thing.happened"] }),
        js: JS,
    };
    let tag = Uuid::new_v4().simple().to_string()[..8].to_string();
    let slug = format!("ret-down-{tag}");
    let app_id = publish_build(&t, &slug, demo_workspace_id(), "b1", true, &[listen])
        .await
        .app_id;
    let other_id = publish_build(
        &t,
        &format!("ret-other-{tag}"),
        demo_workspace_id(),
        "o1",
        true,
        &[],
    )
    .await
    .app_id;
    make_guest_staff();
    let app = app_row(&t.db, app_id).await;
    ops::create(&t.db, &app, &sandbox("r1"), &t.guest())
        .await
        .expect("create the sandbox");
    publish_sandbox(
        &t,
        &slug,
        "sbx-1",
        "r1",
        &[FunctionSpec {
            name: "on-event",
            manifest: json!({ "on": ["other/thing.happened"] }),
            js: JS,
        }],
    )
    .await;

    // Four events: the app's own in the sandbox and in production, another
    // app's in the sandbox that the app listens to, and another app's in
    // production.
    let event = |env: &str, source: Uuid, key: &str| {
        let id = Uuid::new_v4();
        (
            id,
            app_events::ActiveModel {
                id: Set(id),
                org_id: Set(t.org_id),
                workspace_id: Set(demo_workspace_id()),
                environment: Set(env.to_string()),
                source_app_id: Set(Some(source)),
                r#type: Set("x/thing.happened".to_string()),
                key: Set(key.to_string()),
                data_bytes: Set(2),
                writer_role: Set("app_writer_x".to_string()),
                schema_revision_id: Set(None),
                actor: Set(None),
                causation_id: Set(None),
                root_id: Set(id),
                depth: Set(0),
                invocation_id: Set(None),
                delay_seconds: Set(0),
                published_at: Set(Utc::now().fixed_offset()),
                relayed_at: Set(Utc::now().fixed_offset()),
                relay_error: Set(None),
                mirrored_at: Set(None),
            },
        )
    };
    let delivery = |event_id: Uuid, app: Uuid, env: &str| deliveries::ActiveModel {
        id: Set(Uuid::new_v4()),
        event_id: Set(event_id),
        app_id: Set(app),
        function_name: Set("on-event".to_string()),
        environment: Set(env.to_string()),
        status: Set("held".to_string()),
        attempts: Set(0),
        last_error: Set(None),
        run_id: Set(None),
        invocation_id: Set(None),
        created_at: Set(Utc::now().fixed_offset()),
        updated_at: Set(Utc::now().fixed_offset()),
    };
    let (own_sbx, a) = event("dev-r1", app_id, "own");
    let (own_prod, b) = event("production", app_id, "own-prod");
    let (other_sbx, c) = event("dev-r1", other_id, "other");
    let (other_prod, d) = event("production", other_id, "other-prod");
    for e in [a, b, c, d] {
        e.insert(&t.db).await.expect("seed an event");
    }
    // The app's delivery of another's sandbox event, its own sandbox event's
    // delivery to the other app, and the app's production delivery.
    for row in [
        delivery(other_sbx, app_id, "dev-r1"),
        delivery(own_sbx, other_id, "dev-r1"),
        delivery(other_prod, app_id, "production"),
        delivery(own_prod, other_id, "production"),
    ] {
        row.insert(&t.db).await.expect("seed a delivery");
    }

    let run_id = ops::begin_delete(
        &t.db,
        &app,
        &sandbox("r1"),
        Some(&t.guest()),
        TeardownReason::Deleted,
    )
    .await
    .expect("delete dev-r1");
    let outcome = crate::sandbox_teardown_task::run_queued(&t.db, &run_id).await;
    let TaskOutcome::Done { answer, .. } = outcome else {
        panic!("the teardown failed: {outcome:?}");
    };
    // Its own sandbox event, and the two deliveries in the sandbox (one to
    // the app, one of its event, which went with it).
    assert!(answer.contains("2 event rows removed"), "{answer}");

    let alive = |id: Uuid| {
        let db = t.db.clone();
        async move {
            app_events::Entity::find_by_id(id)
                .one(&db)
                .await
                .expect("read")
                .is_some()
        }
    };
    assert!(!alive(own_sbx).await, "the app's sandbox event is gone");
    assert!(alive(own_prod).await, "production's is not touched");
    assert!(alive(other_sbx).await, "another app's event is its own");
    assert!(alive(other_prod).await);
    let mut left: Vec<(Uuid, String)> = deliveries::Entity::find()
        .filter(deliveries::Column::EventId.is_in([own_sbx, own_prod, other_sbx, other_prod]))
        .all(&t.db)
        .await
        .expect("read deliveries")
        .into_iter()
        .map(|d| (d.event_id, d.environment))
        .collect();
    left.sort();
    let mut want = vec![
        (other_prod, "production".to_string()),
        (own_prod, "production".to_string()),
    ];
    want.sort();
    assert_eq!(left, want, "only production's deliveries are left");
}

/// The org's outbox mark for `environment`, if any.
async fn mark_exists(t: &Tenant, environment: &str) -> bool {
    entity::app_event_outbox_marks::Entity::find_by_id((t.org_id, environment.to_string()))
        .one(&t.db)
        .await
        .expect("read the mark")
        .is_some()
}

/// R36: a sandbox's teardown deletes the org's outbox mark for its
/// environment with its events, so a torn-down sandbox leaves nothing for
/// the sweep — unless another app of the org still has that environment:
/// marks are per org and environment, and that app's outbox may be waiting.
#[tokio::test]
async fn retiring_an_environment_deletes_its_outbox_mark() {
    crate::sandbox_teardown_task::use_scratch_homes();
    let t = seeded_tenant().await;
    let tag = Uuid::new_v4().simple().to_string()[..8].to_string();
    let a = publish_build(
        &t,
        &format!("ret-mark-a-{tag}"),
        demo_workspace_id(),
        "a1",
        true,
        &[],
    )
    .await
    .app_id;
    let b = publish_build(
        &t,
        &format!("ret-mark-b-{tag}"),
        demo_workspace_id(),
        "b1",
        true,
        &[],
    )
    .await
    .app_id;
    make_guest_staff();
    for app_id in [a, b] {
        let app = app_row(&t.db, app_id).await;
        ops::create(&t.db, &app, &sandbox("r1"), &t.guest())
            .await
            .expect("create the sandbox");
    }
    for environment in ["dev-r1", "production"] {
        oxy_api_custom_apps::app_events::marks::mark_published(&t.db, t.org_id, environment)
            .await
            .expect("mark");
    }

    retire_environment_events(&t.db, a, "dev-r1")
        .await
        .expect("retire a's sandbox");
    assert!(
        mark_exists(&t, "dev-r1").await,
        "b's dev-r1 still publishes into its outbox"
    );

    // b's sandbox goes too: its teardown has begun.
    t.db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE app_environments SET deleting_at = now() WHERE app_id = $1 AND name = 'dev-r1'",
        [b.into()],
    ))
    .await
    .expect("b's teardown begins");
    retire_environment_events(&t.db, a, "dev-r1")
        .await
        .expect("retire a's sandbox");
    assert!(
        !mark_exists(&t, "dev-r1").await,
        "the environment's mark is gone"
    );
    assert!(
        mark_exists(&t, "production").await,
        "production's is untouched"
    );
}

#[tokio::test]
async fn the_whole_fleet_pass_runs() {
    // The pass `spawn_retention` runs, over every org. Safe beside other
    // tests' rows: nothing they create is dead, a month old or unreferenced
    // for a day.
    sweep_once(&seeded_tenant().await.db, Utc::now())
        .await
        .expect("sweep every org");
}
