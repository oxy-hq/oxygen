//! The relay (app events design §6.2): an org's outbox rows become
//! `app_events` rows, one delivery per listener and an `app_function` task per
//! pending delivery, all committed before the rows leave the outbox.
//!
//! Against real stores: the org's OLTP tenant on the control plane's cluster
//! (`AppStore`, as the shape zoo provisions it), events published through the
//! definer function as the publishing app's writer role, and every app
//! published through the real `publish`, so each listener is a subscription its
//! live build registered.
//!
//! The production store forces every event's environment to `production`
//! (ruling R12), so no row from it reaches a non-production listener. The hold
//! rules outside production are driven here through [`fan_out`], the platform
//! half every store's rows reach; the branch store's own relay is
//! `app_events_branches`'.

use std::sync::Arc;

use agentic_core::delegation::{BackoffStrategy, TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::TaskExecutor;
use chrono::{DateTime, FixedOffset, Utc};
use entity::{
    app_environments, app_event_deliveries as deliveries, app_event_outbox_marks, app_events,
    workspaces,
};
use futures::FutureExt as _;
use oxy_api_custom_apps::app_events::marks;
use oxy_api_custom_apps::app_events::provenance::{Provenance, sign_hex};
use oxy_api_custom_apps::app_events::relay::testing::{fan_out, relay_rows};
use oxy_api_custom_apps::app_events::relay::{
    BATCH, RELAY_KIND, RelayExecutor, RelayPayload, RelayStats, RelayedEvent, enqueue_relay,
    listener_task_policy, relay_once, sweep_once,
};
use oxy_api_custom_apps::app_events::schema::live_revision;
use oxy_api_custom_apps::custom_apps_publish::{OrgRef, PublishInput, publish};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use oxy_custom_apps::events::{OxyEvent, event_id, jsonb_text_len, qualified_type};
use oxy_oltp::WriterRef;
use oxy_oltp::entity::tenants::{self as oltp_tenants, Entity as OltpTenants};
use oxy_oltp::events::{self as outbox, OutboxTarget, PUBLISH_SQL};
use oxy_oltp::resolver::{WriterConnection, resolve_writer_connection_for_org};
use oxy_oltp::schema::app_writer_name;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait,
    QueryFilter, Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, seeded_tenant, throwaway_org};
use crate::custom_app_functions_shape_zoo_oltp::AppStore;

// Provenance: the relay delivers only rows the host signed (R25(c)).
mod provenance;

const EVENT: &str = "statement.uploaded";
/// Declared by the publisher, listened to by nobody.
const QUIET: &str = "report.ready";
const JS: &str = "export default async () => Response.json({});";

fn schema() -> Value {
    json!({ "type": "object", "properties": { "period": { "type": "string" } } })
}

/// The publisher's `events` catalogue, [`EVENT`] at `event_schema`.
fn catalog(event_schema: Value) -> Value {
    json!({ "events": {
        EVENT: { "schema": event_schema },
        QUIET: { "schema": schema() },
    }})
}

/// An org with an OLTP store, a workspace, and a publishing app whose
/// promoted build declares [`EVENT`] and [`QUIET`].
struct Relay {
    t: Tenant,
    workspace: Uuid,
    /// The publishing app.
    publisher: Uuid,
    slug: String,
    store: AppStore,
    tag: String,
    _root: tempfile::TempDir,
}

impl Relay {
    /// With `app_events` on for the org when `flag_on`.
    async fn new(flag_on: bool) -> Self {
        let t = throwaway_org(&seeded_tenant().await).await;
        if flag_on {
            crate::app_events_publish::enable_app_events(&t, &[t.org_id]).await;
        }
        let tag = Uuid::new_v4().simple().to_string()[..8].to_string();
        let root = tempfile::tempdir().expect("workspace dir");
        let workspace = Uuid::new_v4();
        workspaces::ActiveModel {
            id: Set(workspace),
            name: Set("Relay".into()),
            org_id: Set(Some(t.org_id)),
            path: Set(Some(root.path().to_string_lossy().into_owned())),
            ..Default::default()
        }
        .insert(&t.db)
        .await
        .expect("seed the workspace");
        let slug = format!("relay-pub-{tag}");
        let store = AppStore::new(&t, &slug).await;
        store.provision(workspace).await;
        let publisher = publish_app(&t, &slug, workspace, "pub-1", &[], catalog(schema())).await;
        Self {
            t,
            workspace,
            publisher,
            slug,
            store,
            tag,
            _root: root,
        }
    }

    fn org(&self) -> Uuid {
        self.t.org_id
    }

    fn event_type(&self, name: &str) -> String {
        qualified_type(&self.slug, name)
    }

    /// Publish app `<base>-<tag>` with one function, `on-event`, listening to
    /// `types`, promoted: it listens in production and staging.
    async fn listener(&self, base: &str, types: &[String]) -> Uuid {
        let function = FunctionSpec {
            name: "on-event",
            manifest: json!({ "on": types }),
            js: JS,
        };
        let slug = format!("{base}-{}", self.tag);
        publish_app(
            &self.t,
            &slug,
            self.workspace,
            &format!("{slug}-1"),
            &[function],
            json!({}),
        )
        .await
    }

    /// Publish `name` into the org's outbox as the publisher's writer role,
    /// against the revision production's build declares (none for a name it
    /// does not declare). Returns the event id.
    async fn publish_event(&self, environment: &str, name: &str, key: &str, delay: i32) -> Uuid {
        self.publish_as(&self.store.writer, environment, name, key, delay)
            .await
    }

    /// [`Self::publish_event`] as `writer`, with the id an honest host derives.
    async fn publish_as(
        &self,
        writer: &WriterRef,
        environment: &str,
        name: &str,
        key: &str,
        delay: i32,
    ) -> Uuid {
        let id = event_id(self.org(), environment, &self.event_type(name), key);
        let revision = live_revision(&self.t.db, self.publisher, "production", name)
            .await
            .expect("read the live revision")
            .map(|r| r.id);
        let row = Row {
            id,
            environment,
            name,
            key,
            revision,
            delay,
            data: json!({ "period": "2026-11" }),
        };
        self.publish_row(writer, &row).await;
        id
    }

    /// One `oxy_events.publish` call as `writer`, exactly as `row` says, with
    /// the MAC the host signs for it: what an app's own SQL can write without
    /// the host, were it to hold the org's key. A row that does not match its
    /// MAC is [`provenance`]'s.
    async fn publish_row(&self, writer: &WriterRef, row: &Row<'_>) {
        let conn = self.publishable_writer(writer).await;
        let lineage = Lineage::top_level(row.id);
        let mac = self.host_mac(writer, &conn.role, row, &lineage);
        self.call_publish(&conn, row, &lineage, &mac).await;
    }

    /// `writer`'s connection on production's store, granted the definer.
    async fn publishable_writer(&self, writer: &WriterRef) -> WriterConnection {
        let db = &self.t.db;
        let conn = resolve_writer_connection_for_org(db, self.org(), writer)
            .await
            .expect("resolve the writer");
        outbox::ensure_publishable(db, self.org(), &OutboxTarget::Production, &conn.role)
            .await
            .expect("ensure publishable");
        conn
    }

    /// The MAC the host signs for `row`, published by `writer` (logging in as
    /// `role`) with `lineage`, at depth 0 and from no invocation.
    fn host_mac(
        &self,
        writer: &WriterRef,
        role: &str,
        row: &Row<'_>,
        lineage: &Lineage<'_>,
    ) -> String {
        let event_type = qualified_type(&writer_slug(writer), row.name);
        sign_hex(
            self.org(),
            &Provenance {
                id: row.id,
                environment: row.environment,
                event_type: &event_type,
                key: row.key,
                writer_role: role,
                actor: lineage.actor,
                root_id: lineage.root,
                causation_id: lineage.causation,
                depth: 0,
                delay_seconds: row.delay,
                data_bytes: jsonb_text_len(&row.data) as i64,
                schema_revision: row.revision,
                invocation_id: None,
            },
        )
    }

    /// One `oxy_events.publish` call on `conn`: `row` with `lineage`, at depth
    /// 0, carrying `mac` (hex) whatever it signs.
    async fn call_publish(
        &self,
        conn: &WriterConnection,
        row: &Row<'_>,
        lineage: &Lineage<'_>,
        mac: &str,
    ) {
        connect(&conn.dsn)
            .await
            .query_one(
                PUBLISH_SQL,
                &[
                    &row.id,
                    &row.environment,
                    &row.name,
                    &row.key,
                    &row.data,
                    &row.revision,
                    &lineage.actor,
                    &lineage.causation,
                    &lineage.root,
                    &0i32,
                    &None::<Uuid>,
                    &row.delay,
                    &mac,
                ],
            )
            .await
            .unwrap_or_else(|e| panic!("publish: {e:?}"));
    }

    /// `row` inserted into the outbox by the owner, as `writer`'s: a row the
    /// definer function would now refuse, as one written before it bounded
    /// its arguments can still be.
    /// Signed as the host would sign it, so the row is refused for what it
    /// holds, never for its provenance.
    async fn insert_as_owner(&self, writer: &WriterRef, row: &Row<'_>) {
        let role = resolve_writer_connection_for_org(&self.t.db, self.org(), writer)
            .await
            .expect("resolve the writer")
            .role;
        let lineage = Lineage {
            actor: None,
            ..Lineage::top_level(row.id)
        };
        let mac = self.host_mac(writer, &role, row, &lineage);
        let dsn = oxy_oltp::migrator::owner_dsn(&self.tenant().await).expect("owner dsn");
        connect(&dsn)
            .await
            .execute(
                "INSERT INTO oxy_events.outbox (id, writer_role, environment, name, key, data, \
                 schema_revision, root_id, depth, delay_seconds, provenance) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $1, 0, $8, decode($9, 'hex'))",
                &[
                    &row.id,
                    &role,
                    &row.environment,
                    &row.name,
                    &row.key,
                    &row.data,
                    &row.revision,
                    &row.delay,
                    &mac,
                ],
            )
            .await
            .unwrap_or_else(|e| panic!("insert as the owner: {e:?}"));
    }

    async fn tenant(&self) -> oltp_tenants::Model {
        OltpTenants::find()
            .filter(oltp_tenants::Column::OrgId.eq(self.org()))
            .one(&self.t.db)
            .await
            .expect("read the tenant")
            .expect("the org's tenant")
    }

    /// The outbox rows still waiting for a relay. A relayed row stays, as its
    /// body's log, with `relayed_at` set.
    async fn waiting(&self) -> i64 {
        let dsn = oxy_oltp::migrator::owner_dsn(&self.tenant().await).expect("owner dsn");
        connect(&dsn)
            .await
            .query_one(
                "SELECT count(*) FROM oxy_events.outbox WHERE relayed_at IS NULL",
                &[],
            )
            .await
            .expect("count the outbox")
            .get(0)
    }

    async fn events(&self) -> Vec<app_events::Model> {
        app_events::Entity::find()
            .filter(app_events::Column::OrgId.eq(self.org()))
            .all(&self.t.db)
            .await
            .expect("read the events")
    }

    async fn deliveries(&self) -> Vec<deliveries::Model> {
        deliveries::Entity::find()
            .all(&self.t.db)
            .await
            .expect("read the deliveries")
    }

    /// Every queued `app_function` task an event started.
    async fn event_tasks(&self) -> Vec<QueuedTask> {
        self.t
            .db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT task_id, spec->'payload' AS payload, policy, available_at \
                   FROM agentic_task_queue \
                  WHERE spec->>'kind' = 'app_function' \
                    AND spec->'payload'->>'trigger' = 'event'",
            ))
            .await
            .expect("read the queue")
            .iter()
            .map(|row| QueuedTask {
                task_id: row.try_get("", "task_id").expect("task_id"),
                payload: row.try_get("", "payload").expect("payload"),
                policy: row.try_get("", "policy").expect("policy"),
                available_at: row.try_get("", "available_at").expect("available_at"),
            })
            .collect()
    }

    /// The org's production outbox mark.
    async fn mark(&self) -> Option<app_event_outbox_marks::Model> {
        app_event_outbox_marks::Entity::find_by_id((self.org(), "production".to_string()))
            .one(&self.t.db)
            .await
            .expect("read the mark")
    }

    /// The `app_events_relay` tasks waiting in the queue.
    async fn relay_tasks(&self) -> Vec<(String, Value)> {
        self.t
            .db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                format!(
                    "SELECT task_id, spec FROM agentic_task_queue \
                      WHERE spec->>'kind' = '{RELAY_KIND}' AND queue_status = 'queued'"
                ),
            ))
            .await
            .expect("read the queue")
            .iter()
            .map(|row| {
                (
                    row.try_get("", "task_id").expect("task_id"),
                    row.try_get("", "spec").expect("spec"),
                )
            })
            .collect()
    }

    /// Turn `app`'s `environment` events switch on or off.
    async fn events_switch(&self, app: Uuid, environment: &str, on: bool) {
        app_environments::Entity::update_many()
            .col_expr(app_environments::Column::EventsEnabled, Expr::value(on))
            .filter(app_environments::Column::AppId.eq(app))
            .filter(app_environments::Column::Name.eq(environment))
            .exec(&self.t.db)
            .await
            .expect("flip the events switch");
    }

    /// One event exactly: `id`, in production, stored without error, one
    /// pending delivery for each of `listeners` whose task carries the
    /// envelope (without its body) and the delivery, and nothing waiting.
    async fn assert_delivered_once(&self, id: Uuid, listeners: &[Uuid]) {
        let events = self.events().await;
        assert_eq!(events.len(), 1, "{events:?}");
        let event = &events[0];
        assert_eq!(event.id, id);
        assert_eq!(event.environment, "production");
        assert_eq!(event.source_app_id, Some(self.publisher));
        assert_eq!(event.workspace_id, self.workspace);
        assert_eq!(event.r#type, self.event_type(EVENT));
        assert_eq!(event.relay_error, None);

        let deliveries = self.deliveries().await;
        assert_eq!(deliveries.len(), listeners.len(), "{deliveries:?}");
        let tasks = self.event_tasks().await;
        assert_eq!(tasks.len(), listeners.len(), "{tasks:?}");
        for listener in listeners {
            let delivery = deliveries
                .iter()
                .find(|d| d.app_id == *listener)
                .unwrap_or_else(|| panic!("no delivery for {listener}: {deliveries:?}"));
            assert_eq!(delivery.event_id, id);
            assert_eq!(delivery.status, "pending");
            assert_eq!(delivery.function_name, "on-event");
            assert_eq!(delivery.environment, "production");
            // R3: the delivery names the run its task started.
            let run_id = delivery.run_id.as_deref().expect("the delivery's run");
            let task = tasks
                .iter()
                .find(|t| t.task_id == run_id)
                .unwrap_or_else(|| panic!("no task {run_id}: {tasks:?}"));
            assert_eq!(task.payload["app_id"], json!(listener.to_string()));
            assert_eq!(task.payload["function_name"], json!("on-event"));
            assert_eq!(
                task.payload["event_delivery_id"],
                json!(delivery.id.to_string())
            );
            assert!(
                task.payload.get("environment").is_none(),
                "production is no environment: {:?}",
                task.payload
            );
            let input = &task.payload["input"];
            assert_eq!(input["id"], json!(id.to_string()));
            assert_eq!(input["type"], json!(self.event_type(EVENT)));
            assert_eq!(input["source"], json!(format!("/apps/{}", self.slug)));
            assert_eq!(
                input["dataschema"],
                json!(format!("{}@1", self.event_type(EVENT)))
            );
            assert_eq!(input["environment"], json!("production"));
            assert_eq!(
                input["data"],
                Value::Null,
                "the body stays in the org's store"
            );
        }
        assert_eq!(self.waiting().await, 0, "the relayed row is marked relayed");
    }

    /// A staging event from the publisher, as a branch store's relay would
    /// hand it to [`fan_out`].
    fn staging_event(&self, key: &str) -> RelayedEvent {
        let event_type = self.event_type(EVENT);
        let id = event_id(self.org(), "staging", &event_type, key);
        RelayedEvent {
            envelope: OxyEvent {
                specversion: "1.0".into(),
                id,
                dataschema: format!("{event_type}@1"),
                event_type,
                source: format!("/apps/{}", self.slug),
                time: Utc::now(),
                data: Value::Null,
                key: key.into(),
                environment: "staging".into(),
                invocation: None,
                actor: None,
                causationid: None,
                rootid: id,
                depth: 0,
            },
            org_id: self.org(),
            workspace_id: self.workspace,
            source_app_id: Some(self.publisher),
            writer_role: "app_writer_staging".into(),
            data_bytes: 21,
            schema_revision_id: None,
            delay_seconds: 0,
            relay_error: None,
            directed: None,
            only_listener: None,
        }
    }
}

/// Who one `oxy_events.publish` call says caused the event.
struct Lineage<'a> {
    actor: Option<&'a str>,
    root: Uuid,
    causation: Option<Uuid>,
}

impl Lineage<'_> {
    /// What the host writes for a publish of `id` no event's handler made,
    /// called by a person.
    fn top_level(id: Uuid) -> Self {
        Self {
            actor: Some("user:test"),
            root: id,
            causation: None,
        }
    }
}

/// The slug of the app `writer` is: its writer name with each `_` a `-`, as
/// the relay reads it back.
fn writer_slug(writer: &WriterRef) -> String {
    match writer {
        WriterRef::App(name) | WriterRef::Pipeline(name) => name.replace('_', "-"),
    }
}

/// The columns of one `oxy_events.publish` call a test chooses.
struct Row<'a> {
    id: Uuid,
    environment: &'a str,
    name: &'a str,
    key: &'a str,
    revision: Option<Uuid>,
    delay: i32,
    data: Value,
}

#[derive(Debug)]
struct QueuedTask {
    task_id: String,
    payload: Value,
    policy: Option<Value>,
    available_at: DateTime<FixedOffset>,
}

async fn connect(dsn: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(dsn, tokio_postgres::NoTls)
        .await
        .unwrap_or_else(|e| panic!("connect: {e}"));
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

/// Publish `functions` (and the manifest keys in `extra`) as build `build` of
/// app `slug` in `workspace`, promoted: the build is production's and
/// staging's.
pub(crate) async fn publish_app(
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

/// Run `body` against a fresh [`Relay`] with app events on for its org,
/// dropping the org's store (database, then roles) whatever happened.
async fn with_relay<F, Fut>(body: F)
where
    F: FnOnce(Arc<Relay>) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    with_relay_flag(true, body).await;
}

/// [`with_relay`], with app events on for the org or not.
async fn with_relay_flag<F, Fut>(flag_on: bool, body: F)
where
    F: FnOnce(Arc<Relay>) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let relay = Arc::new(Relay::new(flag_on).await);
    // Boxed: a body that publishes several apps outgrows the test thread's
    // stack when pinned on it.
    let outcome = std::panic::AssertUnwindSafe(Box::pin(body(relay.clone())))
        .catch_unwind()
        .await;
    relay.store.cleanup().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn a_relayed_event_reaches_each_listener_once() {
    with_relay(|r| async move {
        let types = [r.event_type(EVENT)];
        let a = r.listener("relay-a", &types).await;
        let b = r.listener("relay-b", &types).await;
        let id = r.publish_event("production", EVENT, "statement:1", 0).await;

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(
            stats,
            RelayStats {
                relayed: 1,
                deliveries: 2,
                rejected: 0
            }
        );
        r.assert_delivered_once(id, &[a, b]).await;
    })
    .await;
}

#[tokio::test]
async fn relaying_again_after_a_crash_is_a_no_op() {
    with_relay(|r| async move {
        let types = [r.event_type(EVENT)];
        let a = r.listener("relay-a", &types).await;
        let b = r.listener("relay-b", &types).await;
        let id = r.publish_event("production", EVENT, "statement:1", 0).await;

        // The platform transaction commits, then the relay dies before it
        // marks the row: the claim rolls back and the row stays waiting.
        let claim = outbox::claim(&r.t.db, r.org(), &OutboxTarget::Production, 500)
            .await
            .expect("claim");
        let (first, discard) =
            relay_rows(&r.t.db, r.org(), &OutboxTarget::Production, claim.rows())
                .await
                .expect("relay the claimed rows");
        assert_eq!(first.deliveries, 2);
        assert!(discard.is_empty(), "the stored event points at its row");
        claim.release().await.expect("release");
        assert_eq!(r.waiting().await, 1, "the row is still waiting");

        let again = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay again");
        assert_eq!(again, RelayStats::default(), "the event was already stored");
        r.assert_delivered_once(id, &[a, b]).await;
    })
    .await;
}

#[tokio::test]
async fn concurrent_relays_deliver_once() {
    with_relay(|r| async move {
        let types = [r.event_type(EVENT)];
        let a = r.listener("relay-a", &types).await;
        let b = r.listener("relay-b", &types).await;
        let id = r.publish_event("production", EVENT, "statement:1", 0).await;

        let (one, two) = tokio::join!(
            relay_once(&r.t.db, r.org(), "production"),
            relay_once(&r.t.db, r.org(), "production"),
        );
        let (one, two) = (one.expect("relay one"), two.expect("relay two"));
        assert_eq!(one.relayed + two.relayed, 1, "{one:?} {two:?}");
        assert_eq!(one.deliveries + two.deliveries, 2, "{one:?} {two:?}");
        r.assert_delivered_once(id, &[a, b]).await;
    })
    .await;
}

#[tokio::test]
async fn an_event_with_no_listeners_is_stored() {
    with_relay(|r| async move {
        r.listener("relay-a", &[r.event_type(EVENT)]).await;
        let id = r.publish_event("production", QUIET, "report:1", 0).await;

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(
            stats,
            RelayStats {
                relayed: 1,
                deliveries: 0,
                rejected: 0
            }
        );
        let events = r.events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, id);
        assert_eq!(events[0].relay_error, None);
        assert!(r.deliveries().await.is_empty());
        assert!(r.event_tasks().await.is_empty());
        assert_eq!(r.waiting().await, 0);
    })
    .await;
}

#[tokio::test]
async fn an_undeclared_name_is_stored_with_a_relay_error_and_not_delivered() {
    with_relay(|r| async move {
        r.listener("relay-a", &[r.event_type("not.declared")]).await;
        let id = r
            .publish_event("production", "not.declared", "k:1", 0)
            .await;

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(
            stats,
            RelayStats {
                relayed: 0,
                deliveries: 0,
                rejected: 1
            }
        );
        let events = r.events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, id);
        let error = events[0].relay_error.as_deref().expect("a relay error");
        assert!(error.contains("not.declared"), "{error}");
        assert!(r.deliveries().await.is_empty(), "nobody receives it");
        assert!(r.event_tasks().await.is_empty());
        assert_eq!(r.waiting().await, 0);
    })
    .await;
}

/// A row naming a schema revision recorded for no version of the event is
/// not delivered.
#[tokio::test]
async fn an_unknown_revision_is_a_relay_error() {
    with_relay(|r| async move {
        r.listener("relay-a", &[r.event_type(EVENT)]).await;
        let row = Row {
            id: event_id(r.org(), "production", &r.event_type(EVENT), "k:1"),
            environment: "production",
            name: EVENT,
            key: "k:1",
            revision: Some(Uuid::new_v4()),
            delay: 0,
            data: json!({ "period": "2026-11" }),
        };
        r.publish_row(&r.store.writer, &row).await;

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(stats.rejected, 1, "{stats:?}");
        let events = r.events().await;
        let error = events[0].relay_error.as_deref().expect("a relay error");
        assert!(error.contains("schema revision"), "{error}");
        assert!(r.deliveries().await.is_empty());
        assert!(r.event_tasks().await.is_empty());
    })
    .await;
}

/// An event published against revision 1 just before a compatible revision 2
/// went live is still delivered, described by the revision it was published
/// against.
#[tokio::test]
async fn an_event_at_the_previous_revision_delivers_after_a_newer_one_goes_live() {
    with_relay(|r| async move {
        let a = r.listener("relay-a", &[r.event_type(EVENT)]).await;
        let id = r.publish_event("production", EVENT, "statement:1", 0).await;
        // An added optional property: compatible, so the promote is allowed.
        let mut newer = schema();
        newer["properties"]["note"] = json!({ "type": "string" });
        publish_app(&r.t, &r.slug, r.workspace, "pub-2", &[], catalog(newer)).await;
        let live = live_revision(&r.t.db, r.publisher, "production", EVENT)
            .await
            .expect("read the live revision")
            .expect("production declares it");
        assert_eq!(live.revision, 2, "revision 2 is live");

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(stats.relayed, 1, "{stats:?}");
        let events = r.events().await;
        assert_eq!(events[0].relay_error, None);
        let tasks = r.event_tasks().await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].payload["app_id"], json!(a.to_string()));
        assert_eq!(tasks[0].payload["input"]["id"], json!(id.to_string()));
        assert_eq!(
            tasks[0].payload["input"]["dataschema"],
            json!(format!("{}@1", r.event_type(EVENT)))
        );
    })
    .await;
}

/// The id is the relay's to derive (§6.1), not the row's. A row carrying
/// another event's id — here, the id of an event the app has not published
/// yet — is stored under the id its own type and key derive, with a relay
/// error, and the real event, published later, is still delivered.
#[tokio::test]
async fn a_forged_id_is_stored_under_the_derived_id_and_the_real_event_still_delivers() {
    with_relay(|r| async move {
        let a = r.listener("relay-a", &[r.event_type(EVENT)]).await;
        let real = event_id(r.org(), "production", &r.event_type(EVENT), "statement:1");
        let revision = live_revision(&r.t.db, r.publisher, "production", EVENT)
            .await
            .expect("read the live revision")
            .map(|rev| rev.id);
        let forged = Row {
            id: real,
            environment: "production",
            name: EVENT,
            key: "k:squat",
            revision,
            delay: 0,
            data: json!({ "period": "2026-11" }),
        };
        r.publish_row(&r.store.writer, &forged).await;

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(
            stats,
            RelayStats {
                relayed: 0,
                deliveries: 0,
                rejected: 1
            }
        );
        let events = r.events().await;
        assert_eq!(events.len(), 1);
        let derived = event_id(r.org(), "production", &r.event_type(EVENT), "k:squat");
        assert_eq!(events[0].id, derived, "stored under the id it derives");
        let error = events[0].relay_error.as_deref().expect("a relay error");
        assert!(error.contains(&real.to_string()), "{error}");
        assert!(r.deliveries().await.is_empty());

        let honest = r.publish_event("production", EVENT, "statement:1", 0).await;
        assert_eq!(honest, real);
        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(stats.relayed, 1, "the real event was not taken: {stats:?}");
        let deliveries = r.deliveries().await;
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].event_id, real);
        assert_eq!(deliveries[0].app_id, a);
        assert_eq!(deliveries[0].status, "pending");
    })
    .await;
}

/// Another app's writer inserting the id of this app's next event does not
/// stop that event: the outbox is keyed on `(writer_role, id)`, so both rows
/// sit in it, and the relay stores the honest one under its derived id and
/// delivers it, and the squatter's under its own derived id with a relay
/// error.
#[tokio::test]
async fn a_squatted_id_does_not_stop_the_real_event() {
    with_relay(|r| async move {
        let a = r.listener("relay-a", &[r.event_type(EVENT)]).await;
        let squatter_slug = format!("relay-sq-{}", r.tag);
        let squatter_app =
            publish_app(&r.t, &squatter_slug, r.workspace, "sq-1", &[], json!({})).await;
        let squatter = WriterRef::app(app_writer_name(&squatter_slug).expect("a writer name"))
            .expect("a valid writer");
        r.store.ensure_extra_writer(&squatter, r.workspace).await;

        let real = event_id(r.org(), "production", &r.event_type(EVENT), "statement:1");
        let revision = live_revision(&r.t.db, r.publisher, "production", EVENT)
            .await
            .expect("read the live revision")
            .map(|rev| rev.id);
        let squat = Row {
            id: real,
            environment: "production",
            name: EVENT,
            key: "statement:1",
            revision,
            delay: 0,
            data: json!({ "period": "2026-11" }),
        };
        r.publish_row(&squatter, &squat).await;
        let honest = r.publish_event("production", EVENT, "statement:1", 0).await;
        assert_eq!(honest, real);
        assert_eq!(r.waiting().await, 2, "both rows landed");

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(
            stats,
            RelayStats {
                relayed: 1,
                deliveries: 1,
                rejected: 1
            }
        );
        let events = r.events().await;
        assert_eq!(events.len(), 2, "{events:?}");
        let stored = events
            .iter()
            .find(|e| e.id == real)
            .expect("the real event");
        assert_eq!(stored.source_app_id, Some(r.publisher));
        assert_eq!(stored.relay_error, None);
        let squatted = events
            .iter()
            .find(|e| e.source_app_id == Some(squatter_app))
            .expect("the squatter's row");
        assert_eq!(
            squatted.id,
            event_id(
                r.org(),
                "production",
                &qualified_type(&squatter_slug, EVENT),
                "statement:1"
            ),
            "stored under the id its own app derives"
        );
        let error = squatted.relay_error.as_deref().expect("a relay error");
        assert!(error.contains(&real.to_string()), "{error}");
        let deliveries = r.deliveries().await;
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].event_id, real);
        assert_eq!(deliveries[0].app_id, a);
        assert_eq!(r.waiting().await, 0);
    })
    .await;
}

/// A payload over the platform's limit, measured in the store as `jsonb`
/// text, is stored with a relay error and its size — never its body — reaches
/// nobody, and leaves the claim set with its batch: one just over the limit
/// and one far over it. The definer function refuses both (R32: its ceiling
/// is this limit, by this measure), so each is written by the owner here, as
/// a row from before the function bounded its arguments; the relay's check is
/// the backstop that keeps such a row from stalling the outbox.
#[tokio::test]
async fn an_oversized_payload_is_refused_and_does_not_block_the_batch() {
    with_relay(|r| async move {
        let a = r.listener("relay-a", &[r.event_type(EVENT)]).await;
        let revision = live_revision(&r.t.db, r.publisher, "production", EVENT)
            .await
            .expect("read the live revision")
            .map(|rev| rev.id);
        let over = json!({ "blob": "x".repeat(70_000) });
        let huge = json!({ "blob": "x".repeat(300_000) });
        // As `jsonb` text, with the space Postgres writes after the colon.
        let text_bytes = |n: usize| n + r#"{"blob": ""}"#.len();
        for (key, data) in [("big:1", over), ("huge:1", huge)] {
            let row = Row {
                id: event_id(r.org(), "production", &r.event_type(EVENT), key),
                environment: "production",
                name: EVENT,
                key,
                revision,
                delay: 0,
                data,
            };
            r.insert_as_owner(&r.store.writer, &row).await;
        }
        let normal = r.publish_event("production", EVENT, "statement:1", 0).await;

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(
            stats,
            RelayStats {
                relayed: 1,
                deliveries: 1,
                rejected: 2
            }
        );
        let events = r.events().await;
        assert_eq!(events.len(), 3, "{events:?}");
        for (key, bytes) in [
            ("big:1", text_bytes(70_000)),
            ("huge:1", text_bytes(300_000)),
        ] {
            let id = event_id(r.org(), "production", &r.event_type(EVENT), key);
            let stored = events.iter().find(|e| e.id == id).expect("stored");
            assert_eq!(stored.data_bytes as usize, bytes, "its size, not its body");
            let error = stored.relay_error.as_deref().expect("a relay error");
            assert!(error.contains(&bytes.to_string()), "{error}");
        }
        let deliveries = r.deliveries().await;
        assert_eq!(deliveries.len(), 1, "only the normal event is delivered");
        assert_eq!(deliveries[0].event_id, normal);
        assert_eq!(deliveries[0].app_id, a);
        assert_eq!(
            r.waiting().await,
            0,
            "the oversized rows left the claim set too"
        );
    })
    .await;
}

/// R12: production's store holds production's events. A row in it claiming
/// another environment is stored as production's, with the claim refused, and
/// reaches no listener — staging's included, with its switch on.
#[tokio::test]
async fn a_production_store_row_claiming_staging_is_rejected() {
    with_relay(|r| async move {
        let a = r.listener("relay-a", &[r.event_type(EVENT)]).await;
        r.events_switch(a, "staging", true).await;
        let claimed = r.publish_event("staging", EVENT, "statement:1", 0).await;

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(
            stats,
            RelayStats {
                relayed: 0,
                deliveries: 0,
                rejected: 1
            }
        );
        let events = r.events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].environment, "production",
            "the store decides, not the row"
        );
        assert_eq!(
            events[0].id,
            event_id(r.org(), "production", &r.event_type(EVENT), "statement:1"),
            "and the id follows the environment the store decided"
        );
        let error = events[0].relay_error.as_deref().expect("a relay error");
        assert!(error.contains("staging"), "{error}");
        assert!(r.deliveries().await.is_empty());
        assert!(r.event_tasks().await.is_empty());
        assert_eq!(r.waiting().await, 0);
        assert_eq!(
            bodies(&r, claimed).await,
            0,
            "stored under another id, so its row is no event's body"
        );
    })
    .await;
}

/// A writer role no app of the org answers to has no app to store the event
/// under: the row is dropped (and logged), so it cannot hold up the outbox.
#[tokio::test]
async fn a_row_from_a_writer_with_no_app_is_dropped() {
    with_relay(|r| async move {
        let orphan =
            WriterRef::app(format!("relay_orphan_{}", r.tag)).expect("a valid writer name");
        r.store.ensure_extra_writer(&orphan, r.workspace).await;
        let id = r.publish_as(&orphan, "production", EVENT, "k:1", 0).await;

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(
            stats,
            RelayStats {
                relayed: 0,
                deliveries: 0,
                rejected: 1
            }
        );
        assert!(r.events().await.is_empty());
        assert_eq!(r.waiting().await, 0);
        assert_eq!(
            bodies(&r, id).await,
            0,
            "no event points at it: deleted, not kept"
        );
    })
    .await;
}

#[tokio::test]
async fn staging_with_the_switch_off_holds_the_delivery() {
    with_relay(|r| async move {
        let a = r.listener("relay-a", &[r.event_type(EVENT)]).await;

        let stats = fan_out(&r.t.db, &[r.staging_event("statement:1")])
            .await
            .expect("fan out");
        assert_eq!(stats.deliveries, 1);
        let held = r.deliveries().await;
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].app_id, a);
        assert_eq!(held[0].environment, "staging");
        assert_eq!(held[0].status, "held");
        assert_eq!(held[0].run_id, None);
        assert!(
            r.event_tasks().await.is_empty(),
            "a held delivery queues nothing"
        );

        // With the switch on, the next event is delivered in staging.
        r.events_switch(a, "staging", true).await;
        fan_out(&r.t.db, &[r.staging_event("statement:2")])
            .await
            .expect("fan out");
        let tasks = r.event_tasks().await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].payload["environment"], json!("staging"));
        assert_eq!(tasks[0].payload["input"]["environment"], json!("staging"));
    })
    .await;
}

#[tokio::test]
async fn a_listener_app_without_that_environment_is_held() {
    with_relay(|r| async move {
        let types = [r.event_type(EVENT)];
        let a = r.listener("relay-a", &types).await;
        let b = r.listener("relay-b", &types).await;
        r.events_switch(a, "staging", true).await;
        r.events_switch(b, "staging", true).await;
        // `b`'s subscription outlives its staging environment.
        app_environments::Entity::delete_by_id((b, "staging".to_string()))
            .exec(&r.t.db)
            .await
            .expect("drop b's staging");

        fan_out(&r.t.db, &[r.staging_event("statement:1")])
            .await
            .expect("fan out");
        let deliveries = r.deliveries().await;
        let status = |app: Uuid| {
            deliveries
                .iter()
                .find(|d| d.app_id == app)
                .map(|d| d.status.clone())
        };
        assert_eq!(status(a).as_deref(), Some("pending"));
        assert_eq!(status(b).as_deref(), Some("held"));
        let tasks = r.event_tasks().await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].payload["app_id"], json!(a.to_string()));
    })
    .await;
}

#[tokio::test]
async fn a_delayed_event_waits() {
    with_relay(|r| async move {
        r.listener("relay-a", &[r.event_type(EVENT)]).await;
        r.publish_event("production", EVENT, "statement:1", 600)
            .await;

        relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        let tasks = r.event_tasks().await;
        assert_eq!(tasks.len(), 1);
        let earliest = Utc::now() + chrono::Duration::seconds(590);
        assert!(
            tasks[0].available_at >= earliest,
            "{} is before {earliest}",
            tasks[0].available_at
        );
        let policy = tasks[0].policy.as_ref().expect("a listener's policy");
        assert_eq!(policy["retry"]["max_retries"], json!(4));
    })
    .await;
}

#[test]
fn listener_task_policy_defaults() {
    let policy = listener_task_policy(&json!({ "on": "a/b" }));
    let retry = policy.retry.expect("a listener retries");
    assert_eq!(retry.max_retries, 4, "five attempts");
    match retry.backoff {
        BackoffStrategy::Exponential {
            initial_delay_ms,
            max_delay_ms,
        } => {
            assert_eq!(initial_delay_ms, 10_000);
            assert_eq!(max_delay_ms, 600_000);
        }
        other => panic!("{other:?}"),
    }

    // The declared `retries` win, including a single attempt.
    let declared = listener_task_policy(&json!({
        "on": "a/b",
        "retries": { "maxAttempts": 3, "minTimeoutMs": 500, "maxTimeoutMs": 2_000 },
    }));
    let retry = declared.retry.expect("declared retries");
    assert_eq!(retry.max_retries, 2);
    let once = listener_task_policy(&json!({ "on": "a/b", "retries": { "maxAttempts": 1 } }));
    assert!(once.retry.is_none(), "{once:?}");
}

/// The nudge queues one relay per org and environment, and the queued task
/// runs through the executor the fleet registers.
#[tokio::test]
async fn a_queued_relay_runs_once_per_org_and_environment() {
    with_relay(|r| async move {
        r.listener("relay-a", &[r.event_type(EVENT)]).await;
        r.publish_event("production", EVENT, "statement:1", 0).await;

        enqueue_relay(&r.t.db, r.org(), "production")
            .await
            .expect("queue");
        enqueue_relay(&r.t.db, r.org(), "production")
            .await
            .expect("queue again");
        let queued = r.relay_tasks().await;
        assert_eq!(queued.len(), 1, "one waiting relay is enough: {queued:?}");
        let (task_id, spec) = &queued[0];
        let payload: RelayPayload =
            serde_json::from_value(spec["payload"].clone()).expect("a relay payload");
        assert_eq!(payload.org_id, r.org());
        assert_eq!(payload.environment, "production");

        let executor = RelayExecutor { db: r.t.db.clone() };
        let mut running = executor
            .execute(TaskAssignment {
                task_id: task_id.clone(),
                parent_task_id: None,
                run_id: task_id.clone(),
                spec: serde_json::from_value(spec.clone()).expect("a task spec"),
                policy: None,
                claim_epoch: Default::default(),
            })
            .await
            .expect("the executor starts");
        let outcome = running.outcomes.recv().await.expect("an outcome");
        assert!(matches!(outcome, TaskOutcome::Done { .. }), "{outcome:?}");
        assert_eq!(r.events().await.len(), 1);
        assert_eq!(r.event_tasks().await.len(), 1);
        assert_eq!(r.waiting().await, 0);
    })
    .await;
}

/// R27: the sweep queues a relay for a production mark published since it
/// was last drained, once the nudge has had a minute; never for a fresh one.
#[tokio::test]
async fn the_sweep_queues_a_relay_for_a_due_mark() {
    with_relay(|r| async move {
        r.publish_event("production", EVENT, "statement:1", 0).await;
        marks::mark_published(&r.t.db, r.org(), "production")
            .await
            .expect("mark");
        assert_eq!(
            sweep_once(&r.t.db).await.expect("sweep").queued,
            0,
            "the nudge's minute"
        );
        assert!(r.relay_tasks().await.is_empty());

        set_mark(&r, Some(120), None).await;
        assert_eq!(sweep_once(&r.t.db).await.expect("sweep").queued, 1);
        let queued = r.relay_tasks().await;
        assert_eq!(queued.len(), 1, "{queued:?}");
        assert_eq!(queued[0].1["payload"]["org_id"], json!(r.org().to_string()));
        assert_eq!(queued[0].1["payload"]["environment"], json!("production"));
    })
    .await;
}

/// R31: with `app_events` off for the org, a pass relays nothing and never
/// connects to the org's store: the row waits, the store's tenant row is
/// untouched, and the pass succeeds.
#[tokio::test]
async fn a_flag_off_orgs_relay_does_nothing() {
    with_relay_flag(false, |r| async move {
        r.listener("relay-a", &[r.event_type(EVENT)]).await;
        r.publish_event("production", EVENT, "statement:1", 0).await;
        one_version_behind(&r).await;
        let before = r.tenant().await.platform_schema_version;

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("a flag-off pass succeeds");
        assert_eq!(stats, RelayStats::default());
        assert!(r.events().await.is_empty());
        assert!(r.event_tasks().await.is_empty());
        assert_eq!(
            r.tenant().await.platform_schema_version,
            before,
            "no connection"
        );
        assert_eq!(r.waiting().await, 1, "the row waits for the flag");
    })
    .await;
}

/// R28 reversed (Task 17): every environment's store is relayed. The sweep
/// queues a settled staging mark as it does a production one, and passes a
/// mark for a name that is no environment by. This org has no staging branch,
/// so nothing was ever published to one: a staging or sandbox pass finds no
/// store, succeeds (no failed system run) and drains its mark, which the
/// sweep then leaves alone. A name the platform has no store for is a quiet
/// no-op.
#[tokio::test]
async fn without_a_staging_branch_a_branch_pass_drains_quietly() {
    with_relay(|r| async move {
        for environment in ["staging", "nowhere"] {
            r.t.db
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "INSERT INTO app_event_outbox_marks (org_id, environment, published_at) \
                     VALUES ($1, $2, now() - interval '10 minutes')",
                    [r.org().into(), environment.into()],
                ))
                .await
                .expect("a due mark");
        }
        assert_eq!(sweep_once(&r.t.db).await.expect("sweep").queued, 1);
        let queued = r.relay_tasks().await;
        assert_eq!(queued.len(), 1, "{queued:?}");
        assert_eq!(queued[0].1["payload"]["environment"], json!("staging"));

        for environment in ["dev-a1b2c3", "nowhere"] {
            let stats = relay_once(&r.t.db, r.org(), environment)
                .await
                .unwrap_or_else(|e| panic!("{environment}: {e}"));
            assert_eq!(stats, RelayStats::default(), "{environment}");
        }

        let executor = RelayExecutor { db: r.t.db.clone() };
        let payload = RelayPayload {
            org_id: r.org(),
            environment: "staging".into(),
        };
        let mut running = executor
            .execute(TaskAssignment {
                task_id: "t".into(),
                parent_task_id: None,
                run_id: "t".into(),
                spec: TaskSpec::Custom {
                    kind: RELAY_KIND.to_string(),
                    payload: serde_json::to_value(&payload).expect("a relay payload"),
                },
                policy: None,
                claim_epoch: Default::default(),
            })
            .await
            .expect("the executor starts");
        let outcome = running.outcomes.recv().await.expect("an outcome");
        assert!(matches!(outcome, TaskOutcome::Done { .. }), "{outcome:?}");

        let mark = app_event_outbox_marks::Entity::find_by_id((r.org(), "staging".to_string()))
            .one(&r.t.db)
            .await
            .expect("read the mark")
            .expect("the staging mark");
        assert!(mark.drained_at.is_some(), "drained: {mark:?}");
        assert_eq!(mark.pending_since, None, "{mark:?}");
        r.t.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "DELETE FROM agentic_task_queue WHERE task_id LIKE $1",
                [format!("{RELAY_KIND}:{}:%", r.org()).into()],
            ))
            .await
            .expect("clear this org's queued relay");
        assert_eq!(
            sweep_once(&r.t.db).await.expect("sweep").queued,
            0,
            "a drained mark is not queued again"
        );
    })
    .await;
}

/// `r`'s production mark, published and drained `seconds` ago (`None`: no
/// drain), created if missing.
async fn set_mark(r: &Relay, published: Option<i64>, drained: Option<i64>) {
    let ago = |s: Option<i64>| s.map(|s| Utc::now() - chrono::Duration::seconds(s));
    r.t.db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO app_event_outbox_marks (org_id, environment, published_at, drained_at) \
             VALUES ($1, 'production', $2, $3) \
             ON CONFLICT (org_id, environment) \
             DO UPDATE SET published_at = $2, drained_at = $3",
            [
                r.org().into(),
                ago(published).unwrap_or_else(Utc::now).into(),
                ago(drained).into(),
            ],
        ))
        .await
        .expect("set the mark");
}

/// The org's store recorded one platform version behind, so anything that
/// opens it the way the relay does (`current_tenant`) brings it current and
/// says so in the tenant row.
async fn one_version_behind(r: &Relay) {
    r.t.db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE oltp_tenants SET platform_schema_version = platform_schema_version - 1 \
              WHERE org_id = $1",
            [r.org().into()],
        ))
        .await
        .expect("set the store one version behind");
}

/// R27: a sweep over `r` opened no connection to its store — the tenant row is
/// still one version behind, which any owner connection the relay makes
/// would have brought current — and queued nothing, though a row waits.
async fn assert_swept_without_touching_the_store(r: &Relay, why: &str) {
    let before = r.tenant().await.platform_schema_version;
    assert_eq!(sweep_once(&r.t.db).await.expect("sweep").queued, 0, "{why}");
    assert!(r.relay_tasks().await.is_empty(), "{why}: nothing queued");
    assert_eq!(
        r.tenant().await.platform_schema_version,
        before,
        "{why}: the sweep connected to the org's store"
    );
    assert_eq!(r.waiting().await, 1, "{why}: the row still waits");
}

#[tokio::test]
async fn the_sweep_never_connects_to_a_flag_off_orgs_store() {
    with_relay_flag(false, |r| async move {
        r.publish_event("production", EVENT, "statement:1", 0).await;
        set_mark(&r, Some(600), None).await;
        one_version_behind(&r).await;
        assert_swept_without_touching_the_store(&r, "app_events is off for the org").await;
    })
    .await;
}

#[tokio::test]
async fn the_sweep_never_connects_to_a_store_with_no_mark() {
    with_relay(|r| async move {
        // Written by the app's own SQL, which marks nothing.
        r.publish_event("production", EVENT, "statement:1", 0).await;
        one_version_behind(&r).await;
        assert_swept_without_touching_the_store(&r, "no mark").await;
    })
    .await;
}

#[tokio::test]
async fn the_sweep_never_connects_to_a_store_whose_mark_is_drained() {
    with_relay(|r| async move {
        r.publish_event("production", EVENT, "statement:1", 0).await;
        set_mark(&r, Some(600), Some(300)).await;
        one_version_behind(&r).await;
        assert_swept_without_touching_the_store(&r, "the mark is drained").await;
    })
    .await;
}

/// R27: a pass that claims less than a full batch marks the outbox drained
/// as of when it began claiming, so the sweep leaves it alone; one that
/// claims a full batch leaves the mark due.
#[tokio::test]
async fn a_pass_that_empties_the_outbox_marks_it_drained() {
    with_relay(|r| async move {
        r.publish_event("production", EVENT, "statement:1", 0).await;
        marks::mark_published(&r.t.db, r.org(), "production")
            .await
            .expect("mark");
        relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        let mark = r.mark().await.expect("the mark");
        let drained = mark.drained_at.expect("drained");
        assert!(drained > mark.published_at, "{mark:?}");
    })
    .await;
}

/// Set `column` of `r`'s production mark to `seconds` ago, returning it.
async fn backdate_mark(r: &Relay, column: &str, seconds: i64) -> DateTime<FixedOffset> {
    let at = (Utc::now() - chrono::Duration::seconds(seconds)).fixed_offset();
    r.t.db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!(
                "UPDATE app_event_outbox_marks SET {column} = $2 \
                  WHERE org_id = $1 AND environment = 'production'"
            ),
            [r.org().into(), at.into()],
        ))
        .await
        .unwrap_or_else(|e| panic!("backdate {column}: {e}"));
    // As the column holds it: microseconds.
    let mark = r.mark().await.expect("the mark");
    match column {
        "published_at" => mark.published_at,
        "pending_since" => mark.pending_since.expect("pending_since"),
        _ => mark.drained_at.expect("drained_at"),
    }
}

/// From now on every relay pass of this (per-test) platform database fails
/// once it has claimed: storing an event is refused.
async fn fail_every_relay(r: &Relay) {
    r.t.db
        .execute_unprepared(
            "CREATE FUNCTION refuse_app_events() RETURNS trigger LANGUAGE plpgsql AS \
             $$ BEGIN RAISE EXCEPTION 'the platform refuses events (test)'; END $$; \
             CREATE TRIGGER refuse_app_events BEFORE INSERT ON app_events \
             FOR EACH ROW EXECUTE FUNCTION refuse_app_events()",
        )
        .await
        .expect("make every relay fail");
}

/// R34: the stale alarm measures how long work has waited (`pending_since`),
/// so a relay that fails on every pass alarms once that is past five minutes,
/// though the sweep keeps queueing it. A failed pass reopens the mark by
/// stamping `published_at` now (so the sweep queues it again after the
/// nudge's minute) and leaves `pending_since`, so the restamp hides nothing.
#[tokio::test]
async fn a_relay_that_keeps_failing_alarms_once_its_work_has_waited_five_minutes() {
    with_relay(|r| async move {
        r.publish_event("production", EVENT, "statement:1", 0).await;
        marks::mark_published(&r.t.db, r.org(), "production")
            .await
            .expect("mark");
        assert!(
            r.mark().await.expect("the mark").pending_since.is_some(),
            "a new mark is pending"
        );
        fail_every_relay(&r).await;
        let published = backdate_mark(&r, "published_at", 120).await;
        let pending = backdate_mark(&r, "pending_since", 120).await;

        let swept = sweep_once(&r.t.db).await.expect("sweep");
        assert_eq!((swept.queued, swept.stale.len()), (1, 0), "{swept:?}");
        relay_once(&r.t.db, r.org(), "production")
            .await
            .expect_err("the pass fails");
        let mark = r.mark().await.expect("the mark");
        assert!(mark.published_at > published, "reopened, now: {mark:?}");
        assert_eq!(
            mark.pending_since,
            Some(pending),
            "still waiting since then"
        );
        let swept = sweep_once(&r.t.db).await.expect("sweep");
        assert_eq!(
            (swept.queued, swept.stale.len()),
            (0, 0),
            "the reopen has the nudge's minute: {swept:?}"
        );
        backdate_mark(&r, "published_at", 120).await;
        let swept = sweep_once(&r.t.db).await.expect("sweep");
        assert_eq!(
            (swept.queued, swept.stale.len()),
            (1, 0),
            "queued again, not yet stale: {swept:?}"
        );

        let pending = backdate_mark(&r, "pending_since", 6 * 60).await;
        relay_once(&r.t.db, r.org(), "production")
            .await
            .expect_err("the pass fails again");
        assert_eq!(
            r.mark().await.expect("the mark").pending_since,
            Some(pending),
            "a failure does not restart the wait"
        );
        let swept = sweep_once(&r.t.db).await.expect("sweep");
        assert_eq!(
            swept.stale,
            [r.org()],
            "the alarm, though the failure stamped the mark just now"
        );
        backdate_mark(&r, "published_at", 120).await;
        let swept = sweep_once(&r.t.db).await.expect("sweep");
        assert_eq!(swept.queued, 1, "and queued again: {swept:?}");
        assert_eq!(swept.stale, [r.org()], "{swept:?}");
        assert_eq!(r.waiting().await, 1, "the row still waits");
    })
    .await;
}

/// R34: publishes into an outbox whose work is waiting keep `published_at`
/// fresh and leave `pending_since` alone, so continuous publishing does not
/// hide a stuck relay: the alarm fires though no mark is past the nudge's
/// minute (which only decides queueing).
#[tokio::test]
async fn continuous_publishes_keep_the_time_work_began_waiting() {
    with_relay(|r| async move {
        marks::mark_published(&r.t.db, r.org(), "production")
            .await
            .expect("mark");
        assert!(r.mark().await.expect("the mark").pending_since.is_some());
        backdate_mark(&r, "published_at", 10 * 60).await;
        let pending = backdate_mark(&r, "pending_since", 10 * 60).await;
        for _ in 0..3 {
            marks::mark_published(&r.t.db, r.org(), "production")
                .await
                .expect("mark again");
        }
        let mark = r.mark().await.expect("the mark");
        assert_eq!(mark.pending_since, Some(pending), "{mark:?}");
        assert!(mark.published_at > pending, "{mark:?}");

        let swept = sweep_once(&r.t.db).await.expect("sweep");
        assert_eq!(swept.stale, [r.org()], "{swept:?}");
        assert_eq!(swept.queued, 0, "the publish's own nudge has its minute");
    })
    .await;
}

/// R34: a pass that drains the outbox clears `pending_since`; a publish after
/// it starts a new wait, and so does a failed pass that reopens a drained
/// mark.
#[tokio::test]
async fn a_drain_clears_the_time_work_began_waiting() {
    with_relay(|r| async move {
        r.publish_event("production", EVENT, "statement:1", 0).await;
        marks::mark_published(&r.t.db, r.org(), "production")
            .await
            .expect("mark");
        assert!(r.mark().await.expect("the mark").pending_since.is_some());
        relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        let drained = r.mark().await.expect("the mark");
        assert!(
            drained.drained_at >= Some(drained.published_at),
            "{drained:?}"
        );
        assert_eq!(drained.pending_since, None, "drained: nothing waits");

        marks::mark_published(&r.t.db, r.org(), "production")
            .await
            .expect("mark");
        let again = r.mark().await.expect("the mark");
        let since = again
            .pending_since
            .expect("a publish after the drain waits");
        assert!(Some(since) > drained.drained_at, "{again:?}");

        // Drained again, then a pass that fails on a row nothing marked.
        relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(r.mark().await.expect("the mark").pending_since, None);
        r.publish_event("production", EVENT, "statement:2", 0).await;
        fail_every_relay(&r).await;
        relay_once(&r.t.db, r.org(), "production")
            .await
            .expect_err("the pass fails");
        let reopened = r.mark().await.expect("the mark");
        assert!(
            Some(reopened.published_at) > reopened.drained_at,
            "due again: {reopened:?}"
        );
        assert!(reopened.published_at > again.published_at, "{reopened:?}");
        assert!(reopened.pending_since.is_some(), "{reopened:?}");
    })
    .await;
}

/// Follow-up to R34: a pass that skipped a failing pass's locked rows, and
/// began before that pass failed, may finish after the failure's reopen. The
/// reopen stamps `published_at` now, so that pass drains only to its own
/// start and the mark stays due: the failed pass's rows are relayed by the
/// sweep's next pass, not left for the org's next publish. A reopen while
/// the mark is already due leaves `pending_since` alone.
#[tokio::test]
async fn a_pass_that_began_before_a_failure_cannot_drain_its_reopen() {
    with_relay(|r| async move {
        r.publish_event("production", EVENT, "statement:1", 0).await;
        marks::mark_published(&r.t.db, r.org(), "production")
            .await
            .expect("mark");
        backdate_mark(&r, "published_at", 120).await;
        let pending = backdate_mark(&r, "pending_since", 120).await;

        // B begins claiming, and skips the rows A holds.
        let b_started = marks::platform_now(&r.t.db).await.expect("now");
        // A fails and reopens the mark.
        fail_every_relay(&r).await;
        relay_once(&r.t.db, r.org(), "production")
            .await
            .expect_err("A fails");
        let reopened = r.mark().await.expect("the mark");

        // B claimed less than a batch and drains, as of when it began.
        marks::mark_drained(&r.t.db, r.org(), "production", b_started)
            .await
            .expect("B drains");
        let mark = r.mark().await.expect("the mark");
        assert!(
            Some(mark.published_at) > mark.drained_at,
            "still due: {mark:?}"
        );
        assert!(reopened.published_at > b_started, "{reopened:?}");
        assert_eq!(
            reopened.pending_since,
            Some(pending),
            "already due: the reopen keeps the wait"
        );
        // A drain vouches nothing older than its start waits, so it may move
        // the wait up to that start, never clear it while the mark is due.
        assert!(mark.pending_since >= Some(pending), "{mark:?}");
        assert_eq!(r.waiting().await, 1, "A's row waits for the next pass");
    })
    .await;
}

/// Set event `id`'s outbox row's `created_at` to `seconds` ago, in the store's
/// clock: a row that has waited that long.
async fn age_outbox_row_by(r: &Relay, id: Uuid, seconds: f64) {
    owner_of(r)
        .await
        .execute(
            "UPDATE oxy_events.outbox SET created_at = now() - make_interval(secs => $2) \
              WHERE id = $1",
            &[&id, &seconds],
        )
        .await
        .expect("age the outbox row");
}

/// R40, R42: a pass beside rows another pass holds never drains. Should the
/// holder hang (heartbeating, or stuck behind a poison row), its row keeps
/// ageing and the wait follows the oldest row actually waiting, so the
/// stale alarm fires; the holder's failure does not restart it.
#[tokio::test]
async fn a_pass_beside_a_hung_holder_never_drains_and_still_alarms() {
    with_relay(|r| async move {
        let held_id = r.publish_event("production", EVENT, "statement:1", 0).await;
        age_outbox_row_by(&r, held_id, 600.0).await;
        marks::mark_published(&r.t.db, r.org(), "production")
            .await
            .expect("mark");
        backdate_mark(&r, "published_at", 120).await;
        backdate_mark(&r, "pending_since", 600).await;
        let still_stale = |why: &'static str| {
            let r = r.clone();
            async move {
                let mark = r.mark().await.expect("the mark");
                assert!(
                    Some(mark.published_at) > mark.drained_at,
                    "{why}: still due: {mark:?}"
                );
                let since = mark.pending_since.expect("still waiting");
                assert!(
                    Utc::now().fixed_offset() - since > chrono::Duration::minutes(9),
                    "{why}: the wait follows the hung row: {mark:?}"
                );
                let swept = sweep_once(&r.t.db).await.expect("sweep");
                assert_eq!(swept.stale, [r.org()], "{why}: the alarm");
            }
        };

        // Another pass holds the waiting row, and hangs.
        let held = outbox::claim(&r.t.db, r.org(), &OutboxTarget::Production, BATCH)
            .await
            .expect("hold the row");
        assert_eq!(held.rows().len(), 1);

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("an empty pass");
        assert_eq!(stats, RelayStats::default());
        still_stale("an empty claim beside a hung holder").await;

        r.publish_event("production", EVENT, "statement:2", 0).await;
        marks::mark_published(&r.t.db, r.org(), "production")
            .await
            .expect("mark");
        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("a short pass");
        assert_eq!(stats.relayed, 1, "{stats:?}");
        still_stale("a short claim beside a hung holder").await;

        // The holder fails at last: it releases its row and reopens the mark.
        held.release().await.expect("release");
        marks::reopen(&r.t.db, r.org(), "production")
            .await
            .expect("reopen");
        still_stale("the holder failed: the wait was not restarted").await;
        assert_eq!(r.waiting().await, 1, "the held row waits for the next pass");
    })
    .await;
}

/// R42: relay passes overlap under steady publishing (`enqueue_relay` dedupes
/// queued tasks only), so a pass often finds rows the previous pass still
/// holds. Those rows are seconds old: the wait follows the oldest row
/// actually waiting, moving up to it, so a healthy relay never looks stale —
/// though no pass drains while another holds rows.
#[tokio::test]
async fn overlapping_passes_under_steady_publishing_do_not_pin_the_wait() {
    with_relay(|r| async move {
        r.publish_event("production", EVENT, "statement:1", 0).await;
        marks::mark_published(&r.t.db, r.org(), "production")
            .await
            .expect("mark");
        let pending = backdate_mark(&r, "pending_since", 600).await;

        // The previous pass holds the row it claimed a moment ago.
        let held = outbox::claim(&r.t.db, r.org(), &OutboxTarget::Production, BATCH)
            .await
            .expect("the previous pass");
        r.publish_event("production", EVENT, "statement:2", 0).await;
        marks::mark_published(&r.t.db, r.org(), "production")
            .await
            .expect("mark");
        let before = marks::platform_now(&r.t.db).await.expect("now");

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("the next pass");
        assert_eq!(stats.relayed, 1, "{stats:?}");
        let mark = r.mark().await.expect("the mark");
        assert!(
            Some(mark.published_at) > mark.drained_at,
            "not drained while a row is held: {mark:?}"
        );
        let since = mark.pending_since.expect("still waiting");
        assert!(
            since > pending && before.fixed_offset() - since < chrono::Duration::minutes(1),
            "the wait moved up to the held row, seconds old: {mark:?}"
        );
        let swept = sweep_once(&r.t.db).await.expect("sweep");
        assert!(swept.stale.is_empty(), "{swept:?}");
        held.release().await.expect("release");
    })
    .await;
}

/// R42: a backlog past one batch leaves rows waiting after a full pass; the
/// wait follows the oldest of them, so a relay that cannot keep up — its
/// oldest row ageing — still alarms.
#[tokio::test]
async fn a_backlog_past_a_batch_still_alarms() {
    with_relay(|r| async move {
        let db = &r.t.db;
        let conn = resolve_writer_connection_for_org(db, r.org(), &r.store.writer)
            .await
            .expect("resolve the writer");
        outbox::ensure_publishable(db, r.org(), &OutboxTarget::Production, &conn.role)
            .await
            .expect("ensure publishable");
        let revision = live_revision(db, r.publisher, "production", QUIET)
            .await
            .expect("read the live revision")
            .map(|rev| rev.id);
        let writer = connect(&conn.dsn).await;
        let event_type = r.event_type(QUIET);
        for n in 0..=BATCH {
            let key = format!("report:{n}");
            let id = event_id(r.org(), "production", &event_type, &key);
            let row = Row {
                id,
                environment: "production",
                name: QUIET,
                key: &key,
                revision,
                delay: 0,
                data: json!({}),
            };
            let lineage = Lineage {
                actor: None,
                ..Lineage::top_level(id)
            };
            let mac = r.host_mac(&r.store.writer, &conn.role, &row, &lineage);
            writer
                .query_one(
                    PUBLISH_SQL,
                    &[
                        &id,
                        &"production",
                        &QUIET,
                        &key,
                        &json!({}),
                        &revision,
                        &None::<String>,
                        &None::<Uuid>,
                        &id,
                        &0i32,
                        &None::<Uuid>,
                        &0i32,
                        &mac,
                    ],
                )
                .await
                .expect("publish");
        }
        owner_of(&r)
            .await
            .execute(
                "UPDATE oxy_events.outbox SET created_at = now() - interval '10 minutes'",
                &[],
            )
            .await
            .expect("age the backlog");
        marks::mark_published(db, r.org(), "production")
            .await
            .expect("mark");
        backdate_mark(&r, "published_at", 600).await;
        backdate_mark(&r, "pending_since", 600).await;

        let stats = relay_once(db, r.org(), "production").await.expect("relay");
        assert_eq!(stats.relayed, BATCH as usize, "{stats:?}");
        let mark = r.mark().await.expect("the mark");
        assert_eq!(mark.drained_at, None, "a full batch never drains");
        let since = mark.pending_since.expect("still waiting");
        assert!(
            Utc::now().fixed_offset() - since > chrono::Duration::minutes(9),
            "the wait follows the oldest row left: {mark:?}"
        );
        let swept = sweep_once(db).await.expect("sweep");
        assert_eq!(swept.stale, [r.org()], "{swept:?}");
    })
    .await;
}

/// R42: under steady publishing a publish lands during nearly every pass, so
/// the mark stays due though every row waits under a second. A short pass
/// that left nothing older waiting moves the wait up to its own start: a
/// healthy outbox never looks stale. (Its publish here is dated a minute
/// ahead: it lands after the pass began.)
#[tokio::test]
async fn a_pass_that_relayed_everything_older_moves_the_wait_up_to_its_start() {
    with_relay(|r| async move {
        r.publish_event("production", EVENT, "statement:1", 0).await;
        marks::mark_published(&r.t.db, r.org(), "production")
            .await
            .expect("mark");
        let pending = backdate_mark(&r, "pending_since", 600).await;
        let published = backdate_mark(&r, "published_at", -60).await;
        let before = marks::platform_now(&r.t.db).await.expect("now");

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(stats.relayed, 1, "{stats:?}");
        let mark = r.mark().await.expect("the mark");
        assert_eq!(mark.published_at, published);
        assert!(
            Some(mark.published_at) > mark.drained_at,
            "the later publish keeps it due: {mark:?}"
        );
        let since = mark.pending_since.expect("still waiting");
        assert!(
            since >= before && since > pending,
            "the wait moved up to the pass's start: {mark:?}, pass began after {before}"
        );
        let swept = sweep_once(&r.t.db).await.expect("sweep");
        assert!(swept.stale.is_empty(), "{swept:?}");
    })
    .await;
}

/// A pass that claims a full batch queues the next relay itself, so a backlog
/// drains without waiting for the sweep.
#[tokio::test]
async fn a_full_batch_queues_the_next_relay() {
    with_relay(|r| async move {
        let db = &r.t.db;
        let conn = resolve_writer_connection_for_org(db, r.org(), &r.store.writer)
            .await
            .expect("resolve the writer");
        outbox::ensure_publishable(db, r.org(), &OutboxTarget::Production, &conn.role)
            .await
            .expect("ensure publishable");
        let revision = live_revision(db, r.publisher, "production", QUIET)
            .await
            .expect("read the live revision")
            .map(|rev| rev.id);
        let writer = connect(&conn.dsn).await;
        let event_type = r.event_type(QUIET);
        for n in 0..=BATCH {
            let key = format!("report:{n}");
            let id = event_id(r.org(), "production", &event_type, &key);
            let row = Row {
                id,
                environment: "production",
                name: QUIET,
                key: &key,
                revision,
                delay: 0,
                data: json!({}),
            };
            let lineage = Lineage {
                actor: None,
                ..Lineage::top_level(id)
            };
            let mac = r.host_mac(&r.store.writer, &conn.role, &row, &lineage);
            writer
                .query_one(
                    PUBLISH_SQL,
                    &[
                        &id,
                        &"production",
                        &QUIET,
                        &key,
                        &json!({}),
                        &revision,
                        &None::<String>,
                        &None::<Uuid>,
                        &id,
                        &0i32,
                        &None::<Uuid>,
                        &0i32,
                        &mac,
                    ],
                )
                .await
                .expect("publish");
        }

        marks::mark_published(db, r.org(), "production")
            .await
            .expect("mark");
        let stats = relay_once(db, r.org(), "production").await.expect("relay");
        assert_eq!(stats.relayed, BATCH as usize, "{stats:?}");
        assert_eq!(r.waiting().await, 1, "one row is left for the next pass");
        let queued = r.relay_tasks().await;
        assert_eq!(queued.len(), 1, "the next relay is queued: {queued:?}");
        assert_eq!(
            r.mark().await.expect("the mark").drained_at,
            None,
            "a full batch may have left rows: not drained"
        );

        // A pass that drained the outbox queues nothing more.
        let rest = relay_once(db, r.org(), "production")
            .await
            .expect("relay the rest");
        assert_eq!(rest.relayed, 1);
        assert_eq!(r.relay_tasks().await.len(), 1);
        assert!(r.mark().await.expect("the mark").drained_at.is_some());
    })
    .await;
}

/// One `oxy_events.publish` as the publisher's writer whose payload is the
/// `jsonb` Postgres reads from `body`: a body only Postgres can hold, such as
/// a number past `f64`.
async fn publish_jsonb_text(r: &Relay, name: &str, key: &str, body: &str) -> Uuid {
    let db = &r.t.db;
    let id = event_id(r.org(), "production", &r.event_type(name), key);
    let revision = live_revision(db, r.publisher, "production", name)
        .await
        .expect("read the live revision")
        .map(|rev| rev.id);
    let conn = resolve_writer_connection_for_org(db, r.org(), &r.store.writer)
        .await
        .expect("resolve the writer");
    outbox::ensure_publishable(db, r.org(), &OutboxTarget::Production, &conn.role)
        .await
        .expect("ensure publishable");
    let writer = connect(&conn.dsn).await;
    // The size the store will report, which the host signs: Rust cannot hold
    // this body, so Postgres measures it.
    let data_bytes: i32 = writer
        .query_one("SELECT octet_length($1::text::jsonb::text)", &[&body])
        .await
        .expect("measure the body")
        .get(0);
    let event_type = r.event_type(name);
    let mac = sign_hex(
        r.org(),
        &Provenance {
            id,
            environment: "production",
            event_type: &event_type,
            key,
            writer_role: &conn.role,
            actor: None,
            root_id: id,
            causation_id: None,
            depth: 0,
            delay_seconds: 0,
            data_bytes: i64::from(data_bytes),
            schema_revision: revision,
            invocation_id: None,
        },
    );
    writer
        .query_one(
            "SELECT oxy_events.publish($1,$2,$3,$4,$5::text::jsonb,$6,$7,$8,$9,$10,$11,$12,\
             pg_catalog.decode($13, 'hex'))",
            &[
                &id,
                &"production",
                &name,
                &key,
                &body,
                &revision,
                &None::<String>,
                &None::<Uuid>,
                &id,
                &0i32,
                &None::<Uuid>,
                &0i32,
                &mac,
            ],
        )
        .await
        .unwrap_or_else(|e| panic!("publish: {e:?}"));
    id
}

/// The org's store, as its owner.
async fn owner_of(r: &Relay) -> tokio_postgres::Client {
    connect(&oxy_oltp::migrator::owner_dsn(&r.tenant().await).expect("owner dsn")).await
}

/// How many outbox rows hold event `id`'s body, relayed or not.
async fn bodies(r: &Relay, id: Uuid) -> i64 {
    owner_of(r)
        .await
        .query_one(
            "SELECT count(*) FROM oxy_events.outbox WHERE id = $1",
            &[&id],
        )
        .await
        .expect("count the bodies")
        .get(0)
}

/// Set `id`'s `published_at` `days` back.
async fn age_event(r: &Relay, id: Uuid, days: i64) {
    r.t.db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE app_events SET published_at = $1 WHERE id = $2",
            [
                (Utc::now() - chrono::Duration::days(days)).into(),
                id.into(),
            ],
        ))
        .await
        .expect("age the event");
}

/// R26: the relay never reads an event's body, so a body Rust cannot even
/// decode — a number past `f64`, which `jsonb` keeps — is relayed and
/// delivered like any other. Oxy's database has no column for a body: it
/// stores the event's routing metadata, the body's size and the writer role
/// that names it; the listener's task carries the envelope with `data: null`;
/// the body stays in the org's store, marked relayed, for the delivery to read.
#[tokio::test]
async fn the_relay_never_reads_a_body() {
    with_relay(|r| async move {
        let a = r.listener("relay-a", &[r.event_type(EVENT)]).await;
        let id = publish_jsonb_text(
            &r,
            EVENT,
            "statement:1",
            r#"{"period": "2026-11", "n": 1e400}"#,
        )
        .await;

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(
            (stats.relayed, stats.deliveries, stats.rejected),
            (1, 1, 0),
            "{stats:?}"
        );

        let columns: Vec<String> =
            r.t.db
                .query_all_raw(Statement::from_string(
                    DatabaseBackend::Postgres,
                    "SELECT column_name::text AS name FROM information_schema.columns \
                  WHERE table_name = 'app_events'",
                ))
                .await
                .expect("read the columns")
                .iter()
                .map(|row| row.try_get("", "name").expect("name"))
                .collect();
        assert!(
            !columns.iter().any(|c| c == "data"),
            "no body in Oxy's database: {columns:?}"
        );
        let stored =
            r.t.db
                .query_one_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "SELECT data_bytes, writer_role FROM app_events WHERE id = $1",
                    [id.into()],
                ))
                .await
                .expect("read the event")
                .expect("the event is stored");
        let owner = owner_of(&r).await;
        let body = owner
            .query_one(
                "SELECT octet_length(data::text), writer_role, relayed_at IS NOT NULL \
                   FROM oxy_events.outbox WHERE id = $1",
                &[&id],
            )
            .await
            .expect("the body stays in the org's store");
        let (size, role, relayed): (i32, String, bool) = (body.get(0), body.get(1), body.get(2));
        assert!(relayed, "the row is marked relayed, not deleted");
        assert_eq!(stored.try_get::<i32>("", "data_bytes").unwrap(), size);
        assert_eq!(stored.try_get::<String>("", "writer_role").unwrap(), role);

        let tasks = r.event_tasks().await;
        assert_eq!(tasks.len(), 1, "{tasks:?}");
        assert_eq!(tasks[0].payload["app_id"], json!(a.to_string()));
        assert_eq!(tasks[0].payload["input"]["id"], json!(id.to_string()));
        assert_eq!(
            tasks[0].payload["input"]["data"],
            Value::Null,
            "the task carries no body"
        );
    })
    .await;
}

/// R26: a relayed event's body stays in the org's store until retention
/// deletes the event, and retention deletes it with the event.
#[tokio::test]
async fn an_events_body_stays_in_the_store_until_retention_deletes_it() {
    with_relay(|r| async move {
        let id = r.publish_event("production", QUIET, "report:1", 0).await;
        relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(bodies(&r, id).await, 1, "relayed, the body stays");

        age_event(&r, id, 31).await;
        let stats =
            oxy_api_custom_apps::app_events::retention::sweep_org(&r.t.db, Utc::now(), r.org())
                .await
                .expect("sweep");
        assert_eq!(stats.events_deleted, 1, "{stats:?}");
        assert!(r.events().await.is_empty());
        assert_eq!(bodies(&r, id).await, 0, "the body went with the event");
    })
    .await;
}

/// Set the outbox row of event `id`'s `column` to `days` ago, in the store.
async fn age_outbox_row(r: &Relay, id: Uuid, column: &str, days: i32) {
    owner_of(r)
        .await
        .execute(
            &format!(
                "UPDATE oxy_events.outbox SET {column} = now() - make_interval(days => $2) \
                  WHERE id = $1"
            ),
            &[&id, &days],
        )
        .await
        .unwrap_or_else(|e| panic!("age the outbox row's {column}: {e}"));
}

/// R35: deleting an app deletes its events (`ON DELETE CASCADE`) but not
/// their bodies in the org's store, which retention, keyed on events, cannot
/// find. When retention connects to the org's store anyway, it also deletes
/// relayed bodies relayed more than retention and a day ago — never a
/// younger one, and never a row still waiting, however old.
#[tokio::test]
async fn retention_deletes_old_relayed_bodies_no_event_points_at() {
    with_relay(|r| async move {
        let orphan = r
            .publish_event("production", QUIET, "report:orphan", 0)
            .await;
        let young = r
            .publish_event("production", QUIET, "report:young", 0)
            .await;
        let expired = r
            .publish_event("production", QUIET, "report:expired", 0)
            .await;
        relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        let waiting = r
            .publish_event("production", QUIET, "report:waiting", 0)
            .await;
        // Their events gone, as an app's deletion takes them.
        r.t.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "DELETE FROM app_events WHERE id = ANY($1)",
                [vec![orphan, young].into()],
            ))
            .await
            .expect("delete the events");
        age_outbox_row(&r, orphan, "relayed_at", 32).await;
        age_outbox_row(&r, young, "relayed_at", 2).await;
        age_outbox_row(&r, waiting, "created_at", 40).await;
        // An event past retention, so the pass connects to the org's store.
        age_event(&r, expired, 31).await;

        let stats =
            oxy_api_custom_apps::app_events::retention::sweep_org(&r.t.db, Utc::now(), r.org())
                .await
                .expect("sweep");
        assert_eq!(stats.events_deleted, 1, "{stats:?}");
        assert_eq!(bodies(&r, expired).await, 0, "deleted with its event");
        assert_eq!(bodies(&r, orphan).await, 0, "relayed 32 days ago: deleted");
        assert_eq!(stats.orphan_bodies_deleted, 1, "{stats:?}");
        assert_eq!(bodies(&r, young).await, 1, "relayed 2 days ago: kept");
        assert_eq!(bodies(&r, waiting).await, 1, "never relayed: kept");
    })
    .await;
}

/// R26: when retention cannot reach the org's store, it keeps the event's
/// metadata, so no body is left with nothing pointing at it, and deletes both
/// on a later pass that can.
#[tokio::test]
async fn retention_keeps_an_event_whose_store_it_cannot_reach() {
    with_relay(|r| async move {
        let id = r.publish_event("production", QUIET, "report:1", 0).await;
        relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        age_event(&r, id, 31).await;
        let set_status = |status: &'static str| {
            let r = r.clone();
            async move {
                r.t.db
                    .execute_raw(Statement::from_sql_and_values(
                        DatabaseBackend::Postgres,
                        "UPDATE oltp_tenants SET status = $1 WHERE org_id = $2",
                        [status.into(), r.org().into()],
                    ))
                    .await
                    .expect("set the store's status");
            }
        };
        let sweep =
            || oxy_api_custom_apps::app_events::retention::sweep_org(&r.t.db, Utc::now(), r.org());

        // A store the platform cannot use.
        set_status("failed").await;
        let unreachable = sweep().await;
        set_status("active").await;
        let unreachable = unreachable.expect("sweep");
        assert_eq!(unreachable.events_deleted, 0, "{unreachable:?}");
        assert_eq!(
            r.events().await.len(),
            1,
            "the metadata waits for the store"
        );
        assert_eq!(bodies(&r, id).await, 1);

        let reached = sweep().await.expect("sweep");
        assert_eq!(reached.events_deleted, 1, "{reached:?}");
        assert!(r.events().await.is_empty());
        assert_eq!(bodies(&r, id).await, 0);
    })
    .await;
}
