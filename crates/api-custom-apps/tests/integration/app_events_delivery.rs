//! Delivering an event to a listener (app events design §6.6): the relay's
//! `app_function` task (trigger `event`) runs through the production executor,
//! which opens an attempt on the delivery, reads the event's body from the
//! org's OLTP store, runs the listener's live build with the envelope as its
//! body and `ctx.delivery` set, and records how the attempt ended: `ok` acks
//! the delivery, a retryable failure leaves it pending for the queue's next
//! try (dead once the last one fails), and a `NonRetryableError`, a listener
//! that stopped subscribing, or a body no attempt can read kills it at once.
//!
//! Every event takes the real road: published into the org's outbox through
//! the definer function as the publisher's writer role (the statement
//! `ctx.events.publish` sends), then relayed by [`relay_once`], so its body is
//! in the org's store and nowhere else, the listener's subscription is the one
//! its promoted build registered, and the task is the one the relay queues
//! (its `input` carrying the envelope with `data: null`). The task is read back
//! from the queue and handed to `AppFunctionTaskExecutor`, as a worker would
//! claim it. Each test owns its org's OLTP store and drops it afterwards.
//!
//! A listener's answer is never kept (its output stays out of Oxy's Postgres,
//! like the body): a run answers `delivered (HTTP n)`. So a listener whose
//! test needs what it saw reports it by publishing it as [`CHILD`] in the
//! org's store, and the test reads that back ([`Fx::reported`]).

use std::collections::BTreeMap;
use std::sync::Arc;

use agentic_core::delegation::{TaskOutcome, TaskSpec};
use chrono::Utc;
use entity::{
    app_event_deliveries as deliveries, app_event_delivery_attempts as attempts, app_events,
};
use futures::FutureExt as _;
use oxy_api_custom_apps::app_events::MAX_ERROR_CHARS;
use oxy_api_custom_apps::app_events::deliveries::{
    AttemptOutcome, BODY_GONE, CRASHED, DeliveryStart, begin, finish_err, finish_ok,
};
use oxy_api_custom_apps::app_events::relay::relay_once;
use oxy_api_custom_apps::app_events::retention::sweep_org;
use oxy_api_custom_apps::app_events::schema::live_revision;
use oxy_api_custom_apps::custom_apps_publish::{OrgRef, PublishInput, publish};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use oxy_custom_apps::events::{event_id, qualified_type};
use oxy_oltp::WriterRef;
use oxy_oltp::entity::tenants::{self as oltp_tenants, Entity as OltpTenants};
use oxy_oltp::events::{self as outbox, OutboxKey, OutboxTarget, PUBLISH_SQL};
use oxy_oltp::resolver::resolve_writer_connection_for_org;
use oxy_oltp::schema::app_writer_name;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait,
    QueryFilter, QueryOrder, Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app_events_publish::enable_app_events;
use crate::custom_app_functions_fixture::{
    FunctionSpec, Tenant, invocations, seeded_tenant, throwaway_org,
};
use crate::custom_app_functions_shape_zoo::{compile, write_config};
use crate::custom_app_functions_shape_zoo_oltp::AppStore;
use crate::env_schedules::run_spec;

/// The publisher's event, which every listener here listens to.
const EVENT: &str = "statement.uploaded";
/// What the chaining listener publishes in turn.
const CHILD: &str = "statement.filed";
/// The one function of every listener.
pub(crate) const FUNCTION: &str = "on-event";

/// Answers with the body it was handed, parsed.
pub(crate) const ECHO_JS: &str =
    "export default async (req) => Response.json({ body: JSON.parse(req.body) });";

/// Throws a plain error, every time.
pub(crate) const BROKEN_JS: &str =
    "export default async () => { throw new Error(\"the listener broke\"); };";

/// Throws an error far longer than an attempt keeps.
const LONG_ERROR_JS: &str =
    "export default async () => { throw new Error(\"too long \".repeat(1000)); };";

/// The SDK's `NonRetryableError`, as `@oxy-hq/sdk/events` compiles it.
const NON_RETRYABLE_JS: &str = r#"
class NonRetryableError extends Error {
  name = "NonRetryableError";
  constructor(message) {
    super(message);
    Object.setPrototypeOf(this, NonRetryableError.prototype);
  }
}
export default async () => { throw new NonRetryableError("statement has no period"); };
"#;

/// Fails its first attempt and succeeds on the next.
const SECOND_TIME_JS: &str = r#"
export default async (req, ctx) => {
  if (ctx.delivery.attempt === 1) throw new Error("not yet");
  return Response.json({ ok: true });
};
"#;

/// Reports `ctx.delivery` by publishing it as [`CHILD`], keyed by attempt
/// (a listener's answer is never kept, so the test reads what it saw back
/// from the org's store), then answers a 500 on the first attempt and a 200
/// after.
const DELIVERY_JS: &str = r#"
export default async (req, ctx) => {
  await ctx.events.publish("statement.filed", { delivery: ctx.delivery }, { key: `attempt-${ctx.delivery.attempt}` });
  return new Response("{}", { status: ctx.delivery.attempt === 1 ? 500 : 200 });
};
"#;

/// Reports the envelope it was handed by publishing it as [`CHILD`] under
/// key `received`, then answers 200.
const RECEIVED_JS: &str = r#"
export default async (req, ctx) => {
  await ctx.events.publish("statement.filed", { envelope: JSON.parse(req.body) }, { key: "received" });
  return Response.json({ ok: true });
};
"#;

/// Throws an error carrying the event's body.
const THROW_BODY_JS: &str = r#"
export default async (req) => {
  const event = JSON.parse(req.body);
  throw new Error("listener saw: " + JSON.stringify(event.data));
};
"#;

/// Answers with the body it was handed: a 500 on the first attempt, a 200
/// after.
const ECHO_TWICE_JS: &str = r#"
export default async (req, ctx) =>
  new Response(req.body, { status: ctx.delivery.attempt === 1 ? 500 : 200 });
"#;

/// Never returns: the run's timeout ends it.
const SPIN_JS: &str = "export default async () => { while (true) {} };";

/// Publishes [`CHILD`] while handling the event.
const CHAIN_JS: &str = r#"
export default async (req, ctx) =>
  Response.json(await ctx.events.publish("statement.filed", { period: "2026-11" }, { key: "filed-1" }));
"#;

fn schema() -> Value {
    json!({ "type": "object", "properties": { "period": { "type": "string" } } })
}

/// The body every event here is published with.
pub(crate) fn published() -> Value {
    json!({ "period": "2026-11", "total": 12.5, "lines": ["a", "b"] })
}

/// Run `delivery`'s queued task once through the production executor, as a
/// worker would.
pub(crate) async fn run_delivery(
    db: &sea_orm::DatabaseConnection,
    delivery: &deliveries::Model,
) -> TaskOutcome {
    let run_id = delivery.run_id.clone().expect("a pending delivery's run");
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT spec FROM agentic_task_queue WHERE task_id = $1",
            [run_id.into()],
        ))
        .await
        .expect("read the queue")
        .expect("the delivery's task");
    let spec: Value = row.try_get("", "spec").expect("spec");
    run_spec(db, serde_json::from_value(spec).expect("a task spec")).await
}

/// An org with app events on, its OLTP store (holding the publisher's
/// writer), a compiled workspace, and a publisher app whose promoted build
/// declares [`EVENT`].
pub(crate) struct Fx {
    pub(crate) t: Tenant,
    pub(crate) workspace: Uuid,
    publisher: Uuid,
    publisher_slug: String,
    pub(crate) store: AppStore,
    pub(crate) tag: String,
    _root: tempfile::TempDir,
}

/// A listener app: its id and slug.
pub(crate) struct Listener {
    pub(crate) app_id: Uuid,
    pub(crate) slug: String,
}

impl Fx {
    async fn new() -> Self {
        let t = throwaway_org(&seeded_tenant().await).await;
        enable_app_events(&t, &[t.org_id]).await;
        let tag = Uuid::new_v4().simple().to_string()[..8].to_string();
        let root = write_config("  - name: duck\n    type: duckdb\n    path: events.duckdb\n");
        let workspace = compile(&t, root.path()).await;
        let publisher_slug = format!("dlv-pub-{tag}");
        let store = AppStore::new(&t, &publisher_slug).await;
        store.provision(workspace).await;
        let publisher = publish_app(
            &t,
            &publisher_slug,
            workspace,
            "1",
            &[],
            json!({ "events": { EVENT: { "schema": schema() } } }),
        )
        .await;
        Self {
            t,
            workspace,
            publisher,
            publisher_slug,
            store,
            tag,
            _root: root,
        }
    }

    pub(crate) fn event_type(&self) -> String {
        qualified_type(&self.publisher_slug, EVENT)
    }

    fn listener_slug(&self) -> String {
        format!("dlv-{}", self.tag)
    }

    /// Publish the listener app, promoted: [`FUNCTION`] runs `js` and listens
    /// to [`EVENT`], with `manifest`'s keys beside `on`.
    pub(crate) async fn listener(&self, js: &'static str, manifest: Value) -> Listener {
        self.listener_build("1", js, manifest, json!({})).await
    }

    /// [`Self::listener`] allowed to publish [`CHILD`] as its own writer in
    /// the org's store: how a listener here reports what it saw
    /// ([`Self::reported`]).
    async fn reporting_listener(&self, js: &'static str) -> Listener {
        let writer = WriterRef::app(
            app_writer_name(&self.listener_slug()).expect("the slug backs a schema"),
        )
        .expect("a valid writer name");
        self.store
            .ensure_extra_writer(&writer, self.workspace)
            .await;
        self.listener_build(
            "1",
            js,
            json!({ "events": { "publish": [CHILD] } }),
            json!({ "events": { CHILD: { "schema": schema() } } }),
        )
        .await
    }

    /// [`Self::listener`] as build `build`, with the app manifest's keys in
    /// `extra`.
    async fn listener_build(
        &self,
        build: &str,
        js: &'static str,
        manifest: Value,
        extra: Value,
    ) -> Listener {
        let mut entry = json!({ "on": [self.event_type()], "timeoutSeconds": 30 });
        if let (Some(entry), Some(more)) = (entry.as_object_mut(), manifest.as_object()) {
            entry.extend(more.clone());
        }
        let slug = self.listener_slug();
        let function = FunctionSpec {
            name: FUNCTION,
            manifest: entry,
            js,
        };
        let app_id = publish_app(&self.t, &slug, self.workspace, build, &[function], extra).await;
        Listener { app_id, slug }
    }

    /// Publish [`EVENT`] under `key` with `data` as its body into the org's
    /// production outbox, as the publisher's writer through the definer
    /// function, by `user:ada` and rooted at `root` (the event itself when
    /// `None`), against the revision production's build declares; its id.
    async fn publish(&self, key: &str, root: Option<Uuid>, data: &Value) -> Uuid {
        let db = &self.t.db;
        let id = event_id(self.t.org_id, "production", &self.event_type(), key);
        let revision = live_revision(db, self.publisher, "production", EVENT)
            .await
            .expect("read the live revision")
            .map(|r| r.id);
        let conn = resolve_writer_connection_for_org(db, self.t.org_id, &self.store.writer)
            .await
            .expect("resolve the publisher's writer");
        outbox::ensure_publishable(db, self.t.org_id, &OutboxTarget::Production, &conn.role)
            .await
            .expect("ensure publishable");
        connect(&conn.dsn)
            .await
            .query_one(
                PUBLISH_SQL,
                &[
                    &id,
                    &"production",
                    &EVENT,
                    &key,
                    data,
                    &revision,
                    &Some("user:ada"),
                    &None::<Uuid>,
                    &root.unwrap_or(id),
                    &0i32,
                    &None::<Uuid>,
                    &0i32,
                ],
            )
            .await
            .unwrap_or_else(|e| panic!("publish: {e:?}"));
        id
    }

    /// Publish [`EVENT`] under `key` with [`published`] as its body and relay
    /// the outbox; `listener`'s delivery of it.
    pub(crate) async fn deliver(&self, key: &str, listener: &Listener) -> deliveries::Model {
        self.deliver_data(key, None, &published(), listener).await
    }

    /// [`Self::deliver`], the event rooted at `root` with `data` as its body.
    async fn deliver_data(
        &self,
        key: &str,
        root: Option<Uuid>,
        data: &Value,
        listener: &Listener,
    ) -> deliveries::Model {
        let id = self.publish(key, root, data).await;
        relay_once(&self.t.db, self.t.org_id, "production")
            .await
            .expect("relay the outbox");
        deliveries::Entity::find()
            .filter(deliveries::Column::EventId.eq(id))
            .filter(deliveries::Column::AppId.eq(listener.app_id))
            .one(&self.t.db)
            .await
            .expect("read the delivery")
            .expect("the listener's delivery")
    }

    /// What a [`Self::reporting_listener`] published, by key: the bodies of
    /// the outbox rows still waiting for a relay, read back from the org's
    /// store (the events this test relayed are no longer waiting).
    pub(crate) async fn reported(&self) -> BTreeMap<String, Value> {
        let db = &self.t.db;
        let claim = outbox::claim(db, self.t.org_id, &OutboxTarget::Production, 100)
            .await
            .expect("claim the outbox");
        let rows = claim.rows().to_vec();
        claim.release().await.expect("release the claim");
        let mut reported = BTreeMap::new();
        for row in rows {
            let body = outbox::read_body(
                db,
                self.t.org_id,
                &OutboxTarget::Production,
                &row.writer_role,
                row.id,
            )
            .await
            .expect("read a reported body")
            .expect("the body is there");
            reported.insert(row.key, body);
        }
        reported
    }

    /// Every table of Oxy's Postgres (the platform database's schema) with
    /// a row holding `text` anywhere, and how many: empty when no copy of it
    /// was kept.
    async fn platform_rows_holding(&self, text: &str) -> Vec<(String, i64)> {
        let db = &self.t.db;
        let tables: Vec<String> = db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT table_name::text AS table_name FROM information_schema.tables \
                  WHERE table_schema = current_schema() AND table_type = 'BASE TABLE' \
                  ORDER BY table_name",
            ))
            .await
            .expect("list the platform tables")
            .iter()
            .map(|r| r.try_get("", "table_name").expect("table_name"))
            .collect();
        assert!(
            tables.iter().any(|t| t == "app_function_invocations"),
            "the scan reaches the platform's tables: {tables:?}"
        );
        let mut holding = Vec::new();
        for table in tables {
            let row = db
                .query_one_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    format!(
                        "SELECT count(*) AS n FROM \"{table}\" t \
                          WHERE t::text LIKE '%' || $1 || '%'"
                    ),
                    [text.into()],
                ))
                .await
                .unwrap_or_else(|e| panic!("scan {table}: {e}"))
                .expect("a count");
            let n: i64 = row.try_get("", "n").expect("n");
            if n > 0 {
                holding.push((table, n));
            }
        }
        holding
    }

    /// The stored event `delivery` delivers: its metadata only.
    pub(crate) async fn event(&self, delivery: &deliveries::Model) -> app_events::Model {
        app_events::Entity::find_by_id(delivery.event_id)
            .one(&self.t.db)
            .await
            .expect("read the event")
            .expect("the event")
    }

    /// The delivery's queued task, by its `run_id`: what a worker claims.
    pub(crate) async fn task(&self, delivery: &deliveries::Model) -> (TaskSpec, Value) {
        let run_id = delivery.run_id.clone().expect("a pending delivery's run");
        let row = self
            .t
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT spec FROM agentic_task_queue WHERE task_id = $1",
                [run_id.into()],
            ))
            .await
            .expect("read the queue")
            .expect("the delivery's task");
        let spec: Value = row.try_get("", "spec").expect("spec");
        let input = spec["payload"]["input"].clone();
        (serde_json::from_value(spec).expect("a task spec"), input)
    }

    /// Run the delivery's task once through the production executor.
    pub(crate) async fn run(&self, delivery: &deliveries::Model) -> TaskOutcome {
        let (spec, _) = self.task(delivery).await;
        run_spec(&self.t.db, spec).await
    }

    pub(crate) async fn reload(&self, delivery: &deliveries::Model) -> deliveries::Model {
        deliveries::Entity::find_by_id(delivery.id)
            .one(&self.t.db)
            .await
            .expect("read the delivery")
            .expect("the delivery is still there")
    }

    pub(crate) async fn attempts(&self, delivery: &deliveries::Model) -> Vec<attempts::Model> {
        attempts::Entity::find()
            .filter(attempts::Column::DeliveryId.eq(delivery.id))
            .order_by_asc(attempts::Column::Attempt)
            .all(&self.t.db)
            .await
            .expect("read the attempts")
    }

    async fn tenant(&self) -> oltp_tenants::Model {
        OltpTenants::find()
            .filter(oltp_tenants::Column::OrgId.eq(self.t.org_id))
            .one(&self.t.db)
            .await
            .expect("read the tenant")
            .expect("the org's tenant")
    }

    /// Run `sql` (`$1` = an event id) in the org's store as its owner: what
    /// no app can do. The rows it touched.
    async fn as_owner(&self, sql: &str, id: Uuid) -> u64 {
        let dsn = oxy_oltp::migrator::owner_dsn(&self.tenant().await).expect("owner dsn");
        connect(&dsn)
            .await
            .execute(sql, &[&id])
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e:?}"))
    }

    /// Point the org's tenant row at `host`; the host it had.
    pub(crate) async fn set_store_host(&self, host: &str) -> String {
        let tenant = self.tenant().await;
        let was = tenant.host.clone();
        let mut row: oltp_tenants::ActiveModel = tenant.into();
        row.host = Set(host.to_string());
        row.update(&self.t.db).await.expect("move the tenant");
        was
    }
}

/// Run `body` against a fresh [`Fx`], dropping the org's store (database,
/// then roles) whatever happened.
pub(crate) async fn with_fx<F, Fut>(body: F)
where
    F: FnOnce(Arc<Fx>) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let fx = Arc::new(Fx::new().await);
    // Boxed: a body that publishes several apps outgrows the test thread's
    // stack when pinned on it.
    let outcome = std::panic::AssertUnwindSafe(Box::pin(body(fx.clone())))
        .catch_unwind()
        .await;
    fx.store.cleanup().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
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
/// `slug` in `workspace`, promoted.
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
        build_id: format!("{slug}-{build}"),
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

pub(crate) fn failed(outcome: TaskOutcome) -> String {
    match outcome {
        TaskOutcome::Failed(error) => error,
        other => panic!("the run must fail, so the queue retries: {other:?}"),
    }
}

pub(crate) fn completed(outcome: TaskOutcome) -> String {
    match outcome {
        TaskOutcome::Done { answer, .. } => answer,
        other => panic!("the run must complete: {other:?}"),
    }
}

/// An envelope without its `data`.
fn without_data(envelope: &Value) -> Value {
    let mut envelope = envelope.clone();
    envelope
        .as_object_mut()
        .expect("an envelope is an object")
        .remove("data");
    envelope
}

/// Each attempt's number and outcome, in order.
pub(crate) fn outcomes(tries: &[attempts::Model]) -> Vec<(i32, Option<&str>)> {
    tries
        .iter()
        .map(|a| (a.attempt, a.outcome.as_deref()))
        .collect()
}

#[tokio::test]
async fn a_delivered_event_runs_the_listener_and_acks() {
    with_fx(|fx| async move {
        let listener = fx.reporting_listener(RECEIVED_JS).await;
        let delivery = fx.deliver("statement:1", &listener).await;
        let (_, input) = fx.task(&delivery).await;

        assert_eq!(
            completed(fx.run(&delivery).await),
            "delivered (HTTP 200)",
            "the run's answer is a summary, never the listener's output"
        );
        let reported = fx.reported().await;
        let body = &reported["received"]["envelope"];
        assert_eq!(body["id"], json!(delivery.event_id.to_string()));
        assert_eq!(
            body["data"],
            published(),
            "the listener gets the body the app published, read from the org's store"
        );
        assert_eq!(
            input.get("data"),
            Some(&Value::Null),
            "the task carries no body: {input}"
        );
        assert_eq!(
            without_data(body),
            without_data(&input),
            "the body is the envelope the relay queued, field for field but `data`"
        );

        let runs = invocations(&fx.t.db, listener.app_id, FUNCTION).await;
        assert_eq!(runs.len(), 1, "{runs:?}");
        assert_eq!(runs[0].mode, "event");
        let acked = fx.reload(&delivery).await;
        assert_eq!(acked.status, "acked");
        assert_eq!(acked.attempts, 1);
        assert_eq!(acked.invocation_id, Some(runs[0].id));
        let tries = fx.attempts(&delivery).await;
        assert_eq!(tries.len(), 1, "{tries:?}");
        assert_eq!(tries[0].outcome.as_deref(), Some("ok"));
        assert_eq!(tries[0].invocation_id, Some(runs[0].id));
        assert!(tries[0].finished_at.is_some());
    })
    .await;
}

#[tokio::test]
async fn a_failing_listener_records_the_error_and_retries() {
    with_fx(|fx| async move {
        let listener = fx.listener(BROKEN_JS, json!({})).await;
        let delivery = fx.deliver("statement:1", &listener).await;

        let error = failed(fx.run(&delivery).await);
        assert!(error.contains("the listener broke"), "{error}");

        let pending = fx.reload(&delivery).await;
        assert_eq!(pending.status, "pending", "the queue retries it");
        assert_eq!(pending.attempts, 1);
        let last = pending.last_error.expect("the error is recorded");
        assert!(last.contains("the listener broke"), "{last}");
        let tries = fx.attempts(&delivery).await;
        assert_eq!(tries.len(), 1);
        assert_eq!(tries[0].outcome.as_deref(), Some("error"));
    })
    .await;
}

#[tokio::test]
async fn an_acked_delivery_is_not_run_again() {
    with_fx(|fx| async move {
        let listener = fx.listener(ECHO_JS, json!({})).await;
        let delivery = fx.deliver("statement:1", &listener).await;

        completed(fx.run(&delivery).await);
        completed(fx.run(&delivery).await);

        assert_eq!(
            invocations(&fx.t.db, listener.app_id, FUNCTION).await.len(),
            1,
            "an acked delivery never runs again"
        );
        let acked = fx.reload(&delivery).await;
        assert_eq!(acked.status, "acked");
        assert_eq!(acked.attempts, 1);
        assert_eq!(fx.attempts(&delivery).await.len(), 1);
    })
    .await;
}

#[tokio::test]
async fn a_listener_that_stopped_subscribing_goes_dead() {
    with_fx(|fx| async move {
        let listener = fx.listener(ECHO_JS, json!({})).await;
        let delivery = fx.deliver("statement:1", &listener).await;
        // The next build of the listener still has the function, without `on`.
        publish_app(
            &fx.t,
            &listener.slug,
            fx.workspace,
            "2",
            &[FunctionSpec {
                name: FUNCTION,
                manifest: json!({ "route": true }),
                js: ECHO_JS,
            }],
            json!({}),
        )
        .await;

        completed(fx.run(&delivery).await);

        assert!(
            invocations(&fx.t.db, listener.app_id, FUNCTION)
                .await
                .is_empty(),
            "the listener does not run"
        );
        let dead = fx.reload(&delivery).await;
        assert_eq!(dead.status, "dead");
        assert_eq!(
            dead.last_error.as_deref(),
            Some(format!("listener no longer subscribes to {}", fx.event_type()).as_str())
        );
    })
    .await;
}

#[tokio::test]
async fn a_handler_that_publishes_chains_causation() {
    with_fx(|fx| async move {
        // The listener publishes as its own writer, in the same store.
        let listener = fx.reporting_listener(CHAIN_JS).await;
        // A chain already under way: rooted at some earlier invocation.
        let root = Uuid::new_v4();
        let delivery = fx
            .deliver_data("statement:1", Some(root), &published(), &listener)
            .await;

        completed(fx.run(&delivery).await);

        // The parent was relayed: only the child waits.
        let claim = outbox::claim(&fx.t.db, fx.t.org_id, &OutboxTarget::Production, 10)
            .await
            .expect("claim the outbox");
        let rows = claim.rows().to_vec();
        claim.release().await.expect("release the claim");
        assert_eq!(rows.len(), 1, "{rows:?}");
        let child = &rows[0];
        assert_eq!(child.name, CHILD);
        assert_eq!(child.causation_id, Some(delivery.event_id));
        assert_eq!(child.root_id, root, "the parent's root, not its id");
        assert_eq!(child.depth, 1);
        assert_eq!(child.actor.as_deref(), Some("user:ada"));
    })
    .await;
}

#[tokio::test]
async fn each_attempt_is_recorded_with_its_outcome() {
    with_fx(|fx| async move {
        let listener = fx.listener(SECOND_TIME_JS, json!({})).await;
        let delivery = fx.deliver("statement:1", &listener).await;

        failed(fx.run(&delivery).await);
        completed(fx.run(&delivery).await);

        let runs = invocations(&fx.t.db, listener.app_id, FUNCTION).await;
        assert_eq!(runs.len(), 2);
        let tries = fx.attempts(&delivery).await;
        let summary: Vec<(i32, Option<&str>, Option<Uuid>)> = tries
            .iter()
            .map(|a| (a.attempt, a.outcome.as_deref(), a.invocation_id))
            .collect();
        assert_eq!(
            summary,
            vec![
                (1, Some("error"), Some(runs[0].id)),
                (2, Some("ok"), Some(runs[1].id)),
            ]
        );
        assert!(tries[0].error.as_deref().unwrap_or("").contains("not yet"));
        assert_eq!(tries[1].error, None);
        let acked = fx.reload(&delivery).await;
        assert_eq!((acked.status.as_str(), acked.attempts), ("acked", 2));
    })
    .await;
}

#[tokio::test]
async fn a_timeout_is_recorded_as_timeout() {
    with_fx(|fx| async move {
        let listener = fx.listener(SPIN_JS, json!({ "timeoutSeconds": 1 })).await;
        let delivery = fx.deliver("statement:1", &listener).await;

        failed(fx.run(&delivery).await);

        let tries = fx.attempts(&delivery).await;
        assert_eq!(tries.len(), 1);
        assert_eq!(tries[0].outcome.as_deref(), Some("timeout"));
        assert_eq!(fx.reload(&delivery).await.status, "pending");
    })
    .await;
}

#[tokio::test]
async fn an_unfinished_attempt_is_closed_as_a_crash() {
    with_fx(|fx| async move {
        let listener = fx.listener(ECHO_JS, json!({})).await;
        let delivery = fx.deliver("statement:1", &listener).await;
        // A worker opens the first attempt and dies before it finishes.
        match begin(&fx.t.db, delivery.id).await.expect("begin") {
            DeliveryStart::Run { info, .. } => assert_eq!(info.attempt, 1),
            other => panic!("a pending delivery runs: {other:?}"),
        }

        completed(fx.run(&delivery).await);

        let tries = fx.attempts(&delivery).await;
        assert_eq!(outcomes(&tries), vec![(1, Some("crash")), (2, Some("ok"))]);
        assert!(tries[0].finished_at.is_some());
        assert!(tries[0].error.is_some(), "the crash says what happened");
        let acked = fx.reload(&delivery).await;
        assert_eq!((acked.status.as_str(), acked.attempts), ("acked", 2));
    })
    .await;
}

#[tokio::test]
async fn non_retryable_error_goes_straight_to_dead() {
    with_fx(|fx| async move {
        let listener = fx.listener(NON_RETRYABLE_JS, json!({})).await;
        let delivery = fx.deliver("statement:1", &listener).await;

        completed(fx.run(&delivery).await);

        let dead = fx.reload(&delivery).await;
        assert_eq!(dead.status, "dead");
        assert_eq!(dead.attempts, 1);
        assert_eq!(dead.last_error.as_deref(), Some("statement has no period"));
        let tries = fx.attempts(&delivery).await;
        assert_eq!(tries.len(), 1);
        assert_eq!(tries[0].outcome.as_deref(), Some("non_retryable"));
        assert_eq!(tries[0].error.as_deref(), Some("statement has no period"));
        let runs = invocations(&fx.t.db, listener.app_id, FUNCTION).await;
        assert_eq!(tries[0].invocation_id, Some(runs[0].id));

        // Retried by the queue anyway, it does not run again.
        completed(fx.run(&delivery).await);
        assert_eq!(
            invocations(&fx.t.db, listener.app_id, FUNCTION).await.len(),
            1
        );
    })
    .await;
}

#[tokio::test]
async fn ctx_delivery_reports_the_attempt() {
    with_fx(|fx| async move {
        let listener = fx.reporting_listener(DELIVERY_JS).await;
        let delivery = fx.deliver("statement:1", &listener).await;
        let expected = |attempt: i32| {
            json!({
                "attempt": attempt,
                "maxAttempts": 5,
                "listener": format!("{}/{FUNCTION}", listener.slug),
            })
        };

        let error = failed(fx.run(&delivery).await);
        assert!(
            error.contains("500"),
            "a non-2xx answer is a failure: {error}"
        );
        assert_eq!(completed(fx.run(&delivery).await), "delivered (HTTP 200)");

        let reported = fx.reported().await;
        assert_eq!(reported["attempt-1"]["delivery"], expected(1));
        assert_eq!(reported["attempt-2"]["delivery"], expected(2));
    })
    .await;
}

/// The last attempt failing kills the delivery itself; the retention sweep,
/// finding the task failed afterwards, leaves it as it is.
#[tokio::test]
async fn a_failed_last_attempt_is_dead_and_retention_leaves_it() {
    with_fx(|fx| async move {
        let listener = fx
            .listener(BROKEN_JS, json!({ "retries": { "maxAttempts": 1 } }))
            .await;
        let delivery = fx.deliver("statement:1", &listener).await;

        failed(fx.run(&delivery).await);

        let dead = fx.reload(&delivery).await;
        assert_eq!((dead.status.as_str(), dead.attempts), ("dead", 1));
        let reason = dead.last_error.clone().expect("the reason");
        assert!(reason.contains("the listener broke"), "{reason}");
        assert_eq!(
            fx.attempts(&delivery).await[0].outcome.as_deref(),
            Some("error")
        );

        // The queue gives up on the task: retention finds nothing pending.
        fx.t.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE agentic_task_queue SET queue_status = 'failed' WHERE task_id = $1",
                [dead.run_id.clone().into()],
            ))
            .await
            .expect("fail the task");
        let swept = sweep_org(&fx.t.db, Utc::now(), fx.t.org_id)
            .await
            .expect("sweep");
        assert_eq!(swept.dead, 0, "already dead: nothing to sync");
        let after = fx.reload(&delivery).await;
        assert_eq!(after.last_error, dead.last_error);
        assert_eq!(after.updated_at, dead.updated_at);
    })
    .await;
}

/// A delivery leaves `pending` once: a late finish (a worker the queue gave
/// up on) closes its attempt but does not revive or re-settle it.
#[tokio::test]
async fn a_settled_delivery_does_not_move_again() {
    with_fx(|fx| async move {
        let listener = fx.listener(NON_RETRYABLE_JS, json!({})).await;
        let delivery = fx.deliver("statement:1", &listener).await;
        // A zombie opens an attempt, the next worker's run kills the delivery.
        assert!(matches!(
            begin(&fx.t.db, delivery.id).await.expect("begin"),
            DeliveryStart::Run { .. }
        ));
        completed(fx.run(&delivery).await);
        let dead = fx.reload(&delivery).await;
        assert_eq!(dead.status, "dead");

        // The zombie finishes its attempt after all.
        finish_ok(&fx.t.db, delivery.id, 1, None)
            .await
            .expect("finish");
        let after = fx.reload(&delivery).await;
        assert_eq!(after.status, "dead");
        assert_eq!(after.last_error, dead.last_error);
        assert_eq!(after.attempts, 2);
        assert_eq!(
            outcomes(&fx.attempts(&delivery).await),
            vec![(1, Some("crash")), (2, Some("non_retryable"))],
            "the zombie's ok lands on neither attempt"
        );

        match begin(&fx.t.db, delivery.id).await.expect("begin") {
            DeliveryStart::Skip { reason } => assert!(reason.contains("dead"), "{reason}"),
            other => panic!("a dead delivery is skipped: {other:?}"),
        }
    })
    .await;
}

/// The queue reclaims a task whose worker is still running: worker A opens
/// attempt 1 and stops heartbeating, worker B's run begins (closing A's
/// attempt as a crash and opening attempt 2), then A finishes before B. A's
/// late outcome lands on no attempt and leaves the delivery to B.
#[tokio::test]
async fn a_reclaimed_runs_late_finish_records_nothing() {
    with_fx(|fx| async move {
        let listener = fx.listener(ECHO_JS, json!({})).await;
        let delivery = fx.deliver("statement:1", &listener).await;
        let opened = |start: DeliveryStart| match start {
            DeliveryStart::Run { info, .. } => info.attempt,
            other => panic!("a pending delivery runs: {other:?}"),
        };

        // Step 1: worker A begins; the task is reclaimed and worker B begins.
        let a = opened(begin(&fx.t.db, delivery.id).await.expect("A begins"));
        let b = opened(begin(&fx.t.db, delivery.id).await.expect("B begins"));
        assert_eq!((a, b), (1, 2));

        // Step 2: A finishes late, then B finishes.
        let (a_run, b_run) = (Uuid::new_v4(), Uuid::new_v4());
        finish_err(
            &fx.t.db,
            delivery.id,
            a,
            Some(a_run),
            AttemptOutcome::Error,
            "A's late failure",
        )
        .await
        .expect("A finishes");
        let tries = fx.attempts(&delivery).await;
        assert_eq!(
            tries[0].outcome.as_deref(),
            Some("crash"),
            "A's attempt stays B's verdict"
        );
        assert_eq!(tries[0].error.as_deref(), Some(CRASHED));
        assert_eq!(tries[0].invocation_id, None);
        assert_eq!(
            (
                tries[1].finished_at,
                tries[1].outcome.as_deref(),
                tries[1].invocation_id
            ),
            (None, None, None),
            "B's attempt is untouched by A"
        );
        let between = fx.reload(&delivery).await;
        assert_eq!(between.status, "pending");
        assert_eq!(between.last_error.as_deref(), Some(CRASHED));
        assert_eq!(between.invocation_id, None);

        finish_ok(&fx.t.db, delivery.id, b, Some(b_run))
            .await
            .expect("B finishes");
        let tries = fx.attempts(&delivery).await;
        assert_eq!(tries[1].outcome.as_deref(), Some("ok"));
        assert_eq!(tries[1].invocation_id, Some(b_run));
        let acked = fx.reload(&delivery).await;
        assert_eq!((acked.status.as_str(), acked.attempts), ("acked", 2));
        assert_eq!(acked.invocation_id, Some(b_run));
    })
    .await;
}

/// The documented trade-off: a reclaimed last attempt kills the delivery,
/// and the old run's success then cannot ack it.
#[tokio::test]
async fn a_reclaimed_last_attempt_is_dead_even_if_its_old_run_succeeds() {
    with_fx(|fx| async move {
        let listener = fx
            .listener(ECHO_JS, json!({ "retries": { "maxAttempts": 1 } }))
            .await;
        let delivery = fx.deliver("statement:1", &listener).await;
        assert!(matches!(
            begin(&fx.t.db, delivery.id).await.expect("A begins"),
            DeliveryStart::Run { .. }
        ));

        // The task is reclaimed: B's run finds the only attempt spent.
        let answer = completed(fx.run(&delivery).await);
        assert!(answer.starts_with("dead:"), "{answer}");
        assert!(
            invocations(&fx.t.db, listener.app_id, FUNCTION)
                .await
                .is_empty(),
            "B does not run the listener"
        );
        finish_ok(&fx.t.db, delivery.id, 1, Some(Uuid::new_v4()))
            .await
            .expect("A finishes");

        let dead = fx.reload(&delivery).await;
        assert_eq!((dead.status.as_str(), dead.attempts), ("dead", 1));
        let reason = dead.last_error.expect("the reason");
        assert!(reason.contains("did not finish"), "{reason}");
        let tries = fx.attempts(&delivery).await;
        assert_eq!(tries.len(), 1);
        assert_eq!(tries[0].outcome.as_deref(), Some("crash"));
    })
    .await;
}

/// Retention deletes an event's body from the org's store before its
/// metadata; a delivery that then runs finds the body gone and is dead,
/// without running the listener or spending a retry.
#[tokio::test]
async fn a_body_retention_deleted_is_a_dead_delivery() {
    with_fx(|fx| async move {
        let listener = fx.listener(ECHO_JS, json!({})).await;
        let delivery = fx.deliver("statement:1", &listener).await;
        let event = fx.event(&delivery).await;
        let key = OutboxKey {
            writer_role: event.writer_role.clone(),
            id: event.id,
        };
        let deleted =
            outbox::delete_bodies(&fx.t.db, fx.t.org_id, &OutboxTarget::Production, &[key])
                .await
                .expect("delete the body");
        assert_eq!(deleted, 1, "the body was in the org's store");

        let answer = completed(fx.run(&delivery).await);
        assert_eq!(answer, format!("dead: {BODY_GONE}"));

        assert!(
            invocations(&fx.t.db, listener.app_id, FUNCTION)
                .await
                .is_empty(),
            "the listener does not run"
        );
        let dead = fx.reload(&delivery).await;
        assert_eq!(dead.status, "dead");
        assert_eq!(dead.last_error.as_deref(), Some(BODY_GONE));
        let tries = fx.attempts(&delivery).await;
        assert_eq!(outcomes(&tries), vec![(1, Some("non_retryable"))]);
        assert_eq!(tries[0].error.as_deref(), Some(BODY_GONE));
    })
    .await;
}

/// A body Rust cannot hold (a number past `f64`), written by the store's
/// owner where no app can: no attempt will ever read it, so the delivery is
/// dead at once, its reason naming the event and never the body.
#[tokio::test]
async fn an_undecodable_body_is_dead_and_never_retried() {
    with_fx(|fx| async move {
        let listener = fx.listener(ECHO_JS, json!({})).await;
        let delivery = fx.deliver("statement:1", &listener).await;
        let rewritten = fx
            .as_owner(
                "UPDATE oxy_events.outbox SET data = '{\"n\": 1e400}'::jsonb WHERE id = $1",
                delivery.event_id,
            )
            .await;
        assert_eq!(rewritten, 1);

        let answer = completed(fx.run(&delivery).await);
        assert!(answer.starts_with("dead: "), "no retry: {answer}");

        assert!(
            invocations(&fx.t.db, listener.app_id, FUNCTION)
                .await
                .is_empty(),
            "the listener does not run"
        );
        let dead = fx.reload(&delivery).await;
        assert_eq!(dead.status, "dead");
        let reason = dead.last_error.clone().expect("the reason");
        assert!(
            reason.contains(&delivery.event_id.to_string()),
            "names the event: {reason}"
        );
        assert!(reason.contains("cannot be decoded"), "{reason}");
        assert!(
            !reason.contains(&"0".repeat(20)),
            "never the body (1e400 as numeric text): {reason}"
        );
        let tries = fx.attempts(&delivery).await;
        assert_eq!(outcomes(&tries), vec![(1, Some("non_retryable"))]);
        assert_eq!(tries[0].error, dead.last_error);

        // The queue's retry, if it runs anyway, finds it dead.
        assert!(completed(fx.run(&delivery).await).starts_with("skipped: "));
    })
    .await;
}

/// The org's store cannot be reached: the attempt is an error and the task
/// fails, so the queue retries; once it can be reached the retry delivers.
#[tokio::test]
async fn an_unreachable_store_fails_the_attempt_for_the_queue_to_retry() {
    with_fx(|fx| async move {
        let listener = fx.listener(ECHO_JS, json!({})).await;
        let delivery = fx.deliver("statement:1", &listener).await;

        let host = fx.set_store_host("127.0.0.1:1").await;
        let outcome = fx.run(&delivery).await;
        fx.set_store_host(&host).await;
        let error = failed(outcome);
        assert!(error.contains("could not read the event's body"), "{error}");

        assert!(
            invocations(&fx.t.db, listener.app_id, FUNCTION)
                .await
                .is_empty(),
            "the listener does not run"
        );
        let pending = fx.reload(&delivery).await;
        assert_eq!(pending.status, "pending", "the queue retries it");
        let tries = fx.attempts(&delivery).await;
        assert_eq!(outcomes(&tries), vec![(1, Some("error"))]);
        let recorded = tries[0].error.clone().expect("the error");
        assert!(
            recorded.contains("could not read the event's body"),
            "{recorded}"
        );

        assert_eq!(completed(fx.run(&delivery).await), "delivered (HTTP 200)");
        assert_eq!(fx.reload(&delivery).await.status, "acked");
    })
    .await;
}

/// What an attempt and its delivery store as an error is capped.
#[tokio::test]
async fn a_long_error_is_stored_capped() {
    with_fx(|fx| async move {
        let listener = fx.listener(LONG_ERROR_JS, json!({})).await;
        let delivery = fx.deliver("statement:1", &listener).await;

        failed(fx.run(&delivery).await);

        let recorded = fx.attempts(&delivery).await[0]
            .error
            .clone()
            .expect("the error");
        assert_eq!(recorded.chars().count(), MAX_ERROR_CHARS, "{recorded}");
        assert!(recorded.ends_with('…'), "{recorded}");
        assert!(recorded.contains("too long"), "{recorded}");
        assert_eq!(
            fx.reload(&delivery).await.last_error,
            Some(recorded),
            "the delivery's last error is the same capped text"
        );
    })
    .await;
}

/// A worker opens an attempt and dies; the queue then gives the task up.
/// Retention's dead sync kills the delivery and closes the attempt as a
/// `crash`, as the next `begin` would have, so no attempt stays open.
#[tokio::test]
async fn retention_closes_an_open_attempt_as_a_crash() {
    with_fx(|fx| async move {
        let listener = fx.listener(ECHO_JS, json!({})).await;
        let delivery = fx.deliver("statement:1", &listener).await;
        match begin(&fx.t.db, delivery.id).await.expect("begin") {
            DeliveryStart::Run { info, .. } => assert_eq!(info.attempt, 1),
            other => panic!("a pending delivery runs: {other:?}"),
        }
        fx.t.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE agentic_task_queue SET queue_status = 'dead' WHERE task_id = $1",
                [delivery.run_id.clone().into()],
            ))
            .await
            .expect("the queue gives the task up");

        let swept = sweep_org(&fx.t.db, Utc::now(), fx.t.org_id)
            .await
            .expect("sweep");

        assert_eq!(swept.dead, 1);
        assert_eq!(fx.reload(&delivery).await.status, "dead");
        let tries = fx.attempts(&delivery).await;
        assert_eq!(outcomes(&tries), vec![(1, Some("crash"))]);
        assert!(tries[0].finished_at.is_some(), "closed: {tries:?}");
        assert_eq!(tries[0].error.as_deref(), Some(CRASHED));

        // The worker was not dead after all, and finishes its attempt: the
        // crash retention recorded stands, and the delivery stays dead.
        let dead = fx.reload(&delivery).await;
        finish_ok(&fx.t.db, delivery.id, 1, None)
            .await
            .expect("the late finish");
        let after = fx.attempts(&delivery).await;
        assert_eq!(outcomes(&after), vec![(1, Some("crash"))]);
        assert_eq!(after[0].error.as_deref(), Some(CRASHED));
        assert_eq!(after[0].finished_at, tries[0].finished_at);
        let still = fx.reload(&delivery).await;
        assert_eq!(still.status, "dead");
        assert_eq!(still.last_error, dead.last_error);
        assert_eq!(still.updated_at, dead.updated_at);
    })
    .await;
}

/// The last attempt's body read fails (the org's store cannot be reached):
/// nothing follows it, so the delivery is dead with that error rather than
/// left pending with nothing queued.
#[tokio::test]
async fn an_unreachable_store_on_the_last_attempt_is_dead() {
    with_fx(|fx| async move {
        let listener = fx
            .listener(ECHO_JS, json!({ "retries": { "maxAttempts": 1 } }))
            .await;
        let delivery = fx.deliver("statement:1", &listener).await;

        let host = fx.set_store_host("127.0.0.1:1").await;
        let outcome = fx.run(&delivery).await;
        fx.set_store_host(&host).await;
        let error = failed(outcome);
        assert!(error.contains("could not read the event's body"), "{error}");

        let dead = fx.reload(&delivery).await;
        assert_eq!((dead.status.as_str(), dead.attempts), ("dead", 1));
        let reason = dead.last_error.clone().expect("the reason");
        assert!(
            reason.contains("could not read the event's body"),
            "{reason}"
        );
        let tries = fx.attempts(&delivery).await;
        assert_eq!(outcomes(&tries), vec![(1, Some("error"))]);
        assert_eq!(tries[0].error, dead.last_error);
    })
    .await;
}

/// A delivery unfinished at 30 days expires; an attempt still open on it is
/// closed as a `crash`, as the next `begin` would have closed it.
#[tokio::test]
async fn expiry_closes_an_open_attempt_as_a_crash() {
    with_fx(|fx| async move {
        let listener = fx.listener(ECHO_JS, json!({})).await;
        let delivery = fx.deliver("statement:1", &listener).await;
        match begin(&fx.t.db, delivery.id).await.expect("begin") {
            DeliveryStart::Run { info, .. } => assert_eq!(info.attempt, 1),
            other => panic!("a pending delivery runs: {other:?}"),
        }
        fx.t.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE app_event_deliveries SET created_at = now() - interval '31 days' \
                 WHERE id = $1",
                [delivery.id.into()],
            ))
            .await
            .expect("age the delivery");

        let swept = sweep_org(&fx.t.db, Utc::now(), fx.t.org_id)
            .await
            .expect("sweep");

        assert_eq!((swept.dead, swept.expired), (0, 1));
        assert_eq!(fx.reload(&delivery).await.status, "expired");
        let tries = fx.attempts(&delivery).await;
        assert_eq!(outcomes(&tries), vec![(1, Some("crash"))]);
        assert!(tries[0].finished_at.is_some(), "closed: {tries:?}");
        assert_eq!(tries[0].error.as_deref(), Some(CRASHED));
    })
    .await;
}

/// The listener answers with the event's body (attempt 1 as a 500, attempt 2
/// as a 200): neither answer is kept anywhere in Oxy's Postgres. The run's
/// answer is a summary, the invocation row keeps no result, and a failed
/// answer is recorded by its status alone.
#[tokio::test]
async fn a_listeners_answer_is_never_stored() {
    with_fx(|fx| async move {
        let listener = fx.listener(ECHO_TWICE_JS, json!({})).await;
        let marker = format!("only-in-the-org-store-{}", Uuid::new_v4().simple());
        let delivery = fx
            .deliver_data(
                "statement:1",
                None,
                &json!({ "period": "2026-11", "note": marker }),
                &listener,
            )
            .await;

        let error = failed(fx.run(&delivery).await);
        assert_eq!(error, "the listener answered HTTP 500");
        let answer = completed(fx.run(&delivery).await);
        assert_eq!(answer, "delivered (HTTP 200)");

        let tries = fx.attempts(&delivery).await;
        assert_eq!(
            tries[0].error.as_deref(),
            Some("the listener answered HTTP 500"),
            "a failed answer is recorded by its status alone"
        );
        let runs = invocations(&fx.t.db, listener.app_id, FUNCTION).await;
        assert_eq!(runs.len(), 2, "the listener ran twice");
        assert!(
            runs.iter().all(|r| r.result_body.is_none()),
            "no invocation keeps the answer: {runs:?}"
        );
        assert_eq!(
            fx.platform_rows_holding(&marker).await,
            Vec::<(String, i64)>::new(),
            "no platform row holds the listener's answer"
        );
    })
    .await;
}

/// A listener throws an error carrying the event's body, far longer than an
/// error keeps. Every stored copy of it is capped, the invocation row's
/// included, so the part of the body past the cap (its marker, after the
/// filler) is in no platform row.
#[tokio::test]
async fn a_thrown_error_carrying_the_body_is_stored_capped() {
    with_fx(|fx| async move {
        let listener = fx.listener(THROW_BODY_JS, json!({})).await;
        let tail = format!("past-the-cap-{}", Uuid::new_v4().simple());
        // The filler's key sorts first whichever order the body comes back in
        // (jsonb's, shortest key first, or alphabetical): `JSON.stringify`
        // writes it before the marker's.
        let data = json!({ "a": "x".repeat(3000), "period": "2026-11", "past_the_cap": tail });
        let delivery = fx.deliver_data("statement:1", None, &data, &listener).await;

        let error = failed(fx.run(&delivery).await);
        assert!(error.chars().count() <= MAX_ERROR_CHARS, "{error}");
        assert!(error.contains("listener saw: "), "{error}");

        let runs = invocations(&fx.t.db, listener.app_id, FUNCTION).await;
        assert_eq!(runs.len(), 1);
        let stored = runs[0].error.clone().expect("the invocation's error");
        assert_eq!(stored.chars().count(), MAX_ERROR_CHARS, "{stored}");
        assert!(stored.ends_with('…'), "{stored}");
        assert!(stored.contains("listener saw: "), "{stored}");
        assert_eq!(
            fx.platform_rows_holding(&tail).await,
            Vec::<(String, i64)>::new(),
            "no platform row holds the body past the cap"
        );
    })
    .await;
}
