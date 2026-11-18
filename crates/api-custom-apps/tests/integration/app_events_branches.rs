//! App events on the org's OLTP staging branch (Task 17), which hosts the
//! outbox of staging and of every sandbox on a schema there.
//!
//! Against real stores, as `staging_functions_oltp_branch` does: the org's
//! OLTP tenant on the control plane's cluster (`AppStore`) and its staging
//! branch, which `LocalProvider` copies into a sibling database. Every event
//! is published into the branch's outbox through the definer function as the
//! publishing app's writer role (the statement `ctx.events.publish` sends),
//! every app is published through the real `publish`, and every relay and
//! delivery takes the production path ([`relay_once`], the queued task run by
//! `AppFunctionTaskExecutor`).
//!
//! - **`store`**: a branch cut before the outbox gets it on first touch; a
//!   branch store gets its own writer grant, which the grant audit accepts;
//!   a branch row claiming production or a sandbox not hosted there is
//!   quarantined (R12); a mark with nothing waiting drains on its first relay.
//! - **`own_branch`**: a sandbox on an OLTP branch of its own publishes into
//!   that branch's outbox, relayed and delivered from there, and the branch
//!   quarantines a row claiming staging or production; one app's own
//!   branch that cannot be opened holds up no other app's events.
//! - **`reset_publish`**: after a reset wound a sandbox's own branch back to
//!   platform version 5, while the host still remembers its grant there, a
//!   publish (one-shot and on a `ctx.oltp.tx` handle) brings the branch
//!   current and lands signed.
//! - **`delivery`**: staging reaches a staging listener whose switch is on,
//!   end to end, and holds otherwise; `dev-x` reaches `dev-x` only and holds
//!   for an app without it; a staging event never reaches production; a
//!   sandbox's dead letter is recorded with every attempt; a sandbox's
//!   teardown stands once its store is gone.

mod delivery;
mod own_branch;
mod reset_publish;
mod store;

use std::sync::Arc;

use entity::{app_environments, app_event_deliveries as deliveries, app_events};
use futures::FutureExt as _;
use oxy_api_custom_apps::app_events::provenance::{Provenance, sign_hex};
use oxy_api_custom_apps::app_events::relay::{RelayStats, relay_once};
use oxy_api_custom_apps::app_events::schema::live_revision;
use oxy_api_custom_apps::custom_apps_publish::{PublishInput, PublishTarget, publish, publish_to};
use oxy_custom_apps::events::{event_id, jsonb_text_len, qualified_type};
use oxy_oltp::OltpBranch;
use oxy_oltp::events::{self as outbox, OutboxTarget, PUBLISH_SQL};
use oxy_oltp::resolver::{WriterConnection, branch_owner_dsn, resolve_branch_writer_for_org};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait, QueryFilter, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, seeded_tenant, throwaway_org};
use crate::custom_app_functions_shape_zoo::{compile, write_config};
use crate::custom_app_functions_shape_zoo_oltp::AppStore;
use crate::sandbox_publish::{bundle, input, sandbox};
use crate::staging_functions::{create_sandbox, make_guest_staff};

const EVENT: &str = "statement.uploaded";
/// Every listener's one function.
const FUNCTION: &str = "on-event";
/// Answers 200.
const OK_JS: &str = "export default async () => Response.json({ ok: true });";
/// Throws, every time.
const BROKEN_JS: &str = "export default async () => { throw new Error(\"the listener broke\"); };";

fn catalog() -> Value {
    let schema = json!({ "type": "object", "properties": { "period": { "type": "string" } } });
    json!({ "events": { EVENT: { "schema": schema } } })
}

/// An org with app events on, its OLTP store and staging branch, a compiled
/// workspace, and a publisher app whose promoted build declares [`EVENT`].
struct Fx {
    t: Tenant,
    workspace: Uuid,
    publisher: Uuid,
    slug: String,
    store: AppStore,
    tag: String,
    _root: tempfile::TempDir,
}

impl Fx {
    async fn new() -> Self {
        let t = throwaway_org(&seeded_tenant().await).await;
        crate::app_events_publish::enable_app_events(&t, &[t.org_id]).await;
        make_guest_staff();
        let tag = Uuid::new_v4().simple().to_string()[..8].to_string();
        let root = write_config("  - name: duck\n    type: duckdb\n    path: events.duckdb\n");
        let workspace = compile(&t, root.path()).await;
        let slug = format!("br-pub-{tag}");
        let store = AppStore::new(&t, &slug).await;
        store.provision(workspace).await;
        // Cut before anything was granted: no grant is copied to the branch.
        store.provision_branch().await;
        let publisher = promote(&t, &slug, workspace, &[], catalog()).await;
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

    fn event_type(&self) -> String {
        qualified_type(&self.slug, EVENT)
    }

    /// Publish listener app `<base>-<tag>`, promoted (it listens in
    /// production and staging): [`FUNCTION`] runs `js` on [`EVENT`], with
    /// `manifest`'s keys beside `on`. Its id and slug.
    async fn listener(&self, base: &str, js: &'static str, manifest: Value) -> (Uuid, String) {
        let slug = format!("{base}-{}", self.tag);
        let functions = [self.function(js, manifest)];
        let id = promote(&self.t, &slug, self.workspace, &functions, json!({})).await;
        (id, slug)
    }

    fn function(&self, js: &'static str, manifest: Value) -> FunctionSpec {
        let mut entry = json!({ "on": [self.event_type()], "timeoutSeconds": 30 });
        if let (Some(entry), Some(more)) = (entry.as_object_mut(), manifest.as_object()) {
            entry.extend(more.clone());
        }
        FunctionSpec {
            name: FUNCTION,
            manifest: entry,
            js,
        }
    }

    /// Create `app`'s sandbox `dev-<handle>` (its OLTP on a schema of the
    /// staging branch, the default) and publish `functions` to it, with the
    /// manifest keys in `extra`.
    async fn sandbox(
        &self,
        app: Uuid,
        slug: &str,
        handle: &str,
        functions: &[FunctionSpec],
        extra: Value,
    ) {
        create_sandbox(&self.t, app, handle, None).await;
        let build = format!("{slug}-{handle}");
        let tarball = bundle(slug, functions, extra, &[]);
        let to = PublishInput {
            project_id: self.workspace,
            ..input(&self.t, slug, &build, tarball)
        };
        publish_to(to, PublishTarget::Sandbox(sandbox(handle)))
            .await
            .unwrap_or_else(|e| panic!("publish {slug} to dev-{handle}: {e}"));
    }

    /// The publisher's sandbox `dev-<handle>`, declaring [`EVENT`].
    async fn publisher_sandbox(&self, handle: &str) {
        self.sandbox(self.publisher, &self.slug, handle, &[], catalog())
            .await;
    }

    /// Publish [`EVENT`] under `key` in `environment` into the branch's
    /// outbox, as an honest host would: the id derived for `environment`.
    async fn publish(&self, environment: &str, key: &str) -> Uuid {
        let id = event_id(self.org(), environment, &self.event_type(), key);
        self.publish_row(environment, key, id).await;
        id
    }

    /// One `oxy_events.publish` call on the branch as the publisher's writer,
    /// claiming `environment` under `id`: what its own SQL can do. Granted
    /// first, as the host grants, on the branch: whatever the row claims, it
    /// is the store the row is written to.
    async fn publish_row(&self, environment: &str, key: &str, id: Uuid) {
        let db = &self.t.db;
        let conn =
            resolve_branch_writer_for_org(db, self.org(), OltpBranch::Staging, &self.store.writer)
                .await
                .expect("resolve the writer on the branch")
                .expect("the org has a staging branch")
                .connection;
        let target = OutboxTarget::Branch("staging".to_string());
        outbox::ensure_publishable(db, self.org(), &target, &conn.role)
            .await
            .expect("grant on the branch");
        self.publish_signed(&conn, environment, key, id).await;
    }

    /// One `oxy_events.publish` call on `conn`, already granted the definer
    /// in its store, claiming `environment` under `id`, signed as the host
    /// signs a row it writes.
    async fn publish_signed(
        &self,
        conn: &WriterConnection,
        environment: &str,
        key: &str,
        id: Uuid,
    ) {
        let db = &self.t.db;
        // Against a revision recorded for the event (none for a name that is
        // no environment, which the relay refuses first anyway).
        let revision = live_revision(db, self.publisher, "production", EVENT)
            .await
            .expect("read the live revision")
            .map(|r| r.id);
        let data = json!({ "period": "2026-11" });
        // Signed as the host signs a row it writes, so the relay trusts it
        // (R25(c)): only the claimed environment is under test here.
        let event_type = self.event_type();
        let mac = sign_hex(
            self.org(),
            &Provenance {
                id,
                environment,
                event_type: &event_type,
                key,
                writer_role: &conn.role,
                actor: Some("user:test"),
                root_id: id,
                causation_id: None,
                depth: 0,
                delay_seconds: 0,
                data_bytes: jsonb_text_len(&data) as i64,
                schema_revision: revision,
                invocation_id: None,
            },
        );
        connect(&conn.dsn)
            .await
            .query_one(
                PUBLISH_SQL,
                &[
                    &id,
                    &environment,
                    &EVENT,
                    &key,
                    &data,
                    &revision,
                    &Some("user:test"),
                    &None::<Uuid>,
                    &id,
                    &0i32,
                    &None::<Uuid>,
                    &0i32,
                    &mac,
                ],
            )
            .await
            .unwrap_or_else(|e| panic!("publish on the branch: {e:?}"));
    }

    async fn relay(&self, environment: &str) -> RelayStats {
        relay_once(&self.t.db, self.org(), environment)
            .await
            .unwrap_or_else(|e| panic!("relay {environment}: {e}"))
    }

    async fn events(&self) -> Vec<app_events::Model> {
        app_events::Entity::find()
            .filter(app_events::Column::OrgId.eq(self.org()))
            .all(&self.t.db)
            .await
            .expect("read the events")
    }

    /// Every delivery of the org's events.
    async fn deliveries(&self) -> Vec<deliveries::Model> {
        let ids: Vec<Uuid> = self.events().await.into_iter().map(|e| e.id).collect();
        deliveries::Entity::find()
            .filter(deliveries::Column::EventId.is_in(ids))
            .all(&self.t.db)
            .await
            .expect("read the deliveries")
    }

    /// Turn `app`'s `environment` events switch on.
    async fn switch_on(&self, app: Uuid, environment: &str) {
        app_environments::Entity::update_many()
            .col_expr(app_environments::Column::EventsEnabled, Expr::value(true))
            .filter(app_environments::Column::AppId.eq(app))
            .filter(app_environments::Column::Name.eq(environment))
            .exec(&self.t.db)
            .await
            .expect("flip the events switch");
    }

    /// The branch's owner, connected.
    async fn branch_owner(&self) -> tokio_postgres::Client {
        let dsn = branch_owner_dsn(&self.t.db, self.org(), "staging")
            .await
            .expect("resolve the branch's owner")
            .expect("the org has a staging branch");
        connect(&dsn).await
    }

    /// Run `sql` on the platform database.
    async fn platform(&self, sql: &str, values: Vec<sea_orm::Value>) {
        self.t
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                values,
            ))
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
}

/// Publish build `<slug>-1` of `functions` (and the manifest keys in
/// `extra`), promoted; the app's id.
async fn promote(
    t: &Tenant,
    slug: &str,
    workspace: Uuid,
    functions: &[FunctionSpec],
    extra: Value,
) -> Uuid {
    let build = format!("{slug}-1");
    publish(PublishInput {
        project_id: workspace,
        promote: true,
        ..input(t, slug, &build, bundle(slug, functions, extra, &[]))
    })
    .await
    .unwrap_or_else(|e| panic!("publish {slug}: {e}"))
    .app_id
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

/// Run `body` against a fresh [`Fx`], dropping the org's store (branch,
/// database, then roles) whatever happened.
async fn with_fx<F, Fut>(body: F)
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
