//! Publishing an app event from an Oxy Function (app events design §6.2,
//! §6.4): `ctx.events.publish` writes one row into the org's outbox in its own
//! transaction and queues the relay; `tx.events.publish` writes it on the app's
//! open `ctx.oltp.tx` handle, so it commits or rolls back with the app's own
//! writes, and the relay is queued only once that transaction commits. Every
//! refusal is decided before anything is written.
//!
//! Against real stores, as `custom_app_functions_shape_zoo_oltp` does: the
//! org's OLTP tenant on the control plane's cluster (`AppStore`), the app
//! published through the real `publish` with its `events` catalogue, and a
//! function called through the serve route, so `bootstrap.js`, the broker and
//! the host all run. The org flag `app_events` is turned on with an org
//! override row, loaded into the flag cache as `serve` loads it at boot.
//!
//! Two things no route carries today are driven on a `ProjectFunctionHost`
//! directly: the event an invocation handles (its cause, wired by event
//! delivery), and the moment between a publish and its transaction's commit.

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::Utc;
use futures::FutureExt as _;
use oxy_api_custom_apps::app_events::marks;
use oxy_api_custom_apps::app_events::relay::{RELAY_KIND, RelayStats, relay_once};
use oxy_api_custom_apps::app_events::schema::live_revision;
use oxy_api_custom_apps::custom_apps_functions::InvocationIdentity;
use oxy_api_custom_apps::custom_apps_functions::env_policy::EnvPolicy;
use oxy_api_custom_apps::custom_apps_functions::host::{
    FunctionCapabilities, ProjectFunctionHost, WriterCapability, into_arc,
};
use oxy_api_custom_apps::custom_apps_functions::runtime::{EventCause, FunctionHost};
use oxy_api_custom_apps::custom_apps_functions::seam::FunctionQueryExecutor;
use oxy_api_custom_apps::custom_apps_publish::{OrgRef, PublishInput, publish};
use oxy_api_custom_apps::data_plane::query::DataPlaneQueryExecutor;
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use oxy_custom_apps::events::catalog::EventCatalog;
use oxy_custom_apps::events::{event_id, jsonb_text_len, qualified_type};
use oxy_oltp::entity::tenants::{self as oltp_tenants, Entity as OltpTenants};
use oxy_oltp::events::{self as outbox, OutboxRow, OutboxTarget};
use oxy_oltp::resolver::resolve_writer_connection_for_org;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait,
    QueryFilter, Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::custom_app_functions_fixture::{
    FnCall, FunctionSpec, Tenant, call_function_with, invocations, seeded_tenant, throwaway_org,
};
use crate::custom_app_functions_shape_zoo::{compile, write_config};
use crate::custom_app_functions_shape_zoo_oltp::AppStore;
use crate::staging_functions::{held_ops, held_rows, make_guest_staff, staging_host};
use crate::staging_homes_fixture::{Workspace, duck, workspace};

/// Declared in the app's catalogue and in the function's `events.publish`.
const EVENT: &str = "order.placed";
/// In the function's `events.publish`, but not in the app's catalogue.
const UNDECLARED: &str = "order.ghost";
/// The one function the app declares.
const DRIVE: &str = "drive";

/// Runs each step of the request body and answers what each returned or
/// threw, so one call tries many publishes and every refusal is read in the
/// host's words.
const DRIVE_JS: &str = r#"
const RUN = {
  publish: (ctx, s) => ctx.events.publish(s.type, s.data, s.opts),
  txPublish: (ctx, s) => ctx.oltp.tx(async (tx) => {
    if (s.note !== undefined) await tx.exec("insert into notes (id) values ($1)", [s.note]);
    const published = await tx.events.publish(s.type, s.data, s.opts);
    if (s.throwAfter) throw new Error("the handler threw after publishing");
    return published;
  }),
  appsSend: (ctx, s) => ctx.apps.send(s.app, s.fn, s.body, s.opts),
  oltpExec: (ctx, s) => ctx.oltp.exec(s.sql),
  oltpQuery: (ctx, s) => ctx.oltp.query(s.sql),
  txExec: (ctx, s) => ctx.oltp.tx((tx) => tx.exec(s.sql)),
};

export default async (req, ctx) => {
  const { steps } = JSON.parse(req.body);
  const out = [];
  for (const step of steps) {
    try {
      out.push({ ok: await RUN[step.kind](ctx, step) });
    } catch (e) {
      out.push({ error: String(e && e.message ? e.message : e) });
    }
  }
  return Response.json({ out, channel: ctx.channel });
};
"#;

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": { "orderId": { "type": "integer" } },
        "required": ["orderId"],
    })
}

/// The app's `events` catalogue: [`EVENT`] only.
fn catalog() -> Value {
    json!({
        "events": { EVENT: { "schema": schema() } },
        "calls": { CALLEE: ["handle"] },
    })
}

/// The app the driver sends to: callable, so only the environment decides.
const CALLEE: &str = "send-callee";

fn drive() -> FunctionSpec {
    FunctionSpec {
        name: DRIVE,
        manifest: json!({
            "route": true,
            "timeoutSeconds": 60,
            "oltp": { "enabled": true },
            "events": { "publish": [EVENT, UNDECLARED] },
        }),
        js: DRIVE_JS,
    }
}

fn order(id: i64) -> Value {
    json!({ "orderId": id })
}

/// A `publish` step: `ctx.events.publish(type, data, opts)`.
fn publish_step(event: &str, data: Value, opts: Value) -> Value {
    json!({ "kind": "publish", "type": event, "data": data, "opts": opts })
}

/// What one step answered: `Ok` with what it returned, `Err` with what it threw.
fn outcome(step: &Value) -> Result<&Value, String> {
    match (step.get("ok"), step.get("error")) {
        (Some(ok), None) => Ok(ok),
        (None, Some(Value::String(e))) => Err(e.clone()),
        other => panic!("a step answers ok or error, got {other:?}"),
    }
}

/// `error` is `message`, alone or under an op's label.
fn assert_is(error: &str, message: &str) {
    assert!(
        error == message || error.ends_with(&format!(": {message}")),
        "{error}"
    );
}

/// The host's message under the op's label (`ctx.events: `, `ctx.tx: `).
fn host_message(error: &str) -> &str {
    error.split_once(": ").map_or(error, |(_, rest)| rest)
}

/// An org with a workspace and the app, which declares [`EVENT`] and whose
/// one function publishes it.
struct Publisher {
    t: Tenant,
    slug: String,
    app_id: Uuid,
    workspace: Uuid,
    store: AppStore,
    _root: tempfile::TempDir,
}

impl Publisher {
    /// The org's OLTP store provisioned (with a `notes` table in the app's
    /// schema), app events on for the org.
    async fn new() -> Self {
        let p = Self::bare().await;
        p.provision().await;
        enable_app_events(&p.t, &[p.org()]).await;
        p
    }

    /// The org, its workspace and the published app — no OLTP store, and the
    /// flag cache not loaded.
    async fn bare() -> Self {
        let t = throwaway_org(&seeded_tenant().await).await;
        let slug = format!("evt-pub-{}", &Uuid::new_v4().simple().to_string()[..8]);
        let root = write_config("  - name: duck\n    type: duckdb\n    path: events.duckdb\n");
        let workspace = compile(&t, root.path()).await;
        let store = AppStore::new(&t, &slug).await;
        let app_id = publish_app(&t, &slug, workspace).await;
        crate::app_events_delivery::publish_app(
            &t,
            CALLEE,
            workspace,
            "1",
            &[FunctionSpec {
                name: "handle",
                manifest: json!({ "callable": true }),
                js: "export default async () => Response.json({ ok: true });",
            }],
            json!({}),
        )
        .await;
        Self {
            t,
            slug,
            app_id,
            workspace,
            store,
            _root: root,
        }
    }

    async fn provision(&self) {
        self.store.provision(self.workspace).await;
        self.writer_exec("create table notes (id int primary key)")
            .await;
    }

    fn org(&self) -> Uuid {
        self.t.org_id
    }

    fn event_type(&self) -> String {
        qualified_type(&self.slug, EVENT)
    }

    /// The id an honest publish of [`EVENT`] under `key` in `environment` has.
    fn id_of(&self, environment: &str, key: &str) -> Uuid {
        event_id(self.org(), environment, &self.event_type(), key)
    }

    /// The app's writer connection on production's store.
    async fn writer(&self) -> oxy_oltp::resolver::WriterConnection {
        resolve_writer_connection_for_org(&self.t.db, self.org(), &self.store.writer)
            .await
            .expect("resolve the app's writer")
    }

    async fn writer_exec(&self, sql: &str) {
        connect(&self.writer().await.dsn)
            .await
            .batch_execute(sql)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }

    /// The `notes` rows in production's store, as the writer reads them.
    async fn notes(&self) -> Vec<i32> {
        connect(&self.writer().await.dsn)
            .await
            .query("select id from notes order by id", &[])
            .await
            .expect("read notes")
            .iter()
            .map(|r| r.get(0))
            .collect()
    }

    async fn tenant(&self) -> oltp_tenants::Model {
        OltpTenants::find()
            .filter(oltp_tenants::Column::OrgId.eq(self.org()))
            .one(&self.t.db)
            .await
            .expect("read the tenant")
            .expect("the org's tenant")
    }

    /// The org's `environment` outbox mark.
    async fn mark(&self, environment: &str) -> Option<entity::app_event_outbox_marks::Model> {
        entity::app_event_outbox_marks::Entity::find_by_id((self.org(), environment.to_string()))
            .one(&self.t.db)
            .await
            .expect("read the mark")
    }

    /// Every row in production's outbox, as the relay claims them; released.
    async fn outbox(&self) -> Vec<OutboxRow> {
        let claim = outbox::claim(&self.t.db, self.org(), &OutboxTarget::Production, 100)
            .await
            .expect("claim the outbox");
        let rows = claim.rows().to_vec();
        claim.release().await.expect("release the claim");
        rows
    }

    /// The `(task_id, environment)` of every relay waiting in the queue.
    async fn relay_tasks(&self) -> Vec<(String, String)> {
        self.t
            .db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                format!(
                    "SELECT task_id, spec->'payload'->>'environment' AS environment \
                       FROM agentic_task_queue \
                      WHERE spec->>'kind' = '{RELAY_KIND}' AND queue_status = 'queued'"
                ),
            ))
            .await
            .expect("read the queue")
            .iter()
            .map(|row| {
                (
                    row.try_get("", "task_id").expect("task_id"),
                    row.try_get("", "environment").expect("environment"),
                )
            })
            .collect()
    }

    /// The revision production's build declares for [`EVENT`].
    async fn revision(&self) -> Uuid {
        live_revision(&self.t.db, self.app_id, "production", EVENT)
            .await
            .expect("read the live revision")
            .expect("production's build declares the event")
            .id
    }

    /// Call [`DRIVE`] on production with `steps`; one outcome per step.
    async fn drive(&self, steps: Vec<Value>) -> Vec<Value> {
        self.drive_on(None, steps).await.0
    }

    /// [`Self::drive`] addressed to `host` (a staging host, say), with the
    /// channel the function ran in.
    async fn drive_on(&self, host: Option<&str>, steps: Vec<Value>) -> (Vec<Value>, String) {
        let headers: Vec<(&str, &str)> = host.map(|h| ("host", h)).into_iter().collect();
        let call: FnCall = call_function_with(
            &self.t.org_slug,
            &self.slug,
            DRIVE,
            json!({ "steps": steps }),
            &headers,
        )
        .await;
        let data = call
            .frame("data")
            .unwrap_or_else(|| panic!("no data frame ({}): {}", call.status, call.raw));
        let out = data["out"].as_array().expect("out").clone();
        (
            out,
            data["channel"].as_str().unwrap_or_default().to_string(),
        )
    }

    /// The id of the one invocation of [`DRIVE`].
    async fn invocation(&self) -> Uuid {
        let rows = invocations(&self.t.db, self.app_id, DRIVE).await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        rows[0].id
    }

    /// What the app's build grants [`DRIVE`], as `manifest_capabilities` reads
    /// it from the published manifest.
    fn caps(&self) -> FunctionCapabilities {
        FunctionCapabilities {
            oltp: WriterCapability::resolve(true, &self.slug),
            events: Some(BTreeSet::from([EVENT.to_string(), UNDECLARED.to_string()])),
            events_writer: WriterCapability::resolve(true, &self.slug),
            event_catalog: EventCatalog::from_manifest(Some(&catalog())).expect("the catalogue"),
            ..Default::default()
        }
    }

    /// A host for one invocation of [`DRIVE`] in production, handling `cause`
    /// when it is an event's delivery. `project` only has to outlive it.
    fn host(
        &self,
        project: &Workspace,
        invocation: Uuid,
        cause: Option<EventCause>,
    ) -> Arc<dyn FunctionHost> {
        into_arc(
            ProjectFunctionHost::new(
                project.ctx(),
                Arc::new(DataPlaneQueryExecutor) as Arc<dyn FunctionQueryExecutor>,
                self.t.db.clone(),
                Vec::new(),
                self.workspace,
                self.app_id,
                self.org(),
                self.t.guest_id,
                "Events".into(),
                self.caps(),
                Default::default(),
                oxy_app_core::operating_graph::reach::Reach::nowhere(),
                InvocationIdentity {
                    invocation_id: invocation,
                    function_name: DRIVE.into(),
                    mode: "route".into(),
                    request_id: None,
                    app_slug: self.slug.clone(),
                    user_id: Some(self.t.guest_id),
                    user_email: Some(LOCAL_GUEST_EMAIL.into()),
                    credential_token_id: None,
                },
                EnvPolicy::production(),
            )
            .with_event_cause(cause),
        )
    }
}

/// Publish [`DRIVE`] as `slug` in `workspace`, promoted, with the catalogue.
async fn publish_app(t: &Tenant, slug: &str, workspace: Uuid) -> Uuid {
    publish(PublishInput {
        org_ref: Some(OrgRef::Id(t.org_id)),
        app_slug: slug.to_string(),
        project_id: workspace,
        branch: None,
        build_id: format!("{slug}-1"),
        name: None,
        promote: true,
        tarball: crate::sandbox_publish::bundle(slug, &[drive()], catalog(), &[]),
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

/// Turn `app_events` on for `orgs` alone, as an admin's org override does,
/// and load the flag cache from the table.
pub(crate) async fn enable_app_events(t: &Tenant, orgs: &[Uuid]) {
    let overrides: serde_json::Map<String, Value> = orgs
        .iter()
        .map(|org| (org.to_string(), json!("on")))
        .collect();
    entity::feature_flag::ActiveModel {
        key: Set(oxy_feature_flags::APP_EVENTS.to_string()),
        // The registry default; the kill switch is in the document.
        enabled: Set(false),
        updated_at: Set(Utc::now().fixed_offset()),
        targeting: Set(Some(json!({
            "enabled": true,
            "orgs": overrides,
            "fallthrough": { "type": "variant", "variant": "off" },
        }))),
    }
    .insert(&t.db)
    .await
    .expect("write the app_events override");
    oxy_feature_flags::cache::load_once(crate::oltp_changes_fixture::flag_db(&t.db))
        .await
        .expect("load the flag cache");
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

/// `url` with its database replaced by `database`.
fn on_database(url: &str, database: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (url, None),
    };
    let server = base.rsplit_once('/').map_or(base, |(server, _)| server);
    match query {
        Some(query) => format!("{server}/{database}?{query}"),
        None => format!("{server}/{database}"),
    }
}

/// Run `body` against a fresh [`Publisher`] from `make`, dropping the org's
/// store (database, then roles) whatever happened.
async fn with_publisher<M, MF, F, Fut>(make: M, body: F)
where
    M: FnOnce() -> MF,
    MF: std::future::Future<Output = Publisher>,
    F: FnOnce(Arc<Publisher>) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let p = Arc::new(make().await);
    // Boxed: a body that calls a function and reads two stores outgrows the
    // test thread's stack when pinned on it.
    let outcome = std::panic::AssertUnwindSafe(Box::pin(body(p.clone())))
        .catch_unwind()
        .await;
    p.store.cleanup().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn publish_outside_a_tx_writes_one_outbox_row_and_queues_a_relay() {
    with_publisher(Publisher::new, |p| async move {
        let out = p
            .drive(vec![publish_step(
                EVENT,
                order(7),
                json!({ "key": "order-7", "delaySeconds": 30 }),
            )])
            .await;
        let id = p.id_of("production", "order-7");
        let reply = outcome(&out[0]).expect("the publish succeeds");
        assert_eq!(reply, &json!({ "eventId": id.to_string() }));

        let rows = p.outbox().await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        let row = &rows[0];
        assert_eq!(row.id, id);
        assert_eq!(row.writer_role, p.writer().await.role);
        assert_eq!(row.environment, "production");
        assert_eq!(row.name, EVENT);
        assert_eq!(row.key, "order-7");
        // The body is the org's, read from its store by the row's key, and
        // measured there as the host measured it.
        let body = outbox::read_body(
            &p.t.db,
            p.org(),
            &OutboxTarget::Production,
            &row.writer_role,
            row.id,
        )
        .await
        .expect("read the body");
        assert_eq!(body, Some(order(7)));
        assert_eq!(row.data_bytes as usize, jsonb_text_len(&order(7)));
        assert_eq!(
            outbox::read_body(
                &p.t.db,
                p.org(),
                &OutboxTarget::Production,
                &row.writer_role,
                Uuid::new_v4(),
            )
            .await
            .expect("read a body that is not there"),
            None
        );
        assert_eq!(row.schema_revision, Some(p.revision().await));
        assert_eq!(row.delay_seconds, 30);
        assert_eq!(row.depth, 0);
        assert_eq!(row.causation_id, None);

        let relays = p.relay_tasks().await;
        assert_eq!(relays.len(), 1, "{relays:?}");
        assert_eq!(relays[0].1, "production");
        assert!(
            relays[0]
                .0
                .starts_with(&format!("{RELAY_KIND}:{}:production:", p.org())),
            "{relays:?}"
        );
    })
    .await;
}

/// R25(c): the host signs every row it writes — outside a transaction and on
/// a `ctx.oltp.tx` handle, a handler's chained publish included — so each
/// relays as before: stored without a relay error, none quarantined.
#[tokio::test]
async fn a_host_publish_is_signed_and_relays() {
    with_publisher(Publisher::new, |p| async move {
        let out = p
            .drive(vec![
                publish_step(EVENT, order(1), json!({ "key": "outside" })),
                json!({
                    "kind": "txPublish",
                    "note": 2,
                    "type": EVENT,
                    "data": order(2),
                    "opts": { "key": "on-handle", "delaySeconds": 5 },
                }),
            ])
            .await;
        for step in &out {
            outcome(step).expect("the publish succeeds");
        }
        // A publish made while handling an event: the cause's chain is the
        // host's, and signed with it.
        let project = workspace(&duck("events")).await;
        let cause = EventCause {
            id: Uuid::new_v4(),
            event_type: format!("orders/{EVENT}"),
            actor: Some("user-9".into()),
            root_id: Uuid::new_v4(),
            depth: 3,
        };
        let host = p.host(&project, Uuid::new_v4(), Some(cause));
        host.events(
            "publish".into(),
            json!({ "type": EVENT, "data": order(3), "key": "chained" }),
        )
        .await
        .expect("the chained publish");
        let rows = p.outbox().await;
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert!(
            rows.iter()
                .all(|r| r.provenance.as_ref().is_some_and(|m| m.len() == 32)),
            "every row carries the host's MAC: {rows:?}"
        );

        let stats = relay_once(&p.t.db, p.org(), "production")
            .await
            .expect("relay");
        assert_eq!(
            stats,
            RelayStats {
                relayed: 3,
                deliveries: 0,
                rejected: 0
            }
        );
        let events = entity::app_events::Entity::find()
            .filter(entity::app_events::Column::OrgId.eq(p.org()))
            .all(&p.t.db)
            .await
            .expect("read the events");
        assert_eq!(events.len(), 3, "{events:?}");
        for event in &events {
            assert_eq!(event.relay_error, None, "{event:?}");
        }
    })
    .await;
}

/// The row is written on the handle's own connection: nothing is visible, and
/// no relay is queued, until the transaction commits — then the app's row,
/// the event and the relay all are.
#[tokio::test]
async fn publish_in_a_committed_tx_lands_with_the_apps_row() {
    with_publisher(Publisher::new, |p| async move {
        let project = workspace(&duck("events")).await;
        let host = p.host(&project, Uuid::new_v4(), None);
        let opened = host
            .tx("begin_oltp".into(), json!({}))
            .await
            .expect("begin");
        let tx = opened["id"].as_u64().expect("a handle id");
        host.tx(
            "exec".into(),
            json!({ "id": tx, "sql": "insert into notes (id) values ($1)", "params": [1] }),
        )
        .await
        .expect("the app's write");
        let published = host
            .tx(
                "publish".into(),
                json!({ "id": tx, "type": EVENT, "data": order(1), "key": "order-1" }),
            )
            .await
            .expect("the publish on the handle");
        let id = p.id_of("production", "order-1");
        assert_eq!(published, json!({ "eventId": id.to_string() }));

        assert!(
            p.outbox().await.is_empty(),
            "nothing is visible before commit"
        );
        assert!(p.notes().await.is_empty());
        assert!(
            p.relay_tasks().await.is_empty(),
            "the relay is queued only once the publish commits"
        );

        host.tx("commit".into(), json!({ "id": tx }))
            .await
            .expect("commit");

        let rows = p.outbox().await;
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [id]);
        assert_eq!(p.notes().await, [1]);
        let relays = p.relay_tasks().await;
        assert_eq!(relays.len(), 1, "{relays:?}");
        assert_eq!(relays[0].1, "production");
    })
    .await;
}

#[tokio::test]
async fn publish_in_a_thrown_tx_leaves_nothing() {
    with_publisher(Publisher::new, |p| async move {
        let out = p
            .drive(vec![json!({
                "kind": "txPublish",
                "note": 1,
                "type": EVENT,
                "data": order(1),
                "opts": { "key": "order-1" },
                "throwAfter": true,
            })])
            .await;
        let error = outcome(&out[0]).expect_err("the handler threw");
        assert!(error.contains("threw after publishing"), "{error}");
        assert!(p.notes().await.is_empty(), "no app row");
        assert!(p.outbox().await.is_empty(), "no outbox row");
        assert!(p.relay_tasks().await.is_empty(), "no relay task");
    })
    .await;
}

/// R27: every accepted publish marks the org's outbox in the platform, so the
/// relay sweep finds it without connecting to the org's store: outside a
/// transaction once the row has committed, and on a handle when the publish
/// is made, so even a publish whose transaction is thrown away leaves a
/// (harmless) mark.
#[tokio::test]
async fn a_publish_marks_the_orgs_outbox() {
    with_publisher(Publisher::new, |p| async move {
        assert_eq!(p.mark("production").await, None);
        let out = p
            .drive(vec![publish_step(
                EVENT,
                order(1),
                json!({ "key": "order-1" }),
            )])
            .await;
        outcome(&out[0]).expect("the publish succeeds");
        let first = p.mark("production").await.expect("the publish marked");
        assert_eq!(first.drained_at, None);

        let out = p
            .drive(vec![json!({
                "kind": "txPublish",
                "note": 2,
                "type": EVENT,
                "data": order(2),
                "opts": { "key": "order-2" },
                "throwAfter": true,
            })])
            .await;
        outcome(&out[0]).expect_err("the handler threw");
        let second = p.mark("production").await.expect("still marked");
        assert!(
            second.published_at > first.published_at,
            "the handle's publish marked again: {first:?} {second:?}"
        );
    })
    .await;
}

/// R36: a handle's publish marks the outbox when it is made, and `tx_finish`
/// marks it again at commit: a relay pass that began between the two could
/// not see the uncommitted row and marked the outbox drained. After the
/// commit the mark is due again, published no earlier than the commit.
#[tokio::test]
async fn a_committed_handle_marks_the_outbox_again_at_commit() {
    with_publisher(Publisher::new, |p| async move {
        let project = workspace(&duck("events")).await;
        let host = p.host(&project, Uuid::new_v4(), None);
        let opened = host
            .tx("begin_oltp".into(), json!({}))
            .await
            .expect("begin");
        let tx = opened["id"].as_u64().expect("a handle id");
        host.tx(
            "publish".into(),
            json!({ "id": tx, "type": EVENT, "data": order(1), "key": "order-1" }),
        )
        .await
        .expect("the publish on the handle");
        // A pass that began after that mark, before the commit: it saw no
        // row, and drained the mark.
        let started = marks::platform_now(&p.t.db).await.expect("now");
        marks::mark_drained(&p.t.db, p.org(), "production", started)
            .await
            .expect("drain");
        let drained = p.mark("production").await.expect("the mark");
        assert!(
            drained.drained_at >= Some(drained.published_at),
            "{drained:?}"
        );

        let before_commit = marks::platform_now(&p.t.db).await.expect("now");
        host.tx("commit".into(), json!({ "id": tx }))
            .await
            .expect("commit");
        let mark = p.mark("production").await.expect("the mark");
        assert!(
            mark.published_at >= before_commit,
            "marked again at commit: {mark:?}, commit began {before_commit}"
        );
        assert!(
            Some(mark.published_at) > mark.drained_at,
            "due again: {mark:?}"
        );
    })
    .await;
}

/// R36: a payload that does not match the schema is refused naming where and
/// which keyword it failed — never the value, which is the app's data and
/// would reach its logs and run records.
#[tokio::test]
async fn a_schema_mismatch_names_the_field_never_its_value() {
    with_publisher(Publisher::new, |p| async move {
        let secret = json!({ "holder": "Ada Lovelace", "card": "4242 4242 4242 4242" });
        let out = p
            .drive(vec![
                publish_step(EVENT, json!({ "orderId": secret }), json!({ "key": "k" })),
                publish_step(
                    EVENT,
                    json!({ "note": "Ada Lovelace" }),
                    json!({ "key": "k" }),
                ),
            ])
            .await;
        let error = outcome(&out[0]).expect_err("orderId is an integer");
        assert_eq!(
            host_message(&error),
            "EventSchemaMismatch: /orderId fails `type`"
        );
        let error = outcome(&out[1]).expect_err("orderId is required");
        assert_eq!(
            host_message(&error),
            "EventSchemaMismatch: /orderId fails `required`"
        );
        for step in &out {
            let error = outcome(step).expect_err("refused");
            for part in ["Ada", "Lovelace", "4242", "holder", "card", "note"] {
                assert!(!error.contains(part), "{error}");
            }
        }
        assert!(p.outbox().await.is_empty());
    })
    .await;
}

/// The second publish of a key is `ON CONFLICT DO NOTHING`: the same id, one
/// row, and a transaction that repeats it still commits its own write.
#[tokio::test]
async fn the_same_key_twice_returns_one_event_id_and_one_row() {
    with_publisher(Publisher::new, |p| async move {
        let out = p
            .drive(vec![
                publish_step(EVENT, order(1), json!({ "key": "order-1" })),
                json!({
                    "kind": "txPublish",
                    "note": 5,
                    "type": EVENT,
                    "data": order(1),
                    "opts": { "key": "order-1" },
                }),
            ])
            .await;
        let id = json!({ "eventId": p.id_of("production", "order-1").to_string() });
        assert_eq!(outcome(&out[0]), Ok(&id));
        assert_eq!(outcome(&out[1]), Ok(&id));
        assert_eq!(p.outbox().await.len(), 1);
        assert_eq!(p.notes().await, [5], "the repeating transaction committed");
    })
    .await;
}

#[tokio::test]
async fn an_undeclared_name_is_refused() {
    with_publisher(Publisher::new, |p| async move {
        let out = p
            .drive(vec![
                publish_step(UNDECLARED, order(1), json!({ "key": "k" })),
                publish_step("order.shipped", order(1), json!({ "key": "k" })),
            ])
            .await;
        let error = outcome(&out[0]).expect_err("a name the app's catalogue lacks");
        assert_eq!(
            host_message(&error),
            "EventNotDeclared: `order.ghost` is not in this app's `events`"
        );
        let error = outcome(&out[1]).expect_err("a name the function does not publish");
        assert_eq!(
            host_message(&error),
            "EventNotPublishable: this function may publish [order.ghost, order.placed], not \
             `order.shipped`"
        );
        assert!(p.outbox().await.is_empty());
    })
    .await;
}

/// On for another org only: this org's publish is refused before anything.
#[tokio::test]
async fn publish_is_refused_when_the_org_flag_is_off() {
    let make = || async {
        let p = Publisher::bare().await;
        p.provision().await;
        enable_app_events(&p.t, &[Uuid::new_v4()]).await;
        p
    };
    with_publisher(make, |p| async move {
        let out = p
            .drive(vec![
                publish_step(EVENT, order(1), json!({ "key": "k" })),
                json!({
                    "kind": "txPublish",
                    "note": 1,
                    "type": EVENT,
                    "data": order(1),
                    "opts": { "key": "k" },
                }),
            ])
            .await;
        for step in &out {
            let error = outcome(step).expect_err("app events are off for this org");
            assert_eq!(
                host_message(&error),
                "EventsNotEnabled: app events are not enabled for this org"
            );
        }
        assert!(p.outbox().await.is_empty());
        assert!(
            p.notes().await.is_empty(),
            "the refused publish threw out of the tx"
        );
        assert!(p.relay_tasks().await.is_empty());
    })
    .await;
}

/// `é` is two bytes in UTF-8: the string's JSON is over the byte limit while
/// its length in characters is well under it.
#[tokio::test]
async fn a_multibyte_payload_over_the_byte_limit_is_refused() {
    with_publisher(Publisher::new, |p| async move {
        let text = "é".repeat(32_800);
        let bytes = serde_json::to_vec(&json!(text)).expect("serialise").len();
        assert!(bytes > 65_536 && text.chars().count() < 65_536, "{bytes}");
        let out = p
            .drive(vec![publish_step(
                EVENT,
                json!(text),
                json!({ "key": "k" }),
            )])
            .await;
        let error = outcome(&out[0]).expect_err("over the byte limit");
        let message = host_message(&error);
        assert!(message.starts_with("EventTooLarge"), "{message}");
        assert_eq!(
            message,
            format!("EventTooLarge: payload is {bytes} bytes; the limit is 65536")
        );
        assert!(p.outbox().await.is_empty());
    })
    .await;
}

/// R32: a payload the store writes out longer than JSON does — `1e300` is 5
/// bytes as JSON and 301 as `jsonb` text — is measured as the store measures
/// it, so it is refused before anything is sent: `EventTooLarge` with the
/// store's own figure, no row, no mark. On a `tx` handle the refusal leaves
/// the transaction usable: the app's writes before and after it commit, with
/// the event the handler then publishes.
#[tokio::test]
async fn a_payload_whose_numbers_the_store_writes_out_is_refused_before_anything_is_sent() {
    with_publisher(Publisher::new, |p| async move {
        let data = json!({ "orderId": 1, "xs": vec![1e300; 250] });
        let stored_bytes: i32 = connect(&p.writer().await.dsn)
            .await
            .query_one("SELECT octet_length(($1::jsonb)::text)", &[&data])
            .await
            .expect("measure in Postgres")
            .get(0);
        assert!(
            stored_bytes > 65_536 && serde_json::to_vec(&data).expect("json").len() < 65_536,
            "{stored_bytes}"
        );
        let too_large =
            format!("EventTooLarge: payload is {stored_bytes} bytes; the limit is 65536");

        let out = p
            .drive(vec![publish_step(EVENT, data.clone(), json!({ "key": "k" }))])
            .await;
        let error = outcome(&out[0]).expect_err("over the limit as the store writes it");
        assert_eq!(host_message(&error), too_large);
        assert!(p.outbox().await.is_empty(), "nothing was written");
        assert_eq!(p.mark("production").await, None, "nor marked");

        let project = workspace(&duck("events")).await;
        let host = p.host(&project, Uuid::new_v4(), None);
        let opened = host
            .tx("begin_oltp".into(), json!({}))
            .await
            .expect("begin");
        let tx = opened["id"].as_u64().expect("a handle id");
        let note = |n: i64| {
            json!({ "id": tx, "sql": "insert into notes (id) values ($1)", "params": [n] })
        };
        host.tx("exec".into(), note(1)).await.expect("the app's write");
        let refused = host
            .tx(
                "publish".into(),
                json!({ "id": tx, "type": EVENT, "data": data, "key": "big" }),
            )
            .await
            .expect_err("over the limit on the handle too");
        assert_is(&refused, &too_large);
        host.tx("exec".into(), note(2))
            .await
            .expect("the handle is still usable");
        host.tx(
            "publish".into(),
            json!({ "id": tx, "type": EVENT, "data": order(2), "key": "order-2" }),
        )
        .await
        .expect("a publish within the limit");
        host.tx("commit".into(), json!({ "id": tx }))
            .await
            .expect("commit");
        assert_eq!(p.notes().await, [1, 2], "the transaction committed");
        let rows = p.outbox().await;
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            [p.id_of("production", "order-2")]
        );
    })
    .await;
}

/// Postgres's `text`, and so `jsonb`, cannot hold U+0000: a payload holding it
/// in a string or a key, or a key holding it, is refused by the host before
/// anything is sent, in its own words. Refused by the store instead, the
/// statement would abort a `tx` handle; here the handle stays usable.
#[tokio::test]
async fn a_payload_or_key_holding_nul_is_refused_before_anything_is_sent() {
    with_publisher(Publisher::new, |p| async move {
        let in_a_string = json!({ "orderId": 1, "note": "a\u{0}b" });
        let in_a_key = json!({ "orderId": 1, "x\u{0}y": true });
        let payload_refusal =
            "EventPayloadInvalid: `data` must not contain U+0000, in a string or a key";
        let key_refusal = "EventKeyInvalid: `key` must not contain U+0000";

        let out = p
            .drive(vec![
                publish_step(EVENT, in_a_string.clone(), json!({ "key": "k" })),
                publish_step(EVENT, in_a_key.clone(), json!({ "key": "k" })),
                publish_step(EVENT, order(1), json!({ "key": "k\u{0}" })),
            ])
            .await;
        for (step, expected) in out.iter().zip([payload_refusal, payload_refusal, key_refusal]) {
            let error = outcome(step).expect_err("refused");
            assert_eq!(host_message(&error), expected);
        }

        let project = workspace(&duck("events")).await;
        let host = p.host(&project, Uuid::new_v4(), None);
        let opened = host
            .tx("begin_oltp".into(), json!({}))
            .await
            .expect("begin");
        let tx = opened["id"].as_u64().expect("a handle id");
        let note = |n: i64| {
            json!({ "id": tx, "sql": "insert into notes (id) values ($1)", "params": [n] })
        };
        host.tx("exec".into(), note(1)).await.expect("the app's write");
        for (data, key, expected) in [
            (in_a_string, "k", payload_refusal),
            (in_a_key, "k", payload_refusal),
            (order(1), "k\u{0}", key_refusal),
        ] {
            let refused = host
                .tx(
                    "publish".into(),
                    json!({ "id": tx, "type": EVENT, "data": data, "key": key }),
                )
                .await
                .expect_err("refused on the handle");
            assert_is(&refused, expected);
            host.tx("exec".into(), note(2))
                .await
                .expect("the handle is still usable");
            host.tx(
                "exec".into(),
                json!({ "id": tx, "sql": "delete from notes where id = 2", "params": [] }),
            )
            .await
            .expect("the handle is still usable");
        }
        host.tx(
            "publish".into(),
            json!({ "id": tx, "type": EVENT, "data": order(3), "key": "order-3" }),
        )
        .await
        .expect("a clean publish");
        host.tx("commit".into(), json!({ "id": tx }))
            .await
            .expect("commit");
        assert_eq!(p.notes().await, [1], "the transaction committed");
        let rows = p.outbox().await;
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            [p.id_of("production", "order-3")],
            "nothing refused reached the outbox"
        );
    })
    .await;
}

/// R32: the definer function refuses a payload over its ceiling with the
/// store's own text (`22023`, "data is N bytes as jsonb text"). Should it
/// ever refuse one the host passed, the app is told the host's own
/// `EventTooLarge`, with the store's figures, on both paths; nothing is
/// written. The store's ceiling is lowered here to make it refuse what the
/// host passes.
#[tokio::test]
async fn the_definers_size_refusal_reaches_the_app_as_event_too_large() {
    with_publisher(Publisher::new, |p| async move {
        let ceiling = oxy_oltp::platform::EVENT_MAX_DATA_TEXT_BYTES;
        let owner =
            connect(&oxy_oltp::migrator::owner_dsn(&p.tenant().await).expect("the owner's dsn"))
                .await;
        let definition: String = owner
            .query_one(
                &format!(
                    "SELECT pg_get_functiondef('{}'::regprocedure)",
                    oxy_oltp::platform::EVENTS_PUBLISH_FUNCTION
                ),
                &[],
            )
            .await
            .expect("read the definer function")
            .get(0);
        let lowered = definition
            .replace(&format!("> {ceiling} THEN"), "> 8 THEN")
            .replace(&format!("the limit is {ceiling}'"), "the limit is 8'");
        assert_ne!(lowered, definition, "the ceiling is in the function");
        owner
            .batch_execute(&lowered)
            .await
            .expect("lower the definer's ceiling");
        let bytes = jsonb_text_len(&order(7));
        let too_large = format!("EventTooLarge: payload is {bytes} bytes; the limit is 8");

        let out = p
            .drive(vec![publish_step(EVENT, order(7), json!({ "key": "k" }))])
            .await;
        let error = outcome(&out[0]).expect_err("the definer refuses it");
        assert_eq!(host_message(&error), too_large);

        let project = workspace(&duck("events")).await;
        let host = p.host(&project, Uuid::new_v4(), None);
        let opened = host
            .tx("begin_oltp".into(), json!({}))
            .await
            .expect("begin");
        let tx = opened["id"].as_u64().expect("a handle id");
        let refused = host
            .tx(
                "publish".into(),
                json!({ "id": tx, "type": EVENT, "data": order(7), "key": "k" }),
            )
            .await
            .expect_err("the definer refuses it on the handle");
        assert_is(&refused, &too_large);
        let _ = host.tx("rollback".into(), json!({ "id": tx })).await;
        assert!(p.outbox().await.is_empty());
    })
    .await;
}

/// Each bad key or delay is refused naming its field; a key of 200
/// two-byte characters and the longest delay are accepted.
#[tokio::test]
async fn bad_keys_and_delays_are_refused() {
    with_publisher(Publisher::new, |p| async move {
        let long = "k".repeat(201);
        let bad = [
            (json!({ "key": "" }), "key"),
            (json!({ "key": long }), "key"),
            (json!({}), "key"),
            (json!({ "key": 7 }), "key"),
            (json!({ "key": "k", "delaySeconds": -1 }), "delaySeconds"),
            (json!({ "key": "k", "delaySeconds": 1.5 }), "delaySeconds"),
            (
                json!({ "key": "k", "delaySeconds": 86_401 }),
                "delaySeconds",
            ),
            (json!({ "key": "k", "delaySeconds": "30" }), "delaySeconds"),
        ];
        let wide = "é".repeat(200);
        let mut steps: Vec<Value> = bad
            .iter()
            .map(|(opts, _)| publish_step(EVENT, order(1), opts.clone()))
            .collect();
        steps.push(publish_step(
            EVENT,
            order(1),
            json!({ "key": wide, "delaySeconds": 86_400 }),
        ));
        let out = p.drive(steps).await;
        for ((opts, field), step) in bad.iter().zip(&out) {
            let error = outcome(step).expect_err(&format!("{opts} is refused"));
            let message = host_message(&error);
            let expected = match *field {
                "key" => "EventKeyInvalid: `key` must be 1–200 characters",
                _ => "EventDelayInvalid: `delaySeconds` must be a whole number from 0 to 86400",
            };
            assert_eq!(message, expected, "{opts}");
            assert!(message.contains(&format!("`{field}`")), "{message}");
        }
        let accepted =
            outcome(&out[bad.len()]).expect("200 characters is the limit, in characters");
        let id = p.id_of("production", &wide);
        assert_eq!(accepted, &json!({ "eventId": id.to_string() }));
        let rows = p.outbox().await;
        assert_eq!(rows.len(), 1, "only the accepted publish wrote");
        assert_eq!(rows[0].delay_seconds, 86_400);
    })
    .await;
}

#[tokio::test]
async fn an_org_without_a_store_is_told_how_to_provision() {
    let make = || async {
        let p = Publisher::bare().await;
        enable_app_events(&p.t, &[p.org()]).await;
        p
    };
    with_publisher(make, |p| async move {
        let out = p
            .drive(vec![publish_step(EVENT, order(1), json!({ "key": "k" }))])
            .await;
        let error = outcome(&out[0]).expect_err("the org has no OLTP store");
        let message = host_message(&error);
        assert!(message.starts_with("EventsStoreMissing"), "{message}");
        assert!(message.contains("oxyc oltp provision"), "{message}");
        assert!(p.relay_tasks().await.is_empty());
    })
    .await;
}

/// A publish while handling an event at depth 8 would be depth 9; at depth 7
/// it is depth 8, chained to its cause.
#[tokio::test]
async fn depth_nine_is_refused() {
    with_publisher(Publisher::new, |p| async move {
        let project = workspace(&duck("events")).await;
        let cause = |depth| EventCause {
            id: Uuid::new_v4(),
            event_type: format!("orders/{EVENT}"),
            root_id: Uuid::new_v4(),
            depth,
            actor: Some("user:ada".into()),
        };
        let publish = json!({ "type": EVENT, "data": order(1), "key": "deep" });

        let deepest = cause(8);
        let error = p
            .host(&project, Uuid::new_v4(), Some(deepest))
            .events("publish".into(), publish.clone())
            .await
            .expect_err("depth 9");
        assert_eq!(
            error.message,
            "EventDepthExceeded: this event would be depth 9; the limit is 8"
        );
        assert!(p.outbox().await.is_empty());

        let parent = cause(7);
        let invocation = Uuid::new_v4();
        p.host(&project, invocation, Some(parent.clone()))
            .events("publish".into(), publish)
            .await
            .expect("depth 8 is the limit, and allowed");
        let rows = p.outbox().await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].depth, 8);
        assert_eq!(rows[0].causation_id, Some(parent.id));
        assert_eq!(rows[0].root_id, parent.root_id, "the chain keeps its root");
        assert_eq!(rows[0].actor, parent.actor, "the chain keeps its actor");
        assert_eq!(rows[0].invocation_id, Some(invocation));
    })
    .await;
}

/// `oxy_events` is the platform's: `ctx.oltp` SQL naming it is refused before
/// it is sent, however it is spelled and wherever on the OLTP path.
#[tokio::test]
async fn oltp_sql_naming_oxy_events_is_refused() {
    with_publisher(Publisher::new, |p| async move {
        let forged = format!(
            "select oxy_events.publish('{}'::uuid, 'production', '{EVENT}', 'k', \
             '{{}}'::jsonb, null, null, null, '{}'::uuid, 0, null, 0)",
            p.id_of("production", "k"),
            Uuid::new_v4()
        );
        let out = p
            .drive(vec![
                json!({ "kind": "oltpQuery", "sql": forged }),
                json!({ "kind": "oltpExec", "sql": "delete from OXY_EVENTS.outbox" }),
                json!({ "kind": "txExec", "sql": "select count(*) from \"oxy_events\".outbox" }),
                json!({
                    "kind": "oltpExec",
                    "sql": "do $$ begin execute 'select 1 from oxy_events.outbox'; end $$",
                }),
            ])
            .await;
        for step in &out {
            let error = outcome(step).expect_err("names the platform's schema");
            assert_eq!(
                host_message(&error),
                "OltpPlatformSchema: `oxy_events` belongs to the platform; publish with \
                 ctx.events.publish",
            );
        }
        // The spellings Postgres reads as the schema only when the statement
        // runs: a Unicode-escaped name, and `search_path` set from text built
        // then (on a handle, where it would last for the next statement).
        let unicode = forged.replacen("oxy_events", r#"U&"oxy\005fevents""#, 1);
        let out = p
            .drive(vec![
                json!({ "kind": "oltpQuery", "sql": unicode }),
                json!({
                    "kind": "txExec",
                    "sql": "select set_config('search_path', 'oxy' || '_events', true)",
                }),
            ])
            .await;
        for step in &out {
            let error = outcome(step).expect_err("spells the platform's schema when it runs");
            assert!(
                host_message(&error).starts_with("OltpPlatformSchema: "),
                "{error}"
            );
        }
        assert!(p.outbox().await.is_empty());
    })
    .await;
}

#[tokio::test]
async fn a_top_level_publish_roots_at_its_invocation() {
    with_publisher(Publisher::new, |p| async move {
        let out = p
            .drive(vec![publish_step(
                EVENT,
                order(1),
                json!({ "key": "root" }),
            )])
            .await;
        outcome(&out[0]).expect("the publish succeeds");
        let invocation = p.invocation().await;
        let rows = p.outbox().await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].root_id, invocation);
        assert_eq!(rows[0].invocation_id, Some(invocation));
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[0].causation_id, None);
        assert_eq!(rows[0].actor, Some(p.t.guest_id.to_string()), "the caller");
    })
    .await;
}

/// R21: the payload is built from named fields, so options carrying an `id`
/// or a `type` change neither the handle the publish runs on nor its type.
#[tokio::test]
async fn a_tx_publish_ignores_an_id_and_a_type_in_its_options() {
    with_publisher(Publisher::new, |p| async move {
        let out = p
            .drive(vec![json!({
                "kind": "txPublish",
                "note": 3,
                "type": EVENT,
                "data": order(3),
                "opts": { "key": "order-3", "id": 999, "type": UNDECLARED },
            })])
            .await;
        let id = p.id_of("production", "order-3");
        assert_eq!(outcome(&out[0]), Ok(&json!({ "eventId": id.to_string() })));
        let rows = p.outbox().await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].name, EVENT);
        assert_eq!(
            p.notes().await,
            [3],
            "published in the handle's own transaction"
        );
    })
    .await;
}

/// R11: a grant this process remembers but the store no longer holds (a
/// re-minted role, say) is granted again and the publish retried once — in
/// its own transaction, and on a handle, where the failure must not poison
/// the app's transaction.
#[tokio::test]
async fn a_stale_grant_is_granted_again_and_the_publish_retried() {
    with_publisher(Publisher::new, |p| async move {
        let project = workspace(&duck("events")).await;
        let host = p.host(&project, Uuid::new_v4(), None);
        let publish = |key: &str| json!({ "type": EVENT, "data": order(1), "key": key });
        host.events("publish".into(), publish("first"))
            .await
            .expect("the first publish grants and remembers it");

        let owner =
            connect(&oxy_oltp::migrator::owner_dsn(&p.tenant().await).expect("the owner's dsn"))
                .await;
        let role = p.writer().await.role;
        let revoke = format!(
            "REVOKE EXECUTE ON FUNCTION {} FROM \"{role}\"",
            oxy_oltp::platform::EVENTS_PUBLISH_FUNCTION
        );
        owner.batch_execute(&revoke).await.expect("revoke");
        host.events("publish".into(), publish("second"))
            .await
            .expect("granted again and retried");

        owner.batch_execute(&revoke).await.expect("revoke again");
        let tx = host
            .tx("begin_oltp".into(), json!({}))
            .await
            .expect("begin")["id"]
            .as_u64()
            .expect("a handle id");
        host.tx(
            "exec".into(),
            json!({ "id": tx, "sql": "insert into notes (id) values (9)" }),
        )
        .await
        .expect("the app's write");
        let mut on_handle = publish("third");
        on_handle["id"] = json!(tx);
        host.tx("publish".into(), on_handle)
            .await
            .expect("granted again and retried on the handle");
        host.tx("commit".into(), json!({ "id": tx }))
            .await
            .expect("the app's transaction was not poisoned");

        let keys: BTreeSet<String> = p.outbox().await.into_iter().map(|r| r.key).collect();
        assert_eq!(
            keys,
            BTreeSet::from(["first".into(), "second".into(), "third".into()])
        );
        assert_eq!(p.notes().await, [9]);
    })
    .await;
}

/// Staging with no OLTP branch writes nothing to production's store: the
/// publish — and one on a staging handle, which is production's store opened
/// read only — is held, listed in the invocation's held row, and no relay is
/// queued.
#[tokio::test]
async fn a_staging_publish_without_a_branch_is_held_and_writes_nothing() {
    with_publisher(Publisher::new, |p| async move {
        make_guest_staff();
        let host = staging_host(&p.t, &p.slug);
        let (out, channel) = p
            .drive_on(
                Some(&host),
                vec![
                    publish_step(EVENT, order(1), json!({ "key": "k" })),
                    json!({
                        "kind": "txPublish",
                        "type": EVENT,
                        "data": order(1),
                        "opts": { "key": "on-handle" },
                    }),
                ],
            )
            .await;
        assert_eq!(channel, "staging");
        for step in &out {
            let error = outcome(step).expect_err("held without a branch");
            assert!(
                host_message(&error).starts_with("HeldInStaging: "),
                "{error}"
            );
        }
        let rows = held_rows(&p.t).await;
        assert_eq!(rows.len(), 1, "one held row per invocation");
        let listed: BTreeSet<String> = held_ops(&rows[0]).into_iter().collect();
        for op in ["events.publish", "tx.publish"] {
            assert!(listed.contains(op), "{op} is listed: {listed:?}");
        }
        assert!(p.outbox().await.is_empty(), "nothing in production's store");
        assert!(p.relay_tasks().await.is_empty());
    })
    .await;
}

/// With the org's OLTP staging branch, a staging publish — and one on a
/// staging `ctx.oltp.tx` handle — lands in the branch's outbox as
/// `staging`, never in production's, and queues staging's relay.
///
/// R24: the branch is cut before the writer was granted anything, and the
/// host grants it the definer function on the branch, the store the publish
/// targets, never on production's.
#[tokio::test]
async fn a_staging_publish_lands_on_the_branch_and_never_in_production() {
    with_publisher(Publisher::new, |p| async move {
        make_guest_staff();
        let branch = p.store.provision_branch().await;
        let host = staging_host(&p.t, &p.slug);
        let (out, channel) = p
            .drive_on(
                Some(&host),
                vec![
                    publish_step(EVENT, order(1), json!({ "key": "outside" })),
                    json!({
                        "kind": "txPublish",
                        "note": 4,
                        "type": EVENT,
                        "data": order(4),
                        "opts": { "key": "on-handle" },
                    }),
                ],
            )
            .await;
        assert_eq!(channel, "staging");
        for step in &out {
            outcome(step).expect("a staging publish lands on the branch");
        }
        assert!(p.outbox().await.is_empty(), "nothing in production's store");
        assert!(p.notes().await.is_empty(), "nor the handle's write");

        let admin = crate::common::admin_url().await;
        let on_branch = connect(&on_database(&admin, &branch.database_name)).await;
        let rows = on_branch
            .query(
                "SELECT id, environment, key FROM oxy_events.outbox ORDER BY key",
                &[],
            )
            .await
            .expect("read the branch's outbox");
        let got: Vec<(Uuid, String, String)> = rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect();
        assert_eq!(
            got,
            [
                (
                    p.id_of("staging", "on-handle"),
                    "staging".to_string(),
                    "on-handle".to_string()
                ),
                (
                    p.id_of("staging", "outside"),
                    "staging".to_string(),
                    "outside".to_string()
                ),
            ]
        );
        // R28 reversed: staging's outbox is relayed, so the publish is
        // marked and nudges staging's relay.
        let queued: Vec<String> = p.relay_tasks().await.into_iter().map(|(_, e)| e).collect();
        assert_eq!(queued, ["staging"], "one staging relay is queued");
        assert!(
            p.mark("staging").await.is_some(),
            "the staging publish is marked"
        );
        let role = p.writer().await.role;
        let may_publish = |db: tokio_postgres::Client| {
            let role = role.clone();
            async move {
                db.query_one(
                    "SELECT has_schema_privilege($1, 'oxy_events', 'USAGE')",
                    &[&role],
                )
                .await
                .expect("read the writer's privilege")
                .get::<_, bool>(0)
            }
        };
        assert!(may_publish(on_branch).await, "granted on the branch");
        let production = connect(&on_database(&admin, &p.tenant().await.database_name)).await;
        assert!(
            !may_publish(production).await,
            "never granted on production's store"
        );
    })
    .await;
}

fn send_step() -> Value {
    json!({ "kind": "appsSend", "app": CALLEE, "fn": "handle", "body": { "n": 1 }, "opts": { "key": "s1" } })
}

/// `ctx.apps.send` is held in staging with no OLTP branch exactly as a publish
/// is: it writes nothing to production's store, lists itself in the held row
/// and queues no relay.
#[tokio::test]
async fn a_staging_send_without_a_branch_is_held_and_writes_nothing() {
    with_publisher(Publisher::new, |p| async move {
        make_guest_staff();
        let host = staging_host(&p.t, &p.slug);
        let (out, channel) = p.drive_on(Some(&host), vec![send_step()]).await;
        assert_eq!(channel, "staging");
        let error = outcome(&out[0]).expect_err("held without a branch");
        assert!(
            host_message(&error).starts_with("HeldInStaging: "),
            "{error}"
        );
        let rows = held_rows(&p.t).await;
        assert_eq!(rows.len(), 1, "one held row per invocation");
        assert!(held_ops(&rows[0]).iter().any(|op| op == "apps.send"));
        assert!(p.outbox().await.is_empty(), "nothing in production's store");
        assert!(p.relay_tasks().await.is_empty());
    })
    .await;
}

/// With the OLTP staging branch a staging send lands in the branch's outbox as
/// a staging event, never in production's.
#[tokio::test]
async fn a_staging_send_lands_on_the_branch_and_never_in_production() {
    with_publisher(Publisher::new, |p| async move {
        make_guest_staff();
        let branch = p.store.provision_branch().await;
        let host = staging_host(&p.t, &p.slug);
        let (out, _) = p.drive_on(Some(&host), vec![send_step()]).await;
        outcome(&out[0]).expect("a staging send lands on the branch");
        assert!(p.outbox().await.is_empty(), "nothing in production's store");
        let admin = crate::common::admin_url().await;
        let on_branch = connect(&on_database(&admin, &branch.database_name)).await;
        let rows = on_branch
            .query("SELECT environment, name FROM oxy_events.outbox", &[])
            .await
            .expect("read the branch's outbox");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<_, String>(0), "staging");
        assert!(
            rows[0]
                .get::<_, String>(1)
                .starts_with("call:send-callee:handle:")
        );
    })
    .await;
}
