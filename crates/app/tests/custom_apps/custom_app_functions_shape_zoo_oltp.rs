//! The Postgres shape-zoo cases, read through `ctx.oltp` in the app's own schema.
//!
//! `ctx.oltp` reaches a per-org tenant database through the app's writer role, so this test
//! provisions one the way `tests/platform/oltp_provisioner.rs` does: `OltpProvisioner` over
//! `LocalProvider`, on the control plane's own cluster (`common::admin_url`). It never uses CI's
//! `oltp-postgres` service, whose suite refuses a cluster holding `oxy_org_*` databases. No flag
//! is set: `oxy_oltp::flag::is_enabled` is `true` in a process that registered no check.
//!
//! The zoo table is loaded through the writer's own DSN, the connection `ctx.oltp` uses, then
//! read one column at a time by a published function. Each column is compared with its case's
//! `expect.oltp`, where `numeric`, `bytea`, `"char"` and arrays are `{"$error": …}`, because
//! that path refuses them by name.
//!
//! Writer roles are cluster-global, so the app slug carries a random suffix. The tenant database
//! and role are dropped even when an assertion fails.
//!
//! The app's org is a throwaway (`custom_app_functions_fixture::throwaway_org`), never the Local
//! org `seed_demo` creates: the tenant database is keyed on the org id and dropped on the cluster
//! `OXY_DATABASE_URL` names, and the Local org's id is a fixed one that a dev box's real tenant
//! shares.

use std::sync::{Arc, Mutex};

use agentic_connector::PostgresConnector;
use airhouse::LOCAL_ORG_ID;
use futures::FutureExt as _;
use oxy_oltp::provider::{LocalProvider, OltpProvider, database_name_for, host_from_dsn};
use oxy_oltp::provisioner::project_name_for;
use oxy_oltp::resolver::resolve_writer_connection_for_org;
use oxy_oltp::schema::app_writer_name;
use oxy_oltp::sql::PgSqlExecutor;
use oxy_oltp::{GrantLevel, OltpProvisioner, WriterRef};
use serde_json::json;
use uuid::Uuid;

use crate::common::admin_url;
use crate::custom_app_functions_fixture::{
    FunctionSpec, Tenant, publish_app, seeded_tenant, throwaway_org,
};
use crate::custom_app_functions_shape_zoo::{
    assert_cases, compile, fill_zoo, read_through_in, reads, write_config,
};
use crate::shape_zoo::{self, Engine, LoadedZoo, Plane};

const READ_OLTP: &str = "read-oltp";

const READ_OLTP_JS: &str = r#"
export default async (req, ctx) => {
  const { reads } = JSON.parse(req.body);
  const out = {};
  for (const { column, sql } of reads) {
    try {
      const rows = await ctx.oltp.query(sql);
      out[column] = rows.length === 1 ? rows[0][column] : { $error: `expected 1 row, got ${rows.length}` };
    } catch (err) {
      out[column] = { $error: String(err && err.message ? err.message : err) };
    }
  }
  return Response.json(out);
};
"#;

fn functions() -> Vec<FunctionSpec> {
    vec![FunctionSpec {
        name: READ_OLTP,
        manifest: json!({ "route": true, "timeoutSeconds": 60, "oltp": { "enabled": true } }),
        js: READ_OLTP_JS,
    }]
}

/// The org's OLTP tenant and this app's writer, on the control plane's cluster.
struct AppStore {
    provisioner: OltpProvisioner,
    provider: Arc<LocalProvider>,
    org_id: Uuid,
    writer: WriterRef,
    /// The role `ensure_writer` minted, which cleanup drops.
    minted: Mutex<Option<String>>,
}

impl AppStore {
    async fn new(t: &Tenant, slug: &str) -> Self {
        let admin = admin_url().await;
        let provider = Arc::new(LocalProvider::new(admin.clone(), host_from_dsn(&admin)));
        let provisioner = OltpProvisioner::new(
            t.db.clone(),
            provider.clone(),
            Arc::new(PgSqlExecutor),
            "local",
            oxy_oltp::config::DEFAULT_PG_VERSION,
        );
        let writer = WriterRef::app(app_writer_name(slug).expect("the slug backs a schema"))
            .expect("a valid writer name");
        Self {
            provisioner,
            provider,
            org_id: t.org_id,
            writer,
            minted: Mutex::new(None),
        }
    }

    async fn provision(&self, claimant: Uuid) {
        self.provisioner
            .provision(self.org_id)
            .await
            .expect("provision the org's OLTP tenant");
        let created = self
            .provisioner
            .ensure_writer(
                self.org_id,
                &self.writer,
                GrantLevel::ReadWrite,
                Some(claimant),
            )
            .await
            .expect("mint the app's writer");
        *self.minted.lock().expect("minted") = Some(created.role_name.clone());
    }

    /// Loads the Postgres zoo through the writer's DSN, whose `search_path` is the app's schema.
    async fn load(&self, t: &Tenant, zoo: &LoadedZoo) {
        let conn = resolve_writer_connection_for_org(&t.db, self.org_id, &self.writer)
            .await
            .expect("resolve the app's writer");
        let connector =
            PostgresConnector::from_dsn(&conn.dsn, conn.verify_tls).expect("writer connector");
        fill_zoo(&connector, Engine::Postgres, zoo).await;
    }

    /// Database first, then the role: a role still owning tables cannot be dropped.
    async fn cleanup(&self) {
        let project = database_name_for(&project_name_for(self.org_id));
        let _ = self.provisioner.deprovision(self.org_id).await;
        let _ = self.provider.delete_project(&project).await;
        let role = self.minted.lock().expect("minted").clone();
        if let Some(role) = role {
            let _ = self.provider.delete_role(&project, "local", &role).await;
        }
    }
}

#[tokio::test]
async fn shape_zoo_postgres_cases_read_through_ctx_oltp() {
    let t = throwaway_org(&seeded_tenant().await).await;
    assert_ne!(
        t.org_id, LOCAL_ORG_ID,
        "the OLTP zoo provisions and drops the org's tenant; never the Local org's"
    );
    let zoo = shape_zoo::load();
    let slug = format!("shape-zoo-{}", &Uuid::new_v4().simple().to_string()[..8]);
    // `ctx.oltp` needs no warehouse; any compiled config will do.
    let root = write_config("  - name: duck\n    type: duckdb\n    path: zoo.duckdb\n");
    let workspace = compile(&t, root.path()).await;
    let store = AppStore::new(&t, &slug).await;
    let outcome = std::panic::AssertUnwindSafe(async {
        store.provision(workspace).await;
        store.load(&t, &zoo).await;
        publish_app(&t, &slug, workspace, &functions()).await;
        let body = json!({ "reads": reads(&zoo, Engine::Postgres) });
        let got = read_through_in(&t.org_slug, &slug, READ_OLTP, body).await;
        assert_cases(Engine::Postgres, Plane::Oltp, &zoo, &got);
    })
    .catch_unwind()
    .await;
    store.cleanup().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
