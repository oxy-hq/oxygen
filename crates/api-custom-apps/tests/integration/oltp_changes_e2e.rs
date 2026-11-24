//! The OLTP change stream end to end, through the paths production runs: a
//! promoting `publish` applies the build's OLTP migrations and installs
//! capture on the tables it declares under `oltpChanges`
//! (`custom_apps_publish::pipeline::promote`, so `apply_on_promote` then
//! `install_on_promote`), and a function called through the serve route
//! writes with `ctx.oltp`, the isolate and the host signing its transaction
//! (`custom_apps_functions::host::oltp_sign`). Every assertion reads the
//! org's store as its owner ([`OwnerReads`]).
//!
//! The org is a throwaway one with its OLTP store provisioned on the control
//! plane's cluster (`AppStore`, as `custom_app_functions_shape_zoo_oltp`
//! provisions it), and the org flag `oltp_changes` is turned on with an org
//! override row loaded into the flag cache, as `serve` loads it at boot.
//!
//! Who owns a captured table decides who attaches its triggers: a table the
//! app's migrations made is the writer's, one the store owner made in the
//! app's schema (as a workspace's `schemas/*.sql` do) is the owner's, and one
//! any other role owns is refused.

use std::future::Future;
use std::sync::Arc;

use chrono::Utc;
use futures::FutureExt as _;
use oxy_api_custom_apps::custom_apps_publish::{
    OrgRef, PublishError, PublishInput, PublishResult, publish,
};
use oxy_api_custom_apps::oltp_changes::provenance::{self, ChangeSigner};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use oxy_oltp::WriterRef;
use oxy_oltp::entity::tenants as oltp_tenants;
use oxy_oltp::migrator::{owner_dsn, tenant_for_org};
use oxy_oltp::resolver::{WriterConnection, resolve_writer_connection_for_org};
use oxy_oltp::schema::{analyst_role_for, app_writer_name, reconcile_migration_grants_sql};
use sea_orm::{ActiveModelTrait, ActiveValue::Set};
use serde_json::{Value, json};
use tokio_postgres::Client;
use uuid::Uuid;

use crate::common::admin_url;
use crate::custom_app_functions_fixture::{
    FunctionSpec, Tenant, call_function_in, invocations, seeded_tenant, throwaway_org,
};
use crate::custom_app_functions_shape_zoo::{compile, write_config};
use crate::custom_app_functions_shape_zoo_oltp::AppStore;
use crate::sandbox_publish::bundle;

/// The captured table every build here declares, as the app's first
/// migration creates it.
const LEASES_SQL: (&str, &[u8]) = (
    "migrations/0001_leases.sql",
    b"create table leases (id bigint primary key, name text);",
);
/// A migration that leaves `leases` to someone else, for a build that must
/// carry one (`oltpChanges` needs a `migrations` block, and a declared
/// directory with no files is refused).
const NOTES_SQL: (&str, &[u8]) = (
    "migrations/0001_notes.sql",
    b"create table notes (id bigint primary key);",
);
/// The next migration: a column added to `leases` and backfilled in the same
/// file.
const REGION_SQL: (&str, &[u8]) = (
    "migrations/0002_region.sql",
    b"alter table leases add column region text; update leases set region = 'north';",
);

/// The one function the app declares: runs the request's `sql` with
/// `ctx.oltp.exec` and answers what it returned or threw.
const WRITE: &str = "write";

const WRITE_JS: &str = r#"
export default async (req, ctx) => {
  const { sql } = JSON.parse(req.body);
  try {
    return Response.json({ ok: await ctx.oltp.exec(sql) });
  } catch (e) {
    return Response.json({ error: String(e && e.message ? e.message : e) });
  }
};
"#;

fn write_fn() -> FunctionSpec {
    FunctionSpec {
        name: WRITE,
        manifest: json!({ "route": true, "timeoutSeconds": 60, "oltp": { "enabled": true } }),
        js: WRITE_JS,
    }
}

/// The four capture triggers, by name, each enabled always (`tgenabled`).
fn capture_triggers() -> Vec<(String, String)> {
    ["d", "i", "t", "u"]
        .iter()
        .map(|op| (format!("oxy_capture_{op}"), "A".to_string()))
        .collect()
}

/// One `oxy_changes.log` row, as the relay reads it.
#[derive(Debug, Clone, PartialEq)]
struct Change {
    op: String,
    schema_version: i32,
    key: Value,
    image: Value,
    txid: u64,
    actor: Option<String>,
    invocation: Option<Uuid>,
    provenance: Option<Vec<u8>>,
}

/// An org of the test's own with a compiled workspace and the app's OLTP
/// store; [`App::start`] provisions the store and turns `oltp_changes` on.
struct App {
    t: Tenant,
    slug: String,
    workspace: Uuid,
    store: AppStore,
    _root: tempfile::TempDir,
}

impl App {
    async fn new() -> Self {
        let t = throwaway_org(&seeded_tenant().await).await;
        let slug = format!("chg-e2e-{}", &Uuid::new_v4().simple().to_string()[..8]);
        let root = write_config("  - name: duck\n    type: duckdb\n    path: changes.duckdb\n");
        let workspace = compile(&t, root.path()).await;
        let store = AppStore::new(&t, &slug).await;
        Self {
            t,
            slug,
            workspace,
            store,
            _root: root,
        }
    }

    /// The org's store and the app's writer, and `oltp_changes` on for the
    /// org. Nothing is published yet.
    async fn start(&self) {
        self.store.provision(self.workspace).await;
        enable_oltp_changes(&self.t).await;
    }

    fn org(&self) -> Uuid {
        self.t.org_id
    }

    /// `app_<writer>`, where the app's tables and their capture live.
    fn schema(&self) -> String {
        self.store.writer.schema_name()
    }

    /// A promoting publish of build `build_id`: [`WRITE`], the migration
    /// `files`, and `leases` declared under `oltpChanges` when `declares`.
    async fn promote(
        &self,
        build_id: &str,
        declares: bool,
        files: &[(&str, &[u8])],
    ) -> Result<PublishResult, PublishError> {
        let mut extra = json!({ "migrations": { "dir": "migrations" } });
        if declares {
            extra["oltpChanges"] = json!({ "tables": { "leases": {} } });
        }
        publish(PublishInput {
            org_ref: Some(OrgRef::Id(self.org())),
            app_slug: self.slug.clone(),
            project_id: self.workspace,
            branch: None,
            build_id: build_id.to_string(),
            name: None,
            promote: true,
            tarball: bundle(&self.slug, &[write_fn()], extra, files),
            manifest: None,
            source_repo: None,
            commit_sha: None,
            published_by: Some(self.t.guest_id),
            published_by_email: Some(LOCAL_GUEST_EMAIL.to_string()),
            publisher: Some(oxy_server_authz::Caller::without_credential(
                self.t.guest_id,
                LOCAL_GUEST_EMAIL,
            )),
            machine_app_id: None,
            published_via: None,
            semantic_revision_id: None,
            confirm_schedule_removals: Vec::new(),
        })
        .await
    }

    /// [`WRITE`] called on production with `sql`, as the guest: through the
    /// serve route, the isolate and the host, which signs its transaction.
    async fn write(&self, sql: &str) {
        let call =
            call_function_in(&self.t.org_slug, &self.slug, WRITE, json!({ "sql": sql })).await;
        let data = call
            .frame("data")
            .unwrap_or_else(|| panic!("no data frame ({}): {}", call.status, call.raw));
        assert!(data.get("error").is_none(), "{sql}: {data}");
    }

    async fn tenant(&self) -> oltp_tenants::Model {
        tenant_for_org(&self.t.db, self.org())
            .await
            .expect("the org's tenant")
    }

    /// A session as the store's owner.
    async fn owner(&self) -> Client {
        connect(&owner_dsn(&self.tenant().await).expect("owner dsn")).await
    }

    /// The store read as its owner, for the assertions.
    async fn as_owner(&self) -> OwnerReads {
        OwnerReads {
            client: self.owner().await,
            schema: self.schema(),
        }
    }

    /// The cluster's admin on the org's database: a superuser, which may give
    /// a table to any role.
    async fn admin(&self) -> Client {
        let database = self.tenant().await.database_name;
        connect(&on_database(&admin_url().await, &database)).await
    }

    async fn writer(&self) -> WriterConnection {
        resolve_writer_connection_for_org(&self.t.db, self.org(), &self.store.writer)
            .await
            .expect("resolve the app's writer")
    }

    /// `leases` created in the app's schema by the store's owner, as a
    /// workspace's `schemas/*.sql` create a table, then granted to the writer
    /// exactly as the migrator grants it (`reconcile_migration_grants_sql`).
    /// Returns the store owner's role.
    async fn create_leases_as_store_owner(&self) -> String {
        let owner = self.owner().await;
        let store_owner: String = owner
            .query_one("SELECT current_user::text", &[])
            .await
            .expect("read the store owner")
            .get(0);
        let tenant = self.tenant().await;
        let analyst = analyst_role_for(&tenant.provider, &tenant.database_name);
        let writer_role = self.writer().await.role;
        let grants = reconcile_migration_grants_sql(
            &self.store.writer,
            &store_owner,
            false,
            &analyst,
            &writer_role,
        )
        .expect("the migrator's grants");
        let create = format!(
            "create table {}.leases (id bigint primary key, name text)",
            self.schema()
        );
        for statement in std::iter::once(&create).chain(&grants) {
            owner
                .batch_execute(statement)
                .await
                .unwrap_or_else(|e| panic!("{statement}: {}", oxy_oltp::connect::pg_detail(&e)));
        }
        store_owner
    }

    /// `leases` created in the app's schema and given to the writer of
    /// another app in the org: a role that is neither the app's writer nor
    /// the store's owner. Returns that role.
    async fn give_leases_to_another_writer(&self) -> String {
        let name = app_writer_name(&format!("{}-x", self.slug)).expect("the slug backs a schema");
        let other = WriterRef::app(name).expect("a valid writer name");
        self.store.ensure_extra_writer(&other, self.workspace).await;
        let other_role = resolve_writer_connection_for_org(&self.t.db, self.org(), &other)
            .await
            .expect("resolve the other writer")
            .role;
        let leases = format!("{}.leases", self.schema());
        self.admin()
            .await
            .batch_execute(&format!(
                "create table {leases} (id bigint primary key, name text); \
                 alter table {leases} owner to \"{other_role}\""
            ))
            .await
            .unwrap_or_else(|e| panic!("give leases away: {}", oxy_oltp::connect::pg_detail(&e)));
        other_role
    }

    /// The ids of the rows `leases` holds, as the writer reads them.
    async fn live_ids(&self) -> Vec<i64> {
        connect(&self.writer().await.dsn)
            .await
            .query("select id from leases order by id", &[])
            .await
            .expect("read leases")
            .iter()
            .map(|r| r.get(0))
            .collect()
    }
}

/// The org's store as its owner sees it: the catalog, and `oxy_changes`,
/// which no writer can read.
struct OwnerReads {
    client: Client,
    /// `app_<writer>`.
    schema: String,
}

impl OwnerReads {
    fn qualified(&self, table: &str) -> String {
        format!("{}.{table}", self.schema)
    }

    /// `table`'s owner, as `pg_tables` names it.
    async fn table_owner(&self, table: &str) -> String {
        self.client
            .query_one(
                "SELECT tableowner::text FROM pg_catalog.pg_tables \
                  WHERE schemaname = $1 AND tablename = $2",
                &[&self.schema, &table],
            )
            .await
            .expect("read the table's owner")
            .get(0)
    }

    /// `table`'s oid.
    async fn relid(&self, table: &str) -> u32 {
        self.client
            .query_one("SELECT to_regclass($1)::oid", &[&self.qualified(table)])
            .await
            .expect("read the table's oid")
            .get(0)
    }

    /// `(tgname, tgenabled)` of each trigger on `table` (none when it does
    /// not exist), by name.
    async fn triggers(&self, table: &str) -> Vec<(String, String)> {
        self.client
            .query(
                "SELECT tgname::text, tgenabled::text FROM pg_catalog.pg_trigger \
                  WHERE tgrelid = to_regclass($1)::oid AND NOT tgisinternal ORDER BY tgname",
                &[&self.qualified(table)],
            )
            .await
            .expect("read the table's triggers")
            .iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect()
    }

    /// The capture functions generated for `relid`.
    async fn functions(&self, relid: u32) -> i64 {
        let prefix = format!("_oxy_capture_{relid}_");
        self.client
            .query_one(
                "SELECT count(*) FROM pg_catalog.pg_proc \
                  WHERE left(proname::text, length($1)) = $1",
                &[&prefix],
            )
            .await
            .expect("count the capture functions")
            .get(0)
    }

    /// `(table_name, schema_version, installed)` of each `oxy_changes.tables`
    /// row, by name.
    async fn tables(&self) -> Vec<(String, i32, bool)> {
        self.client
            .query(
                "SELECT table_name, schema_version, installed FROM oxy_changes.tables \
                 ORDER BY table_name",
                &[],
            )
            .await
            .expect("read oxy_changes.tables")
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect()
    }

    /// The versions of `relid`'s recorded shapes, in order.
    async fn shapes(&self, relid: u32) -> Vec<i32> {
        self.client
            .query(
                "SELECT schema_version FROM oxy_changes.shapes WHERE relid = $1 \
                 ORDER BY schema_version",
                &[&relid],
            )
            .await
            .expect("read oxy_changes.shapes")
            .iter()
            .map(|r| r.get(0))
            .collect()
    }

    /// The names of the columns `relid`'s shape at `version` captures.
    async fn shape_columns(&self, relid: u32, version: i32) -> Vec<String> {
        let columns: String = self
            .client
            .query_one(
                "SELECT columns::text FROM oxy_changes.shapes \
                  WHERE relid = $1 AND schema_version = $2",
                &[&relid, &version],
            )
            .await
            .expect("read the shape")
            .get(0);
        let columns: Vec<Value> = serde_json::from_str(&columns).expect("columns are JSON");
        columns
            .iter()
            .map(|c| c["name"].as_str().expect("a column's name").to_string())
            .collect()
    }

    async fn stream_rows(&self) -> i64 {
        self.client
            .query_one("SELECT count(*) FROM oxy_changes.stream", &[])
            .await
            .expect("count oxy_changes.stream")
            .get(0)
    }

    /// Every log row for `table`, in `seq` order.
    async fn log(&self, table: &str) -> Vec<Change> {
        self.client
            .query(
                "SELECT op::text, schema_version, coalesce(key::text, 'null'), \
                        coalesce(row_image::text, 'null'), txid::text, actor, \
                        invocation::text, provenance \
                   FROM oxy_changes.log WHERE table_name = $1 ORDER BY seq",
                &[&table],
            )
            .await
            .expect("read oxy_changes.log")
            .iter()
            .map(change_of)
            .collect()
    }

    /// Each row's latest change to `table` in the log, by its `id`; markers,
    /// which carry no key, left out.
    async fn latest_changes(&self, table: &str) -> Vec<Change> {
        let mut latest: Vec<Change> = Vec::new();
        for change in self.log(table).await {
            if change.key.is_null() {
                continue;
            }
            latest.retain(|c| c.key != change.key);
            latest.push(change);
        }
        latest.sort_by_key(|c| c.key["id"].as_i64());
        latest
    }
}

/// One row of [`OwnerReads::log`]'s query.
fn change_of(r: &tokio_postgres::Row) -> Change {
    let parsed =
        |i: usize| -> Value { serde_json::from_str(&r.get::<_, String>(i)).expect("JSON") };
    let txid: String = r.get(4);
    let invocation: Option<String> = r.get(6);
    Change {
        op: r.get(0),
        schema_version: r.get(1),
        key: parsed(2),
        image: parsed(3),
        txid: provenance::parse_txid(&txid).expect("a transaction id"),
        actor: r.get(5),
        invocation: invocation.map(|id| Uuid::parse_str(&id).expect("a uuid")),
        provenance: r.get(7),
    }
}

/// Turn `oltp_changes` on for `t`'s org alone, as an admin's org override
/// does, and load the flag cache from the table.
async fn enable_oltp_changes(t: &Tenant) {
    let mut orgs = serde_json::Map::new();
    orgs.insert(t.org_id.to_string(), json!("on"));
    entity::feature_flag::ActiveModel {
        key: Set(oxy_feature_flags::OLTP_CHANGES.to_string()),
        // The registry default; the kill switch is in the document.
        enabled: Set(false),
        updated_at: Set(Utc::now().fixed_offset()),
        targeting: Set(Some(json!({
            "enabled": true,
            "orgs": orgs,
            "fallthrough": { "type": "variant", "variant": "off" },
        }))),
    }
    .insert(&t.db)
    .await
    .expect("write the oltp_changes override");
    oxy_feature_flags::cache::load_once()
        .await
        .expect("load the flag cache");
}

async fn connect(dsn: &str) -> Client {
    oxy_oltp::connect::connect(dsn, "oltp_changes_e2e test")
        .await
        .unwrap_or_else(|e| panic!("connect: {}", oxy_oltp::connect::pg_detail(&e)))
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

/// Run `body` against a fresh [`App`], started, dropping the org's store
/// (database, then roles) whatever happened.
async fn with_app<F, Fut>(body: F)
where
    F: FnOnce(Arc<App>) -> Fut,
    Fut: Future<Output = ()>,
{
    let app = Arc::new(App::new().await);
    let run = {
        let app = app.clone();
        async move {
            app.start().await;
            body(app).await;
        }
    };
    // Boxed: a body that publishes, calls a function and reads the store
    // outgrows the test thread's stack when pinned on it.
    let outcome = std::panic::AssertUnwindSafe(Box::pin(run))
        .catch_unwind()
        .await;
    app.store.cleanup().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// The table an app's own migration creates is its writer's, so the writer
/// attaches the four capture triggers, each enabled always. A write the host
/// makes (a function's `ctx.oltp.exec` on production) is logged with the
/// caller as actor and the run's invocation, under a MAC that verifies for
/// that row's transaction id and under no other org.
#[tokio::test]
async fn an_app_migration_makes_a_writer_owned_table_and_its_writes_are_signed() {
    with_app(|app| async move {
        let published = app
            .promote("chg-1", true, &[LEASES_SQL])
            .await
            .expect("the promote applies the migration and installs capture");
        let store = app.as_owner().await;
        assert_eq!(store.table_owner("leases").await, app.writer().await.role);
        assert_eq!(store.triggers("leases").await, capture_triggers());
        assert_eq!(store.tables().await, [("leases".to_string(), 1, true)]);

        app.write("insert into leases (id, name) values (1, 'Flat 1')")
            .await;
        let log = store.log("leases").await;
        assert_eq!(log.len(), 1, "{log:?}");
        assert_eq!((log[0].op.as_str(), log[0].schema_version), ("i", 1));
        assert_eq!(log[0].key, json!({ "id": 1 }));
        assert_eq!(log[0].image, json!({ "id": 1, "name": "Flat 1" }));

        let runs = invocations(&app.t.db, published.app_id, WRITE).await;
        assert_eq!(runs.len(), 1, "{runs:?}");
        assert_signed_by_the_host(&app, &log[0], runs[0].id);
    })
    .await;
}

/// `change` was signed by the host for the guest's run `invocation` on
/// production: its actor and invocation are the signer's, and its MAC
/// verifies for that row's transaction id under the org's key, and for no
/// other org or transaction.
fn assert_signed_by_the_host(app: &App, change: &Change, invocation: Uuid) {
    let actor = app.t.guest_id.to_string();
    assert_eq!(change.actor.as_deref(), Some(actor.as_str()), "the caller");
    assert_eq!(change.invocation, Some(invocation), "the run");
    let schema = app.schema();
    let signer = ChangeSigner {
        txid: change.txid,
        environment: "production",
        writer: &schema,
        actor: &actor,
        invocation: Some(invocation),
    };
    let mac = change.provenance.as_deref();
    assert_eq!(mac.map(<[u8]>::len), Some(32), "the host's MAC");
    assert!(provenance::verify(app.org(), &signer, mac), "it verifies");
    assert!(
        !provenance::verify(Uuid::new_v4(), &signer, mac),
        "never under another org"
    );
    let elsewhere = ChangeSigner {
        txid: change.txid + 1,
        ..signer
    };
    assert!(
        !provenance::verify(app.org(), &elsewhere, mac),
        "never for another transaction"
    );
}

/// A table the store's owner creates in the app's schema, as a workspace's
/// `schemas/*.sql` do, granted to the writer as the migrator grants it: the
/// writer holds `ALL` on it but does not own it, so the owner attaches its
/// triggers. It is captured like any other, and a build declaring nothing
/// takes the capture down again as the owner, keeping its shapes.
#[tokio::test]
async fn a_workspace_owned_table_in_the_apps_schema_installs_and_captures() {
    with_app(|app| async move {
        let store_owner = app.create_leases_as_store_owner().await;
        let store = app.as_owner().await;
        assert_eq!(store.table_owner("leases").await, store_owner);

        app.promote("chg-1", true, &[NOTES_SQL])
            .await
            .expect("capture installs on a table the owner owns");
        let relid = store.relid("leases").await;
        assert_eq!(store.tables().await, [("leases".to_string(), 1, true)]);
        assert_eq!(store.triggers("leases").await, capture_triggers());
        assert_eq!(store.functions(relid).await, 4);

        app.write("insert into leases (id, name) values (1, 'Flat 1')")
            .await;
        let log = store.log("leases").await;
        assert_eq!(log.len(), 1, "{log:?}");
        assert_eq!((log[0].op.as_str(), log[0].schema_version), ("i", 1));
        assert_eq!(log[0].image, json!({ "id": 1, "name": "Flat 1" }));

        app.promote("chg-2", false, &[NOTES_SQL])
            .await
            .expect("a build declaring nothing removes the capture");
        assert!(store.triggers("leases").await.is_empty(), "triggers gone");
        assert!(store.tables().await.is_empty(), "its tables row gone");
        assert_eq!(store.functions(relid).await, 0);
        assert_eq!(store.shapes(relid).await, [1], "its shapes stay");
        app.write("insert into leases (id, name) values (2, 'Flat 2')")
            .await;
        assert_eq!(store.log("leases").await, log, "no longer captured");
    })
    .await;
}

/// A declared table some third role owns: neither the writer's session nor
/// the owner's may attach its triggers, so the promote is refused as the
/// author's error, naming the table and its owner, before anything of the
/// capture is installed.
#[tokio::test]
async fn a_table_owned_by_another_role_is_refused() {
    with_app(|app| async move {
        let other_role = app.give_leases_to_another_writer().await;
        let store = app.as_owner().await;
        assert_eq!(store.table_owner("leases").await, other_role);

        let refused = app
            .promote("chg-1", true, &[NOTES_SQL])
            .await
            .expect_err("a table another role owns refuses the promote");
        let PublishError::Migration(error) = &refused else {
            panic!("refused as a migration is: {refused}");
        };
        assert!(error.is_author_fault(), "the author's to fix: {error}");
        let message = error.to_string();
        assert!(message.contains("\"leases\""), "{message}");
        assert!(message.contains(&format!("\"{other_role}\"")), "{message}");

        let relid = store.relid("leases").await;
        assert!(store.tables().await.is_empty(), "nothing registered");
        assert!(store.triggers("leases").await.is_empty(), "no triggers");
        assert_eq!(store.functions(relid).await, 0, "no functions");
        assert_eq!(store.stream_rows().await, 0, "the stream is not seeded");
    })
    .await;
}

/// `leases` captured from a first promote, two rows written to it through
/// the host, then a second promote whose migration adds `region` and
/// backfills it before the install reshapes the table. The log as it stood
/// before the second promote.
async fn installed_then_backfilled(app: &App, store: &OwnerReads) -> Vec<Change> {
    app.promote("chg-1", true, &[LEASES_SQL])
        .await
        .expect("install capture on leases");
    app.write("insert into leases (id, name) values (1, 'Flat 1'), (2, 'Flat 2')")
        .await;
    let before = store.log("leases").await;
    assert_eq!(before.len(), 2, "{before:?}");
    app.promote("chg-2", true, &[LEASES_SQL, REGION_SQL])
        .await
        .expect("the backfill applies and the install reshapes leases");
    before
}

/// The acceptance test for the reshape baseline (design §5.5): a backfill
/// in the migration that adds a column reaches the change stream, so after
/// the promote every live row's latest change is a snapshot at the new
/// `schema_version` carrying the column. Today the backfill's update images
/// are filtered to the old shape and dropped as unchanged, so this fails;
/// PR 3 un-ignores it and deletes
/// `a_backfill_before_the_reshape_writes_no_change_today`.
#[tokio::test]
#[ignore = "needs the reshape baseline: design §5.5, review F14; PR 3"]
async fn a_backfill_in_the_migration_that_adds_a_column_reaches_the_change_stream() {
    with_app(|app| async move {
        let store = app.as_owner().await;
        installed_then_backfilled(&app, &store).await;
        assert_eq!(app.live_ids().await, [1, 2]);
        let latest = store.latest_changes("leases").await;
        let ids: Vec<i64> = latest.iter().filter_map(|c| c.key["id"].as_i64()).collect();
        assert_eq!(ids, [1, 2], "every live row has a change: {latest:?}");
        for change in &latest {
            assert_eq!(
                (change.op.as_str(), change.schema_version),
                ("s", 2),
                "a snapshot of the new shape: {change:?}"
            );
            assert_eq!(change.image["region"], json!("north"), "{change:?}");
        }
    })
    .await;
}

/// Today's capture of that same promote, pinned so a change to it is
/// noticed: the backfill runs while the triggers still bake in the old shape,
/// so each update's images are filtered to `id` and `name`, compare equal and
/// are dropped, and the backfill writes no log row; the reinstall after it
/// records `schema_version` 2, whose shape lists `region`. Replaced by
/// `a_backfill_in_the_migration_that_adds_a_column_reaches_the_change_stream`
/// once the reshape baseline lands (design §5.5, PR 3).
#[tokio::test]
async fn a_backfill_before_the_reshape_writes_no_change_today() {
    with_app(|app| async move {
        let store = app.as_owner().await;
        let before = installed_then_backfilled(&app, &store).await;
        assert_eq!(
            store.log("leases").await,
            before,
            "the backfill logged nothing"
        );
        let relid = store.relid("leases").await;
        assert_eq!(store.tables().await, [("leases".to_string(), 2, true)]);
        assert_eq!(store.shapes(relid).await, [1, 2]);
        assert_eq!(store.shape_columns(relid, 1).await, ["id", "name"]);
        assert_eq!(
            store.shape_columns(relid, 2).await,
            ["id", "name", "region"]
        );
    })
    .await;
}
