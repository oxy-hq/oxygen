//! The OLTP change stream's test app, shared by `oltp_changes_e2e` and
//! `oltp_changes_relay`. Holds no tests.
//!
//! An [`App`] is a throwaway org with a compiled workspace and the app's OLTP
//! store, provisioned on the control plane's cluster (`AppStore`, as
//! `custom_app_functions_shape_zoo_oltp` provisions it), with the org flag
//! `oltp_changes` turned on by an org override row loaded into the flag
//! cache, as `serve` loads it at boot ([`enable_oltp_changes`]).
//!
//! It runs the paths production runs: a promoting `publish` applies the
//! build's OLTP migrations and installs capture on the tables it declares
//! under `oltpChanges` (`custom_apps_publish::pipeline::promote`, so
//! `apply_on_promote` then `install_on_promote`), and a function called
//! through the serve route writes with `ctx.oltp`, the isolate and the host
//! signing its transaction (`custom_apps_functions::host::oltp_sign`).

use std::future::Future;
use std::sync::Arc;

use chrono::Utc;
use futures::FutureExt as _;
use oxy_api_custom_apps::custom_apps_publish::{
    OrgRef, PublishError, PublishInput, PublishResult, publish,
};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use oxy_oltp::entity::tenants as oltp_tenants;
use oxy_oltp::migrator::{owner_dsn, tenant_for_org};
use oxy_oltp::resolver::{WriterConnection, resolve_writer_connection_for_org};
use oxy_oltp::schema::{analyst_role_for, app_writer_name, reconcile_migration_grants_sql};
use sea_orm::{ActiveModelTrait, ActiveValue::Set};
use serde_json::{Value, json};
use tokio_postgres::Client;
use uuid::Uuid;

use crate::custom_app_functions_fixture::{
    FunctionSpec, Tenant, call_function_in, seeded_tenant, throwaway_org,
};
use crate::custom_app_functions_shape_zoo::{compile, write_config};
use crate::custom_app_functions_shape_zoo_oltp::AppStore;
use crate::sandbox_publish::bundle;

/// The captured table most builds declare, as the app's first migration
/// creates it.
pub(crate) const LEASES_SQL: (&str, &[u8]) = (
    "migrations/0001_leases.sql",
    b"create table leases (id bigint primary key, name text);",
);
/// A migration that leaves `leases` to someone else, for a build that must
/// carry one (`oltpChanges` needs a `migrations` block, and a declared
/// directory with no files is refused).
pub(crate) const NOTES_SQL: (&str, &[u8]) = (
    "migrations/0001_notes.sql",
    b"create table notes (id bigint primary key);",
);

/// The one function the app declares: runs the request's `sql` with
/// `ctx.oltp.exec` and answers what it returned or threw.
pub(crate) const WRITE: &str = "write";

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

/// An org of the test's own with a compiled workspace and the app's OLTP
/// store; [`App::start`] provisions the store and turns `oltp_changes` on.
pub(crate) struct App {
    pub(crate) t: Tenant,
    pub(crate) slug: String,
    pub(crate) workspace: Uuid,
    pub(crate) store: AppStore,
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

    pub(crate) fn org(&self) -> Uuid {
        self.t.org_id
    }

    /// `app_<writer>`, where the app's tables and their capture live.
    pub(crate) fn schema(&self) -> String {
        self.store.writer.schema_name()
    }

    /// The bare writer name: the log's `writer` column, and `oltp_<writer>`
    /// in Airhouse.
    pub(crate) fn writer_name(&self) -> String {
        app_writer_name(&self.slug).expect("the slug backs a writer")
    }

    /// A promoting publish of build `build_id`: [`WRITE`], the migration
    /// `files`, and `leases` declared under `oltpChanges` when `declares`.
    pub(crate) async fn promote(
        &self,
        build_id: &str,
        declares: bool,
        files: &[(&str, &[u8])],
    ) -> Result<PublishResult, PublishError> {
        let tables: &[&str] = if declares { &["leases"] } else { &[] };
        self.promote_declaring(build_id, tables, files).await
    }

    /// [`App::promote`], declaring each of `tables` under `oltpChanges` (and
    /// no `oltpChanges` at all for none).
    pub(crate) async fn promote_declaring(
        &self,
        build_id: &str,
        tables: &[&str],
        files: &[(&str, &[u8])],
    ) -> Result<PublishResult, PublishError> {
        let mut extra = json!({ "migrations": { "dir": "migrations" } });
        if !tables.is_empty() {
            let declared: serde_json::Map<String, Value> = tables
                .iter()
                .map(|table| (table.to_string(), json!({})))
                .collect();
            extra["oltpChanges"] = json!({ "tables": declared });
        }
        self.promote_bundle(build_id, &[write_fn()], extra, files)
            .await
    }

    /// A promoting publish of build `build_id` carrying `functions`, with
    /// `extra` merged into its `oxy-app.json` and `files` beside it.
    pub(crate) async fn promote_bundle(
        &self,
        build_id: &str,
        functions: &[FunctionSpec],
        extra: Value,
        files: &[(&str, &[u8])],
    ) -> Result<PublishResult, PublishError> {
        publish(PublishInput {
            org_ref: Some(OrgRef::Id(self.org())),
            app_slug: self.slug.clone(),
            project_id: self.workspace,
            branch: None,
            build_id: build_id.to_string(),
            name: None,
            promote: true,
            tarball: bundle(&self.slug, functions, extra, files),
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
    pub(crate) async fn write(&self, sql: &str) {
        self.call(WRITE, json!({ "sql": sql })).await;
    }

    /// `function` called on production with `body`, as the guest, through
    /// the serve route: the `data` frame it answered, which names no `error`.
    pub(crate) async fn call(&self, function: &str, body: Value) -> Value {
        let call = call_function_in(&self.t.org_slug, &self.slug, function, body.clone()).await;
        let data = call
            .frame("data")
            .unwrap_or_else(|| panic!("no data frame ({}): {}", call.status, call.raw));
        assert!(data.get("error").is_none(), "{body}: {data}");
        data.clone()
    }

    /// A second app in this app's org, on the same workspace and store, with
    /// a slug and writer of its own. Its writer is minted in the org's store
    /// as another app's is, and this app's cleanup drops it with the store.
    pub(crate) async fn sibling(&self, name: &str) -> App {
        let t = Tenant {
            db: self.t.db.clone(),
            org_id: self.t.org_id,
            org_slug: self.t.org_slug.clone(),
            guest_id: self.t.guest_id,
        };
        let slug = format!("{}-{name}", self.slug);
        let store = AppStore::new(&t, &slug).await;
        self.store
            .ensure_extra_writer(&store.writer, self.workspace)
            .await;
        App {
            t,
            slug,
            workspace: self.workspace,
            store,
            _root: tempfile::tempdir().expect("a temp dir"),
        }
    }

    pub(crate) async fn tenant(&self) -> oltp_tenants::Model {
        tenant_for_org(&self.t.db, self.org())
            .await
            .expect("the org's tenant")
    }

    /// A session as the store's owner.
    pub(crate) async fn owner(&self) -> Client {
        connect(&owner_dsn(&self.tenant().await).expect("owner dsn")).await
    }

    pub(crate) async fn writer(&self) -> WriterConnection {
        resolve_writer_connection_for_org(&self.t.db, self.org(), &self.store.writer)
            .await
            .expect("resolve the app's writer")
    }

    /// `leases` created in the app's schema by the store's owner, as a
    /// workspace's `schemas/*.sql` create a table, then granted to the writer
    /// exactly as the migrator grants it (`reconcile_migration_grants_sql`).
    /// Returns the store owner's role.
    pub(crate) async fn create_leases_as_store_owner(&self) -> String {
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
}

/// Turn `oltp_changes` on for `t`'s org alone, as an admin's org override
/// does, and load the flag cache from the table.
pub(crate) async fn enable_oltp_changes(t: &Tenant) {
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
    oxy_feature_flags::cache::load_once(flag_db(&t.db))
        .await
        .expect("load the flag cache");
}

/// The flag cache's connector for a test: the test's own database handle, so a
/// load reads the override row the test just wrote, through the connection it
/// wrote it with (`OXY_DATABASE_URL` points at the same per-test database), not
/// the process-wide pool.
pub(crate) fn flag_db(
    db: &sea_orm::DatabaseConnection,
) -> impl oxy_feature_flags::ports::FlagDb + 'static {
    let db = db.clone();
    move || std::future::ready(Ok::<_, oxy_shared::errors::OxyError>(db.clone()))
}

/// The same for an auth lookup (`UserService`): the test's own database
/// handle, not the process-wide pool.
pub(crate) fn auth_db(db: &sea_orm::DatabaseConnection) -> impl oxy_auth::ports::AuthDb + 'static {
    let db = db.clone();
    move || std::future::ready(Ok::<_, oxy_shared::errors::OxyError>(db.clone()))
}

pub(crate) async fn connect(dsn: &str) -> Client {
    oxy_oltp::connect::connect(dsn, "oltp_changes test")
        .await
        .unwrap_or_else(|e| panic!("connect: {}", oxy_oltp::connect::pg_detail(&e)))
}

/// Run `body` against a fresh [`App`], started, dropping the org's store
/// (database, then roles) whatever happened.
pub(crate) async fn with_app<F, Fut>(body: F)
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
