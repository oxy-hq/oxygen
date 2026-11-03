//! The OLTP provisioner's fixture: a control-plane database from `common` and a
//! provisioner wired to the same cluster's tenant plane.
//!
//! Shared by path (`#[path = "../common/oltp_fx.rs"]`) between `oxy-app`'s
//! `platform` binary (`oltp_provisioner`, `oltp_branches`, `notifications_send`)
//! and `oxy-api-custom-apps`' integration binary (`console_test_notification`), so
//! neither carries a copy. Not a module of `common/mod.rs`: that harness is
//! included by crates that have no `oxy-oltp`.
#![allow(dead_code, unused_imports)]

use std::sync::Arc;

use futures::FutureExt as _;

use entity::organizations;
use oxy_oltp::provider::{LocalProvider, OltpProvider};
use oxy_oltp::sql::PgSqlExecutor;
use oxy_oltp::{GrantLevel, OltpProvisioner, ProvisionerError, WriterRef};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use uuid::Uuid;

use oxy_oltp::entity::roles::Entity as OltpRoles;
use oxy_oltp::entity::roles::{self as oltp_roles};
use oxy_oltp::entity::tenants::Entity as OltpTenants;
use oxy_oltp::entity::tenants::{self as oltp_tenants};

/// Control plane + a provisioner wired to the same cluster's tenant plane.
///
/// **Why one cluster here, when the sibling suite insists on its own.**
/// `crates/oltp/tests/integration/` refuses to run against a cluster holding
/// `oxy_org_*` databases, so CI gives it a throwaway on :15433 — and this
/// fixture must NOT be moved onto it. It provisions through the real
/// `OltpProvisioner`, so its databases are exactly the `oxy_org_<uuid>` that
/// guard looks for; under `cargo nextest run --workspace` the two suites
/// overlap and the guard would fire on this fixture's work, turning an
/// unrelated red build into the signal. The clusters are separate on purpose
/// and the separation has to run this way round.
///
/// The cost is residue: a hard kill leaves an `oxy_org_*` database and its
/// cluster-global roles in the control-plane service with nothing to collect
/// them. [`Fx::cleanup`] handles the ordinary path; the container being
/// per-job is what covers the rest.
pub(crate) struct Fx {
    pub(crate) db: DatabaseConnection,
    pub(crate) provisioner: OltpProvisioner,
    pub(crate) provider: Arc<LocalProvider>,
    pub(crate) org_id: Uuid,
    /// Workspace that claims a writer's schema namespace. A fixed id per
    /// fixture so repeat `ensure_writer` calls are the same claimant — a
    /// different one is a *collision*, which is a separate behaviour.
    pub(crate) claimant: Uuid,
    /// Role names this fixture minted, so [`Fx::cleanup`] can drop them.
    ///
    /// Needed because **writer roles are cluster-global and `delete_project`
    /// does not reclaim them** — it drops the database and the `_owner` role
    /// only. Without this every run leaves roles behind in the reused
    /// testcontainer and in CI's shared service container, on the happy path.
    pub(crate) minted_roles: std::sync::Mutex<Vec<String>>,
}

impl Fx {
    pub(crate) async fn create() -> Self {
        Self::create_on(crate::common::Schema::CentralOltp).await
    }

    /// [`Self::create`] on a database migrated by `schema` — `Schema::All` for
    /// a test whose code also queues work on the durable task queue.
    pub(crate) async fn create_on(schema: crate::common::Schema) -> Self {
        let (db, _url) = crate::common::fresh_db(schema).await;
        let admin_url = crate::common::admin_url().await;

        // `LocalProvider` wants the cluster's host:port separately from the DSN
        // because it builds per-tenant DSNs against databases that do not exist
        // yet.
        // The crate's own parser, not a copy: it handles a DSN with no
        // credentials and one carrying `?params`, both of which a hand-rolled
        // `rsplit_once('@')` gets wrong the moment CI's URL shape changes.
        let host = oxy_oltp::provider::host_from_dsn(&admin_url);
        let provider = Arc::new(LocalProvider::new(admin_url.clone(), host));

        let org_id = seed_org(&db).await;
        let provisioner = OltpProvisioner::new(
            db.clone(),
            provider.clone(),
            Arc::new(PgSqlExecutor),
            "local",
            oxy_oltp::config::DEFAULT_PG_VERSION,
        );
        Self {
            db,
            provisioner,
            provider,
            org_id,
            claimant: Uuid::new_v4(),
            minted_roles: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// A writer name unique to this fixture.
    ///
    /// **Postgres roles are cluster-global** — `role_name` is derived from the
    /// schema name alone (`app_bookings_rw`), with no org or database
    /// component. Two tests both asking for `bookings` therefore mint the *same*
    /// role, and `LocalProvider::create_role`'s `IF NOT EXISTS` guard is TOCTOU:
    /// under `db-per-test`'s `max-threads = 4` both sessions can observe
    /// "not exists" and the loser gets `42710 role already exists`. With
    /// `retries = 2` on that group it surfaces as an intermittent flake
    /// attributed to whichever test lost the race, which is the worst possible
    /// shape for it. Suffixing with the org id removes the shared name.
    pub(crate) fn writer(&self, base: &str) -> WriterRef {
        let tag = &self.org_id.simple().to_string()[..12];
        WriterRef::app(format!("{base}_{tag}")).expect("valid writer name")
    }

    /// A PIPELINE writer unique to this fixture.
    ///
    /// `raw_*`, analyst-visible by default — so it gets an
    /// `ALTER DEFAULT PRIVILEGES … TO analyst` row at provision, the object that
    /// broke the widened ownership guard. The fixture only ever built app
    /// writers before, which is why nothing caught it.
    pub(crate) fn pipeline_writer(&self, base: &str) -> WriterRef {
        let tag = &self.org_id.simple().to_string()[..12];
        WriterRef::pipeline(format!("{base}_{tag}")).expect("valid writer name")
    }

    /// `ensure_writer`, recording the role so cleanup can reclaim it.
    pub(crate) async fn ensure_writer(
        &self,
        writer: &WriterRef,
        grant: GrantLevel,
    ) -> Result<oxy_oltp::provisioner::WriterCredentials, ProvisionerError> {
        let created = self
            .provisioner
            .ensure_writer(self.org_id, writer, grant, Some(self.claimant))
            .await?;
        self.minted_roles
            .lock()
            .expect("minted_roles")
            .push(created.role_name.clone());
        Ok(created)
    }

    pub(crate) async fn tenant_row(&self) -> Option<oltp_tenants::Model> {
        OltpTenants::find()
            .filter(oltp_tenants::Column::OrgId.eq(self.org_id))
            .one(&self.db)
            .await
            .expect("query tenant")
    }

    /// Every role row in the control plane, with no tenant join.
    ///
    /// [`Fx::role_rows`] resolves through the tenant, so it cannot answer the
    /// question "did the ledger survive the tenant row?" — the very question a
    /// cascade test asks. Each test gets its own control-plane database, so
    /// unfiltered is scoped to this test.
    pub(crate) async fn all_role_rows(&self) -> Vec<oltp_roles::Model> {
        OltpRoles::find().all(&self.db).await.expect("query roles")
    }

    pub(crate) async fn role_rows(&self) -> Vec<oltp_roles::Model> {
        let tenant = self.tenant_row().await.expect("tenant row");
        OltpRoles::find()
            .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
            .all(&self.db)
            .await
            .expect("query roles")
    }

    /// Register a role for teardown that the fixture did not mint itself.
    ///
    /// A test that creates a role by hand — a stand-in owner, a squatter — has
    /// no other way into [`Fx::cleanup`], and a cluster-global role stranded by
    /// a failing assertion outlives the database it was made beside.
    pub(crate) fn register_role(&self, name: impl Into<String>) {
        self.minted_roles
            .lock()
            .expect("minted_roles")
            .push(name.into());
    }

    /// Best-effort teardown — the cluster is shared across tests in the run.
    ///
    /// Order matters: `deprovision` drops the database first, because a writer
    /// role still owning tables cannot be dropped ("role cannot be dropped
    /// because some objects depend on it"). Once the database is gone the roles
    /// own nothing and go quietly.
    pub(crate) async fn cleanup(&self) {
        let project = oxy_oltp::provider::database_name_for(
            &oxy_oltp::provisioner::project_name_for(self.org_id),
        );
        let _ = self.provisioner.deprovision(self.org_id).await;
        // Unconditionally, not only via `deprovision` — a second
        // `DROP DATABASE IF EXISTS` on the common path, and not redundant in
        // the other direction: `deprovision` covers the case this cannot (it
        // reclaims a tenant recorded under a name this fixture does not
        // derive), and this covers the case that one cannot.
        //
        // `deprovision` reads the tenant row and no-ops when there is none —
        // which is exactly the state a test asserting a REFUSED provision
        // leaves, and the state where a database can still exist under this
        // org's derived name. Locally that leak is worse than a leak:
        // `refuse_if_cluster_is_in_use` counts `oxy_org_*` cluster-wide, so one
        // stranded database blocks the whole oxy-oltp integration suite until
        // someone runs `just oltp-down`.
        let _ = self.provider.delete_project(&project).await;
        // The staging branch's database, by its derived name — the same
        // belt-and-braces as the tenant's, for a test that failed between the
        // copy and the row.
        let req = oxy_oltp::provider::BranchRequest::staging(
            project.as_str(),
            "local",
            oxy_oltp::OltpBranch::Staging.provider_name(),
            project.as_str(),
            format!("{project}_owner"),
        );
        let branch = oxy_oltp::provider::branch_database_name(&project, &req.name);
        let _ = self.provider.delete_branch(&req, &branch).await;
        let roles = self.minted_roles.lock().expect("minted_roles").clone();
        for role in roles {
            let _ = self.provider.delete_role(&project, "local", &role).await;
        }
    }
}

/// Run `body` against a fresh fixture, cleaning up **even when it panics**.
///
/// Cleanup used to be the last statement of each test, so a failed assertion
/// jumped straight past it and stranded the tenant database plus a
/// cluster-global writer role. The database half was papered over by adding
/// `oxy_org_` to the harness's stale-database sweep — but that sweep decides
/// staleness by the absence of a nextest run tag, and a tenant is named by the
/// *product*, so it can never carry one. Every live tenant looked permanently
/// stale, and the sweep fires per-`Schema` inside `ensure_template` while the
/// `custom_apps` / `airhouse` / `platform` binaries run concurrently against one
/// container — so an airhouse test starting after an OLTP test had provisioned
/// would drop that tenant mid-run.
///
/// Unwinding here fixes the cause instead, which lets the sweep entry go away.
/// It also reclaims the writer role, which the database sweep never covered at
/// all — and those names are unique per fixture now, so a panicking test would
/// otherwise strand one forever.
pub(crate) async fn with_fx<F, Fut>(body: F)
where
    F: FnOnce(std::sync::Arc<Fx>) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    run_fx(Fx::create().await, body).await;
}

/// [`with_fx`] on a database migrated by `schema`.
pub(crate) async fn with_fx_on<F, Fut>(schema: crate::common::Schema, body: F)
where
    F: FnOnce(std::sync::Arc<Fx>) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    run_fx(Fx::create_on(schema).await, body).await;
}

async fn run_fx<F, Fut>(fx: Fx, body: F)
where
    F: FnOnce(std::sync::Arc<Fx>) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let fx = std::sync::Arc::new(fx);
    let outcome = std::panic::AssertUnwindSafe(body(fx.clone()))
        .catch_unwind()
        .await;
    fx.cleanup().await;
    if let Err(payload) = outcome {
        std::panic::resume_unwind(payload);
    }
}

pub(crate) async fn seed_org(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set(format!("oltp-test-{}", id.simple())),
        slug: ActiveValue::Set(format!("oltp-test-{}", id.simple())),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    id
}
