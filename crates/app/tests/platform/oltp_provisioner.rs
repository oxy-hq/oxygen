//! DB-backed integration tests for [`OltpProvisioner`].
//!
//! The design doc names these as the largest gap in the OLTP work, and says to
//! model them on `airhouse_provisioner.rs` — which is what this does.
//!
//! **Both planes are real Postgres, on one container.** The control plane is a
//! per-test database from [`common::fresh_db`]; the tenant plane is
//! [`LocalProvider`] pointed at the *same* cluster's superuser DSN, so it
//! genuinely runs `CREATE DATABASE`, `CREATE ROLE` and the platform DDL. That
//! matters more here than in most suites: every bug this subsystem has hit was
//! Postgres refusing to do what well-formed SQL looked like it did (four of
//! them in one session, all from assuming the database owner is a superuser).
//! A `MockProvider` would have passed every one.
//!
//! Run with: `cargo nextest run -p oxy-app --test platform -E 'test(oltp_provisioner)'`

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
    provider: Arc<LocalProvider>,
    pub(crate) org_id: Uuid,
    /// Workspace that claims a writer's schema namespace. A fixed id per
    /// fixture so repeat `ensure_writer` calls are the same claimant — a
    /// different one is a *collision*, which is a separate behaviour.
    claimant: Uuid,
    /// Role names this fixture minted, so [`Fx::cleanup`] can drop them.
    ///
    /// Needed because **writer roles are cluster-global and `delete_project`
    /// does not reclaim them** — it drops the database and the `_owner` role
    /// only. Without this every run leaves roles behind in the reused
    /// testcontainer and in CI's shared service container, on the happy path.
    minted_roles: std::sync::Mutex<Vec<String>>,
}

impl Fx {
    async fn create() -> Self {
        let (db, _url) = crate::common::fresh_db(crate::common::Schema::CentralOltp).await;
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
    fn pipeline_writer(&self, base: &str) -> WriterRef {
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
    async fn all_role_rows(&self) -> Vec<oltp_roles::Model> {
        OltpRoles::find().all(&self.db).await.expect("query roles")
    }

    async fn role_rows(&self) -> Vec<oltp_roles::Model> {
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
    fn register_role(&self, name: impl Into<String>) {
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
    async fn cleanup(&self) {
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
        let req = oxy_oltp::provider::BranchRequest {
            project_id: project.clone(),
            parent_branch_id: "local".into(),
            name: oxy_oltp::OltpBranch::Staging.provider_name().into(),
            database_name: project.clone(),
            owner_role: format!("{project}_owner"),
        };
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
    let fx = std::sync::Arc::new(Fx::create().await);
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

// ── provision ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn provision_creates_the_tenant_and_lands_it_at_the_current_platform_version() {
    with_fx(|fx| async move {
        let row = fx
            .provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");

        assert_eq!(row.org_id, fx.org_id);
        assert_eq!(
            row.platform_schema_version,
            oxy_oltp::platform::PLATFORM_SCHEMA_VERSION,
            "a new tenant must land at the current version, not at 0 with a \
             follow-up step someone has to remember"
        );
        assert!(
            fx.provider
                .get_project(&row.project_id)
                .await
                .expect("get project")
                .is_some(),
            "the remote project must actually exist"
        );

        fx.cleanup().await;
    })
    .await;
}

/// The property the whole design leans on: `provision` is the reconcile entry
/// point, not a create-once. Re-running must not buy a second billable project.
#[tokio::test]
async fn provision_is_idempotent_and_does_not_buy_a_second_project() {
    with_fx(|fx| async move {
        let first = fx.provisioner.provision(fx.org_id).await.expect("first");
        let second = fx.provisioner.provision(fx.org_id).await.expect("second");

        assert_eq!(first.id, second.id, "same local row");
        assert_eq!(
            first.project_id, second.project_id,
            "same remote project — a second one would be a silent recurring cost"
        );
        assert_eq!(
            OltpTenants::find()
                .filter(oltp_tenants::Column::OrgId.eq(fx.org_id))
                .all(&fx.db)
                .await
                .expect("count")
                .len(),
            1
        );

        fx.cleanup().await;
    })
    .await;
}

/// An unknown org must be rejected *before* anything is bought. This is the
/// ordering that matters: a project created and then orphaned by a failed
/// local insert keeps costing money and keeps holding data.
#[tokio::test]
async fn provisioning_an_unknown_org_creates_no_remote_project() {
    with_fx(|fx| async move {
        let ghost = Uuid::new_v4();

        let err = fx
            .provisioner
            .provision(ghost)
            .await
            .expect_err("unknown org must be rejected");
        assert!(
            matches!(err, ProvisionerError::OrgNotFound(id) if id == ghost),
            "expected OrgNotFound, got {err:?}"
        );

        let orphan = oxy_oltp::provisioner::project_name_for(ghost);
        assert!(
            fx.provider
                .get_project(&oxy_oltp::provider::database_name_for(&orphan))
                .await
                .expect("get project")
                .is_none(),
            "nothing may be provisioned for an org that does not exist"
        );

        fx.cleanup().await;
    })
    .await;
}

/// Self-healing: if the project is wiped provider-side, the next `provision`
/// recreates it rather than serving a local row pointing at nothing.
#[tokio::test]
async fn provision_recreates_a_project_that_vanished_provider_side() {
    with_fx(|fx| async move {
        let first = fx
            .provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");

        fx.provider
            .delete_project(&first.project_id)
            .await
            .expect("wipe the remote");
        assert!(
            fx.provider
                .get_project(&first.project_id)
                .await
                .expect("get")
                .is_none()
        );

        let healed = fx
            .provisioner
            .provision(fx.org_id)
            .await
            .expect("reconcile");
        assert!(
            fx.provider
                .get_project(&healed.project_id)
                .await
                .expect("get")
                .is_some(),
            "reconcile must restore the remote"
        );

        fx.cleanup().await;
    })
    .await;
}

// ── ensure_writer ───────────────────────────────────────────────────────────

#[tokio::test]
async fn ensure_writer_is_idempotent_and_records_one_role_row() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let writer = fx.writer("bookings");

        let first = fx
            .ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("ensure once");
        let second = fx
            .ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("ensure twice");

        assert_eq!(first.role_name, second.role_name);
        let rows = fx.role_rows().await;
        assert_eq!(
            rows.iter()
                .filter(|r| r.role_name == first.role_name)
                .count(),
            1,
            "a second ensure must not add a second row"
        );

        fx.cleanup().await;
    })
    .await;
}

/// The app-delete/rename guards refuse on `writer_is_provisioned`, and the
/// operator's way out is `deprovision_writer`. Pin the whole loop against a real
/// cluster — and assert the ACTUAL DDL, not just control-plane rows: the schema
/// (`pg_namespace`, per-database) and the role (`pg_roles`, cluster-global) are
/// present after ensure and GONE after deprovision.
///
/// Runs it for BOTH an app writer and a PIPELINE writer. The pipeline writer is
/// the load-bearing case: it holds `CREATE ON DATABASE` and a default-privilege
/// row, so `REASSIGN OWNED`/`DROP OWNED` must run on the tenant connection — a
/// deprovision that ran them on the admin database (as the first cut did) reaches
/// nothing and wedges `DROP ROLE` on 2BP01, which the `pg_roles` check below
/// catches. Without this, deleting either guard block, or that half-drop, leaves
/// the suite green on a property whose job is keeping one app out of another's
/// rows.
#[tokio::test]
async fn deprovision_writer_drops_schema_and_role_for_app_and_pipeline_writers() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        // A pipeline writer is analytics-visible by default, so `ensure_writer`
        // grants its tables to the analyst — which must therefore exist.
        fx.provisioner
            .ensure_analyst(fx.org_id)
            .await
            .expect("ensure analyst");

        let client = tenant_superuser_conn(fx.org_id).await;
        const NS: &str = "SELECT count(*) FROM pg_namespace WHERE nspname = $1";
        const RO: &str = "SELECT count(*) FROM pg_roles WHERE rolname = $1";

        for (writer, is_pipeline) in [
            (fx.writer("bookings"), false),
            (fx.pipeline_writer("toast"), true),
        ] {
            assert!(
                !oxy_oltp::resolver::writer_is_provisioned(&fx.db, fx.org_id, &writer)
                    .await
                    .expect("check before ensure"),
                "{writer}: nothing provisioned yet"
            );

            let creds = fx
                .ensure_writer(&writer, GrantLevel::ReadWrite)
                .await
                .expect("ensure");
            let schema = writer.schema_name();
            assert!(
                oxy_oltp::resolver::writer_is_provisioned(&fx.db, fx.org_id, &writer)
                    .await
                    .expect("check after ensure"),
                "{writer}: provisioned → the guard refuses delete/rename"
            );
            assert_eq!(
                count(&client, NS, &schema).await,
                1,
                "{writer}: schema exists"
            );
            assert_eq!(
                count(&client, RO, &creds.role_name).await,
                1,
                "{writer}: role exists"
            );

            // Give the PIPELINE writer an object OUTSIDE its own schema, which
            // only `CREATE ON DATABASE` (pipeline-only) lets it do. Without this
            // the writer owns nothing after DROP SCHEMA CASCADE and the routing
            // is untested: the residue is what forces REASSIGN/DROP OWNED to run
            // on the tenant connection, and a deprovision that ran them on the
            // admin DB would leave this owned and wedge DROP ROLE on 2BP01.
            let squatted = format!("squatted_{}", writer.schema_name());
            if is_pipeline {
                run_sql(&creds.dsn, &format!("CREATE SCHEMA \"{squatted}\"")).await;
                run_sql(
                    &creds.dsn,
                    &format!("CREATE TABLE \"{squatted}\".t (i int)"),
                )
                .await;
                assert_eq!(count(&client, NS, &squatted).await, 1, "squat exists");
            }

            // The operator's escape hatch: drop just this writer's schema + role.
            fx.provisioner
                .deprovision_writer(fx.org_id, &writer)
                .await
                .expect("deprovision_writer");

            assert!(
                !oxy_oltp::resolver::writer_is_provisioned(&fx.db, fx.org_id, &writer)
                    .await
                    .expect("check after deprovision"),
                "{writer}: deprovisioned → the guard allows delete/rename again"
            );
            assert_eq!(
                count(&client, NS, &schema).await,
                0,
                "{writer}: schema dropped"
            );
            assert_eq!(
                count(&client, RO, &creds.role_name).await,
                0,
                "{writer}: role dropped (the pipeline case fails 2BP01 if DROP OWNED \
                 ran on the wrong database)"
            );
            assert!(
                !fx.role_rows()
                    .await
                    .iter()
                    .any(|r| r.role_name == creds.role_name),
                "{writer}: oltp_roles row removed"
            );
            if is_pipeline {
                // REASSIGN OWNED transfers, it does not drop: the squatted schema
                // survives, now owned by the tenant owner. Its survival is the
                // proof REASSIGN OWNED ran on the tenant connection — if it had
                // not, the role above would still own this and DROP ROLE would
                // have failed rather than reaching the assertion.
                assert_eq!(
                    count(&client, NS, &squatted).await,
                    1,
                    "the writer's out-of-schema object is reassigned to the owner, not dropped"
                );
            }

            // Idempotent: a second deprovision of a now-absent writer is a no-op.
            fx.provisioner
                .deprovision_writer(fx.org_id, &writer)
                .await
                .expect("second deprovision is a no-op");
        }

        fx.cleanup().await;
    })
    .await;
}

/// The writer's DSN must actually work, and must land unqualified writes in the
/// writer's own schema — the containment `search_path` is there to provide.
#[tokio::test]
async fn an_ensured_writer_can_actually_connect_and_write_its_own_schema() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let writer = fx.writer("orders");

        let conn = fx
            .ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("ensure writer");

        let (client, connection) = tokio_postgres::connect(&conn.dsn, tokio_postgres::NoTls)
            .await
            .expect("the minted DSN must connect");
        let driver = tokio::spawn(async move {
            let _ = connection.await;
        });

        client
            .batch_execute("CREATE TABLE tickets (id int primary key)")
            .await
            .expect("DDL in its own schema");
        let row = client
            .query_one(
                "SELECT schemaname::text AS s FROM pg_tables WHERE tablename = 'tickets'",
                &[],
            )
            .await
            .expect("locate the table");
        let schema: String = row.get("s");
        assert_eq!(
            schema,
            writer.schema_name(),
            "an unqualified CREATE must land in the writer's own schema"
        );

        // Drop the client first so the driver's future actually completes, then
        // join it. `abort()` alone returns immediately and the task can outlive the
        // test by a few milliseconds, which nextest intermittently reports as a
        // leaky test — an intermittent warning is worse than none, because it
        // teaches people to ignore the flag.
        drop(client);
        driver.abort();
        let _ = driver.await;
        fx.cleanup().await;
    })
    .await;
}

/// A re-minted analyst must still be able to READ what it was granted.
///
/// The analyst counterpart of `a_rotated_writer_can_still_write_its_own_schema`,
/// and the worse of the two: a writer re-mint costs one schema, an analyst
/// re-mint costs every schema in the org at once. `mint_role`'s remediation
/// branch DELETES the role through the provider and re-creates it, Postgres
/// keys ACL entries on the role OID, so a fresh role with the same name
/// inherits none of the grants `set_analytics_visibility` issued — and
/// `mint_role` restores `CONNECT` alone. The analyst comes back able to
/// authenticate and unable to read anything, from a call whose logs say it
/// succeeded.
///
/// Driven through the real remediation path rather than a stub: contaminating
/// the role with an unexpected group membership is what `assert_confined_sql`
/// raises `OXY01` on, which is the condition `ensure_analyst` re-mints for. And
/// asserted through `resolve_analyst_connection_for_org`, the same door the
/// `postgres_managed` query path uses, so this holds if the repair moves.
#[tokio::test]
async fn a_reminted_analyst_can_still_read_visible_schemas() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let writer = fx.writer("analyst_remint");
        let created = fx
            .ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("ensure writer");

        // A table the WRITER owns — the half of the analyst's grants that only
        // the writer can issue, and the half a re-mint drops.
        run_sql(&created.dsn, "CREATE TABLE readable (id int primary key)").await;
        run_sql(&created.dsn, "INSERT INTO readable VALUES (1)").await;

        fx.provisioner
            .set_analytics_visibility(fx.org_id, &writer, true)
            .await
            .expect("opt the schema into analytics");

        let table = format!("{}.readable", writer.schema_name());
        let read = || async {
            let conn = oxy_oltp::resolver::resolve_analyst_connection_for_org(&fx.db, fx.org_id)
                .await
                .expect("resolve the analyst connection");
            try_sql(&conn.dsn(), &format!("SELECT id FROM {table}")).await
        };

        read()
            .await
            .expect("baseline: the analyst can read the schema");

        // Contaminate the role so `ensure_analyst` takes its destructive
        // branch. Cluster-global, so the admin connection can grant it from any
        // database.
        let tenant = fx.tenant_row().await.expect("tenant row");
        let analyst = oxy_oltp::schema::analyst_role_for(&tenant.provider, &tenant.database_name);
        let group = format!("{analyst}_pretend_superuser");
        let admin = crate::common::admin_url().await;
        // Registered with the fixture so `with_fx`'s unwinding teardown drops it
        // however the assertions go. The pre-emptive drop that `grants.rs` uses
        // does NOT cover this test: `group` is derived from a per-run name
        // (org-id-suffixed), so it can never match a prior run's leftover — a
        // red run would strand a uniquely-named role nothing ever collects.
        fx.register_role(group.clone());
        run_sql(&admin, &format!("CREATE ROLE \"{group}\"")).await;
        run_sql(&admin, &format!("GRANT \"{group}\" TO \"{analyst}\"")).await;

        // Drives the real remediation branch, but on `LocalProvider`, where
        // `delete_role`/`create_role` are plain SQL. What Neon's API does to a
        // role that owns objects is still unobserved — only `neon_live` could
        // settle that.
        fx.provisioner
            .ensure_analyst(fx.org_id)
            .await
            .expect("re-mint the contaminated analyst");

        // The assertion. Without the grant re-application this is
        // `permission denied for schema …` on a call that reported success.
        read()
            .await
            .expect("a re-minted analyst must still read what it was granted");
    })
    .await;
}

/// Run one statement, panicking on failure.
async fn run_sql(dsn: &str, sql: &str) {
    try_sql(dsn, sql)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// Run one statement, returning the rendered error rather than panicking.
async fn try_sql(dsn: &str, sql: &str) -> Result<(), String> {
    let (client, connection) = tokio_postgres::connect(dsn, tokio_postgres::NoTls)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let driver = tokio::spawn(async move {
        let _ = connection.await;
    });
    let out = client
        .batch_execute(sql)
        .await
        // The SQLSTATE lives behind `as_db_error()`; `Display` is the bare
        // string "db error".
        .map_err(|e| match e.as_db_error() {
            Some(db) => format!("{}: {}", db.code().code(), db.message()),
            None => e.to_string(),
        });
    drop(client);
    driver.abort();
    let _ = driver.await;
    out
}

/// A superuser connection to the TENANT database (`oxy_org_<uuid>`), so a
/// per-database `pg_namespace` check can see a writer's schema. The admin URL
/// points at the admin database; this repoints the same superuser at the tenant
/// one. `pg_roles` is cluster-global and readable from here too. The connection
/// driver is detached — it lives until the returned client drops at test end.
pub(crate) async fn tenant_superuser_conn(org_id: Uuid) -> tokio_postgres::Client {
    let admin = crate::common::admin_url().await;
    let db =
        oxy_oltp::provider::database_name_for(&oxy_oltp::provisioner::project_name_for(org_id));
    let (base, query) = match admin.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (admin.as_str(), None),
    };
    let cut = base.rfind('/').expect("admin DSN has a /dbname path");
    let mut dsn = format!("{}/{}", &base[..cut], db);
    if let Some(q) = query {
        dsn.push('?');
        dsn.push_str(q);
    }
    let (client, connection) = tokio_postgres::connect(&dsn, tokio_postgres::NoTls)
        .await
        .unwrap_or_else(|e| panic!("connect tenant db {db}: {e}"));
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

/// `SELECT count(*)` catalog probe with one text parameter.
async fn count(client: &tokio_postgres::Client, sql: &str, arg: &str) -> i64 {
    client
        .query_one(sql, &[&arg])
        .await
        .unwrap_or_else(|e| panic!("{sql} [{arg}]: {e}"))
        .get(0)
}

/// A database that merely carries the tenant's name must be refused, not taken.
///
/// Adoption's safety argument is "the name is derived, so a match is an
/// identity match" — true of a database Oxy created, false of a restored dump
/// or a hand-made one sitting under the same name. Taking those over resets the
/// owner password and lands the row, and the failure surfaces two steps later
/// inside platform step 1 (`REVOKE ALL ON DATABASE … FROM PUBLIC` needs
/// ownership), by which time things have been mutated. A refusal that names the
/// owner is the operator-actionable answer, and it maps to 409 rather than 500.
#[tokio::test]
async fn a_foreign_owned_database_is_refused_and_names_its_owner() {
    with_fx(|fx| async move {
        // A database with the exact name this org's tenant would use, owned by
        // somebody else entirely.
        let db = oxy_oltp::provider::database_name_for(&oxy_oltp::provisioner::project_name_for(
            fx.org_id,
        ));
        let admin = crate::common::admin_url().await;
        let squatter = format!("{db}_squatter");
        // Registered BEFORE it exists, so `with_fx`'s unwinding teardown
        // reclaims it however the assertions below go. Dropping it after them
        // covers only the green path, which is the half that never needed
        // covering.
        fx.register_role(squatter.clone());
        let _ = try_sql(&admin, &format!("DROP DATABASE IF EXISTS \"{db}\"")).await;
        let _ = try_sql(&admin, &format!("DROP ROLE IF EXISTS \"{squatter}\"")).await;
        run_sql(&admin, &format!("CREATE ROLE \"{squatter}\" LOGIN")).await;
        run_sql(
            &admin,
            &format!("CREATE DATABASE \"{db}\" OWNER \"{squatter}\""),
        )
        .await;

        let err = fx
            .provisioner
            .provision(fx.org_id)
            .await
            .expect_err("provisioning must not take over a database it does not own");
        let rendered = format!("{err}");
        assert!(
            rendered.contains(&squatter),
            "the refusal must name the owner — that is the fact an operator \
             needs to act on: {rendered}"
        );

        // And nothing was mutated on the way to refusing.
        assert!(
            fx.tenant_row().await.is_none(),
            "a refused provision must not leave a tenant row behind"
        );
    })
    .await;
}

/// A half-provision must be recoverable by re-provisioning.
///
/// `create_new` has fallible steps between the provider call and the
/// `oltp_tenants` insert — a crypto failure, a control-plane blip — and each
/// leaves a created database with no row. Without adoption the next provision
/// takes the create path again, finds the database, and gets
/// `ProjectNameTaken`, which `is_retryable()` explicitly denies: the org wedges
/// until someone runs `DROP DATABASE` by hand, behind an error naming a
/// collision rather than whatever really failed.
///
/// The window itself cannot be reached from outside without fault injection,
/// but its RESULT can be produced exactly: a database whose row is gone. That
/// is the state a retry has to survive, so it is the state worth asserting on.
#[tokio::test]
async fn a_database_left_without_its_row_is_adopted_not_collided_with() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let before = fx.tenant_row().await.expect("tenant row");

        // With a writer, so the ledger is not empty by construction — which is
        // the one variant of "row gone" the first version of this test built,
        // and the variant that cannot show whether adoption strands anything.
        let writer = fx.writer("adopted");
        let created = fx
            .ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("ensure writer");
        run_sql(&created.dsn, "CREATE TABLE kept (id int primary key)").await;
        assert_eq!(
            fx.all_role_rows().await.len(),
            1,
            "the ledger has the writer"
        );

        // Exactly what a failure between `create_remote` and the insert leaves:
        // the database is there, the row is not.
        oltp_tenants::Entity::delete_many()
            .filter(oltp_tenants::Column::OrgId.eq(fx.org_id))
            .exec(&fx.db)
            .await
            .expect("drop the row");
        assert!(
            fx.tenant_row().await.is_none(),
            "the setup must actually leave the tenant unrecorded"
        );
        // `oltp_roles.tenant_row_id` is `REFERENCES oltp_tenants(id) ON DELETE
        // CASCADE`, so losing the tenant row takes the ledger with it. That is
        // what keeps adoption honest: it re-mints from an empty ledger rather
        // than binding a new tenant id over rows that still name the old one,
        // which would leave every writer unresolvable while its schema and
        // grants sat there intact.
        assert!(
            fx.all_role_rows().await.is_empty(),
            "the FK must cascade — otherwise adoption strands the role ledger"
        );

        // The retry must adopt the existing database rather than collide.
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("a re-provision must recover, not answer ProjectNameTaken");

        let after = fx.tenant_row().await.expect("the row must be back");
        assert_eq!(
            after.database_name, before.database_name,
            "and it must adopt the SAME database, not create a second one"
        );

        // The writer is re-declarable against the adopted database, and its
        // data is still there — the schema and its tables were never touched.
        let again = fx
            .ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("the writer must be re-declarable after adoption");
        run_sql(&again.dsn, "INSERT INTO kept VALUES (1)").await;

        fx.cleanup().await;
    })
    .await;
}

/// A stale `pg_version` must be corrected on the next provision.
///
/// The row is written once at creation, and `mark_active`'s early return used
/// to cover status alone — so an Active tenant never had the column re-read and
/// a cluster upgraded underneath it could not be noticed. On the local provider
/// that is the only kind of drift possible, and `oxy oltp status` deliberately
/// suppresses its drift note there (the requested version is not honoured), so
/// nothing else could ever have said so.
///
/// Fails against the version where the status check came first, which is the
/// ordering the fix turns on.
#[tokio::test]
async fn a_stale_pg_version_is_refreshed_on_the_next_provision() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let real = fx.tenant_row().await.expect("tenant row").pg_version;

        // A major nobody is running, so a refresh is unambiguous. The tenant
        // stays Active, which is exactly the state the early return skipped.
        oltp_tenants::Entity::update_many()
            .col_expr(oltp_tenants::Column::PgVersion, Expr::value(15i16))
            .filter(oltp_tenants::Column::OrgId.eq(fx.org_id))
            .exec(&fx.db)
            .await
            .expect("stale the version");
        let staled = fx.tenant_row().await.expect("tenant row");
        assert_eq!(staled.pg_version, 15, "the setup must actually take");
        assert_eq!(
            staled.status,
            oxy_oltp::entity::tenants::TenantStatus::Active,
            "the tenant must still be Active — that is the case being tested"
        );

        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("re-provision");

        let after = fx.tenant_row().await.expect("tenant row");
        assert_eq!(
            after.pg_version, real,
            "an Active tenant's recorded version must be refreshed from the \
             cluster, not left at whatever was stamped at creation"
        );
        assert_eq!(
            after.status,
            oxy_oltp::entity::tenants::TenantStatus::Active,
            "and refreshing it must not disturb the status"
        );

        fx.cleanup().await;
    })
    .await;
}

/// A re-minted pipeline writer keeps the analyst able to read its FUTURE tables.
///
/// The restore half of the guard fix, which the strip test does not cover:
/// `mint_role`'s remediation `DROP OWNED`s the writer's `ALTER DEFAULT
/// PRIVILEGES … TO analyst`, and `ensure_writer` must put it back. The
/// discriminating assertion is a table created AFTER the re-mint — readable by
/// the analyst only through the default-privilege grant, so it fails if the
/// restore is dropped (mirrors the analyst-side test one screen up).
#[tokio::test]
async fn a_reminted_pipeline_writer_keeps_the_analyst_reading_new_tables() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let writer = fx.pipeline_writer("toast");
        let created = fx
            .ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("ensure pipeline writer");

        // Contaminate the WRITER role so the next reconcile takes mint_role's
        // remediation branch (which DROP OWNEDs its default-priv). No table is
        // created first, so the strip is not refused on `pg_class`.
        let admin = crate::common::admin_url().await;
        let group = format!("{}_pretend_superuser", created.role_name);
        // Registered for teardown — per-run name, so no pre-emptive drop can
        // match it (see the analyst-side test above).
        fx.register_role(group.clone());
        run_sql(&admin, &format!("CREATE ROLE \"{group}\"")).await;
        run_sql(
            &admin,
            &format!("GRANT \"{group}\" TO \"{}\"", created.role_name),
        )
        .await;

        // Re-provision: reconcile → re-mint → restore.
        let recreated = fx
            .ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("re-provision the contaminated pipeline writer");

        // A table created AFTER the re-mint — readable only via the restored
        // default-privilege grant.
        run_sql(&recreated.dsn, "CREATE TABLE fresh (id int primary key)").await;
        run_sql(&recreated.dsn, "INSERT INTO fresh VALUES (1)").await;

        let conn = oxy_oltp::resolver::resolve_analyst_connection_for_org(&fx.db, fx.org_id)
            .await
            .expect("resolve analyst");
        let table = format!("{}.fresh", writer.schema_name());
        try_sql(&conn.dsn(), &format!("SELECT id FROM {table}"))
            .await
            .expect(
                "analyst must read a FUTURE table of the re-minted writer — default-priv restored",
            );

        let _ = try_sql(&admin, &format!("DROP ROLE IF EXISTS \"{group}\"")).await;
    })
    .await;
}

/// A pipeline writer's default-privilege row must NOT block the strip.
///
/// Every analytics-visible writer gets an `ALTER DEFAULT PRIVILEGES … TO
/// analyst` row (`defaclrole = writer`) at provision. Counting `pg_default_acl`
/// in the ownership guard therefore refused EVERY pipeline writer — so
/// `mint_role`'s remediation branch, the reason SQL-minting exists, could never
/// repair a contaminated one. The strip must succeed here; the grant it drops
/// is restorable (and `ensure_writer` restores it), unlike a table.
#[tokio::test]
async fn a_pipeline_writers_default_privilege_does_not_block_the_strip() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let writer = fx.pipeline_writer("toast");
        let created = fx
            .ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("ensure pipeline writer");

        // It owns its default-privilege row but NO table.
        let tenant = fx.tenant_row().await.expect("tenant row");
        fx.provisioner
            .strip_role_dependencies_for_test(&tenant, &created.role_name)
            .await
            .expect("a pipeline writer owning only a default-priv row must strip, not refuse");
    })
    .await;
}

/// A role owning only a FUNCTION — no table — must also be refused.
///
/// The guard once counted `pg_class` alone, but `DROP OWNED BY` also drops
/// functions, schemas and `ALTER DEFAULT PRIVILEGES` entries. A writer that owns
/// one of those but no table slipped the guard and had it dropped silently —
/// the default-privilege case losing the analyst's future-table SELECT with
/// only `apply_to_org` to restore it. This proves the widened count catches the
/// non-table case; the table case is covered above.
#[tokio::test]
async fn a_role_owning_a_non_table_object_is_also_refused() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let writer = fx.writer("owns_function");
        let created = fx
            .ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("ensure writer");

        // A function in the writer's own schema, owned by the writer — and NO
        // table, so a `pg_class`-only guard would see nothing to refuse.
        run_sql(
            &created.dsn,
            &format!(
                "CREATE FUNCTION {}.noop() RETURNS int LANGUAGE sql AS 'SELECT 1'",
                writer.schema_name()
            ),
        )
        .await;

        let tenant = fx.tenant_row().await.expect("tenant row");
        let err = fx
            .provisioner
            .strip_role_dependencies_for_test(&tenant, &created.role_name)
            .await
            .expect_err("a role owning a function must not be stripped");
        assert!(
            format!("{err}").contains("[OXY03]"),
            "the widened guard must refuse a non-table owner too: {err}"
        );

        // Still the writer's — DROP OWNED never ran.
        run_sql(
            &created.dsn,
            &format!("DROP FUNCTION {}.noop()", writer.schema_name()),
        )
        .await;
    })
    .await;
}

/// A writer that owns tables must not be re-minted behind the operator's back.
///
/// The dangerous shape is not failure, it is success. `REASSIGN OWNED BY writer
/// TO owner` makes the drop possible and hands the tenant's tables to the
/// database owner permanently — nothing gives them back, because
/// `ensure_writer_sql`'s `ReadWrite` arm grants `USAGE, CREATE ON SCHEMA` and
/// rests on the invariant that a writer owns what it created. The writer would
/// come back able to create new tables and denied on every row it had before:
/// the app up, new writes working, existing data unreachable, no error
/// anywhere. On a rotation — which is what an operator reaches for on a
/// suspected leak — that is the worst possible outcome.
///
/// So the strip refuses. This asserts the refusal AND that the data survived
/// it: a test that only checked for an error would pass against a version that
/// errored after moving the tables.
#[tokio::test]
async fn a_writer_that_owns_tables_is_refused_not_reassigned() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let writer = fx.writer("owns_tables");
        let created = fx
            .ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("ensure writer");
        run_sql(&created.dsn, "CREATE TABLE precious (id int primary key)").await;
        run_sql(&created.dsn, "INSERT INTO precious VALUES (1)").await;

        let tenant = fx.tenant_row().await.expect("tenant row");
        let err = fx
            .provisioner
            .strip_role_dependencies_for_test(&tenant, &created.role_name)
            .await
            .expect_err("a role owning tables must not be stripped");
        // `[OXY03]` with brackets, which only `pg_detail`'s rendering of a real
        // SQLSTATE produces. A bare `OXY03` or `owns` would also match the
        // ECHOED STATEMENT TEXT that `SqlError::Statement` carries — this
        // assertion passed against a version whose guard was raising `42601`,
        // because the string it matched was the SQL, not the error.
        let rendered = format!("{err}");
        assert!(
            rendered.contains("[OXY03]"),
            "the refusal must raise OUR error, not fail incidentally: {rendered}"
        );

        // And the tables are still the writer's, which is the half that would
        // otherwise be lost silently.
        run_sql(&created.dsn, "INSERT INTO precious VALUES (2)").await;
        run_sql(&created.dsn, "DROP TABLE precious").await;
    })
    .await;
}

/// A rotated credential must still be able to use its schema.
///
/// Rotation is what an operator reaches for on a suspected leak, so a rotation
/// that reports success and hands back a credential which authenticates and
/// then cannot touch anything is the worst possible shape for it — the incident
/// looks handled.
///
/// The path that made this reachable is inside `mint_role`: a role carrying
/// provider-granted authority cannot be repaired in place, so it is DELETED
/// through the provider and re-created. A fresh role keeps only what is granted
/// to it again, and `mint_role` restores `CONNECT` alone. `rotate_writer` now
/// re-applies the writer DDL for that reason; this asserts the outcome rather
/// than the call, so it holds if the recovery moves elsewhere.
#[tokio::test]
async fn a_rotated_writer_can_still_write_its_own_schema() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let writer = fx.writer("rotate");
        fx.ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("ensure writer");

        let rotated = fx
            .provisioner
            .rotate_writer(fx.org_id, &writer, GrantLevel::ReadWrite)
            .await
            .expect("rotate writer");

        let (client, connection) = tokio_postgres::connect(&rotated.dsn, tokio_postgres::NoTls)
            .await
            .expect("the rotated DSN must connect");
        let driver = tokio::spawn(async move {
            let _ = connection.await;
        });

        client
            .batch_execute("CREATE TABLE after_rotation (id int primary key)")
            .await
            .expect("a rotated writer must still own its schema");
        let row = client
            .query_one(
                "SELECT schemaname::text AS s FROM pg_tables WHERE tablename = 'after_rotation'",
                &[],
            )
            .await
            .expect("locate the table");
        assert_eq!(row.get::<_, String>("s"), writer.schema_name());

        drop(client);
        driver.abort();
        let _ = driver.await;
        fx.cleanup().await;
    })
    .await;
}

/// An unclaimed provision must not lock the namespace.
///
/// The console provisions ahead of any workspace declaring the writer, and it
/// passed `Uuid::nil()` under a comment saying it must not claim. But nil is a
/// workspace id like any other to the comparison in `claim_namespace`, so the
/// row came back claimed by a workspace that will never exist — and the real
/// one then failed `SchemaNamespaceClaimed` against it, with no way out but a
/// manual UPDATE. `None` is the representation that means what the comment said.
#[tokio::test]
async fn provisioning_ahead_of_a_workspace_leaves_the_namespace_free() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let writer = fx.writer("unclaimed");

        // The console's path: no workspace claims this yet.
        fx.provisioner
            .ensure_writer(fx.org_id, &writer, GrantLevel::ReadWrite, None)
            .await
            .expect("an unclaimed provision succeeds");

        let row = fx
            .role_rows()
            .await
            .into_iter()
            .find(|r| r.schema_name == writer.schema_name())
            .expect("the role row exists");
        assert_eq!(
            row.claimed_by_workspace_id, None,
            "an unclaimed provision must leave the namespace free, not claim it \
             for the nil workspace"
        );

        // The real workspace then adopts it rather than colliding.
        fx.provisioner
            .ensure_writer(fx.org_id, &writer, GrantLevel::ReadWrite, Some(fx.claimant))
            .await
            .expect("the declaring workspace must be able to take the namespace");

        // …and the adoption must be RECORDED. Without this the test passes on
        // a regression where the claim is never written — and that is the
        // dangerous one: the namespace stays adoptable forever, so a second
        // workspace takes it too and the two interleave DDL into one schema,
        // which is exactly what `claim_namespace` exists to prevent.
        let row = fx
            .role_rows()
            .await
            .into_iter()
            .find(|r| r.schema_name == writer.schema_name())
            .expect("the role row exists");
        assert_eq!(
            row.claimed_by_workspace_id,
            Some(fx.claimant),
            "the declaring workspace's claim must be recorded, not merely allowed"
        );

        // Which a different workspace must now lose to.
        let err = fx
            .provisioner
            .ensure_writer(
                fx.org_id,
                &writer,
                GrantLevel::ReadWrite,
                Some(Uuid::new_v4()),
            )
            .await
            .expect_err("a second workspace must not take a claimed namespace");
        assert!(
            matches!(err, ProvisionerError::SchemaNamespaceClaimed { .. }),
            "and it must be the claim that refuses it: {err}"
        );
    })
    .await;
}

// ── deprovision ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn deprovision_removes_the_remote_the_tenant_row_and_its_roles() {
    with_fx(|fx| async move {
        let tenant = fx
            .provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let writer = fx.writer("bookings");
        fx.ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("ensure writer");
        assert!(
            !fx.role_rows().await.is_empty(),
            "precondition: a role exists"
        );

        fx.provisioner
            .deprovision(fx.org_id)
            .await
            .expect("deprovision");

        assert!(fx.tenant_row().await.is_none(), "tenant row is gone");
        assert!(
            fx.provider
                .get_project(&tenant.project_id)
                .await
                .expect("get")
                .is_none(),
            "the remote project is gone — an orphan keeps costing money"
        );
        assert_eq!(
            OltpRoles::find()
                .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
                .all(&fx.db)
                .await
                .expect("query roles")
                .len(),
            0,
            "oltp_roles must cascade with the tenant"
        );

        // `deprovision` alone does NOT reclaim the writer role — that is the whole
        // reason `minted_roles` exists. Deprovisioning twice is the documented
        // no-op, and the role drop is `IF EXISTS`, so running full cleanup after
        // the assertions is safe and is what stops this test leaking one
        // cluster-global role per run.
        fx.cleanup().await;
    })
    .await;
}

/// Deprovisioning an org that never had a tenant is a no-op, not an error —
/// org deletion calls this unconditionally and must not fail on the common case.
#[tokio::test]
async fn deprovisioning_an_org_without_a_tenant_is_a_no_op() {
    with_fx(|fx| async move {
        fx.provisioner
            .deprovision(fx.org_id)
            .await
            .expect("no tenant is not an error");
    })
    .await;
}

// ── platform schema reconcile ───────────────────────────────────────────────

/// `reconcile_platform_schema` is called on every touch, so being free when
/// already current is what keeps that affordable — it must not wake a
/// scale-to-zero tenant to discover there is nothing to do.
#[tokio::test]
async fn reconciling_a_current_tenant_leaves_the_version_untouched() {
    with_fx(|fx| async move {
        let row = fx
            .provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");

        let again = fx
            .provisioner
            .reconcile_platform_schema(&row)
            .await
            .expect("reconcile");

        assert_eq!(again.platform_schema_version, row.platform_schema_version);
        assert_eq!(
            again.platform_schema_version,
            oxy_oltp::platform::PLATFORM_SCHEMA_VERSION
        );

        fx.cleanup().await;
    })
    .await;
}

/// A tenant left behind by an older release must catch up. Simulated by winding
/// the recorded version back — the same state a tenant provisioned before a new
/// platform step shipped would be in.
#[tokio::test]
async fn a_tenant_behind_the_platform_version_is_brought_forward() {
    with_fx(|fx| async move {
        let row = fx
            .provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");

        let mut stale: oltp_tenants::ActiveModel = row.clone().into();
        stale.platform_schema_version = ActiveValue::Set(0);
        let stale = stale.update(&fx.db).await.expect("wind back");
        assert_eq!(stale.platform_schema_version, 0);

        let caught_up = fx
            .provisioner
            .reconcile_platform_schema(&stale)
            .await
            .expect("reconcile");

        assert_eq!(
            caught_up.platform_schema_version,
            oxy_oltp::platform::PLATFORM_SCHEMA_VERSION,
            "every statement is idempotent precisely so replaying from 0 repairs \
             drift rather than erroring"
        );

        fx.cleanup().await;
    })
    .await;
}

/// A visibility choice has to outlive the call that made it.
///
/// It lived only as a GRANT inside the tenant, so every reader re-derived it
/// from the writer kind's default: a pipeline hidden with `expose --revoke` had
/// its grants reinstated by the next `apply`, and an opted-in app schema
/// stopped covering tables added later. The column, the migration and both
/// readers all landed in one commit and NOTHING wrote it — and the test that
/// shipped with them re-implemented the formula as a local closure, so it
/// passed while blind to exactly that.
///
/// This one goes through the provisioner and re-reads the row.
#[tokio::test]
async fn a_visibility_choice_is_persisted_not_re_derived() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let pipeline = WriterRef::pipeline("toast").unwrap();
        fx.provisioner
            .ensure_writer(
                fx.org_id,
                &pipeline,
                GrantLevel::ReadWrite,
                Some(fx.claimant),
            )
            .await
            .expect("ensure writer");

        let schema = pipeline.schema_name();
        async fn stored(fx: &Fx, schema: &str) -> Option<bool> {
            OltpRoles::find()
                .filter(oltp_roles::Column::SchemaName.eq(schema.to_string()))
                .one(&fx.db)
                .await
                .expect("query role")
                .expect("role row")
                .analytics_visible
        }

        // Provisioning applies the documented default for a pipeline and
        // RECORDS it, so later readers do not have to re-derive it. NULL is
        // reserved for rows created before the column existed.
        assert_eq!(
            stored(&fx, &schema).await,
            Some(true),
            "raw_* is visible by default, and the default is written down"
        );

        // Hiding it is the case that used to be silently undone.
        fx.provisioner
            .set_analytics_visibility(fx.org_id, &pipeline, false)
            .await
            .expect("hide");
        assert_eq!(
            stored(&fx, &schema).await,
            Some(false),
            "hiding must be recorded, or the next apply re-grants it"
        );

        // And re-provisioning must not re-expose it.
        fx.provisioner
            .ensure_writer(
                fx.org_id,
                &pipeline,
                GrantLevel::ReadWrite,
                Some(fx.claimant),
            )
            .await
            .expect("re-provision");
        assert_eq!(
            stored(&fx, &schema).await,
            Some(false),
            "the idempotent path must not undo an operator's choice"
        );

        fx.provisioner
            .set_analytics_visibility(fx.org_id, &pipeline, true)
            .await
            .expect("show");
        assert_eq!(stored(&fx, &schema).await, Some(true));
    })
    .await;
}

/// The two assumptions `custom_apps_migrations` rests on, against real Postgres.
///
/// That module runs an app's migration SQL as the app's OWN writer role, with
/// the search path pinned by the resolver, and says so in a comment:
///
/// > `search_path` is already pinned to the writer's schema by the resolver, so
/// > an unqualified `CREATE TABLE orders` lands in `app_<writer>`.
///
/// Both halves were reasoned from `oxy_oltp::schema`'s invariants and neither
/// had ever been executed — and both fail on the FIRST promote if wrong, which
/// is the worst time to find out:
///
///   * no `CREATE` in its own schema → every migration dies `permission denied`;
///   * an unpinned search path → the DDL aims at `public`, where the writer has
///     no rights, so it fails loudly but for a reason nobody would guess from
///     the error.
///
/// Asserted on the writer connection the runner actually resolves, not on a
/// superuser one, because a superuser would pass both regardless — which is
/// exactly how this would have looked verified while being untested.
#[tokio::test]
async fn an_app_writer_can_create_in_its_own_schema_and_unqualified_ddl_lands_there() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let writer = fx.writer("migprobe");
        fx.ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("ensure writer");

        // The runner's own resolution path, verbatim.
        let conn =
            oxy_oltp::resolver::resolve_writer_connection_for_org(&fx.db, fx.org_id, &writer)
                .await
                .expect("resolve writer connection");
        // The MECHANISM, not just its outcome. Without this the test still
        // passes if the resolver stops pinning the path and the writer's schema
        // happens to be first by some other route — and it would then go on
        // passing right up until that route changed too. Asserting the option
        // is present makes the test fail when the reason disappears, rather
        // than when the consequence finally does.
        assert!(
            conn.dsn.contains("search_path"),
            "the resolver must pin search_path; the migration runner relies on it \
             for unqualified DDL and says so in a comment"
        );

        let client = oxy_oltp::connect::connect(&conn.dsn, "migration assumption probe")
            .await
            .expect("connect as the writer");

        // UNQUALIFIED, exactly as an author's migration would write it.
        client
            .batch_execute("CREATE TABLE mig_probe (id integer primary key)")
            .await
            .expect("an app writer must hold CREATE inside its own schema");

        let schema = writer.schema_name();
        let landed: String = client
            .query_one(
                "SELECT table_schema::text FROM information_schema.tables \
                 WHERE table_name = 'mig_probe'",
                &[],
            )
            .await
            .expect("the table must exist somewhere")
            .get(0);

        assert_eq!(
            landed, schema,
            "unqualified DDL must land in the writer's own schema, not {landed}"
        );
        assert_ne!(
            landed, "public",
            "an unpinned search_path would aim every app migration at public"
        );

        fx.cleanup().await;
    })
    .await;
}
