//! DB-backed tests for the per-org OLTP plane.
//!
//! **Real Postgres, not a fake.** Every bug this subsystem has hit was a
//! Postgres *behaviour* bug — who may grant on whose tables, what
//! `information_schema` shows a SELECT-only role, whether DDL and its ledger
//! row commit together. A mock provider would have passed all of them.
//!
//! Needs its OWN throwaway Postgres — NOT `oxy start`'s.
//!
//! ```bash
//! just oltp-test-db   # a disposable postgres:18 on :15433
//! cargo nextest run -p oxy-oltp --test integration
//! ```
//!
//! **On :15433, deliberately off `oxy start`'s :15432.** This suite creates
//! and drops whole databases and mints cluster-global roles, and
//! `refuse_if_cluster_is_in_use` makes it refuse any cluster already holding
//! `oxy_org_*` tenants. `oxy start` now provisions per-org OLTP into its own
//! :15432 cluster by default, so sharing that port meant one Provision click
//! blocked this suite for good — and the printed escape hatch is the
//! unqualified `just oltp-down`. A separate port keeps the two from ever
//! colliding.
//!
//! Skips (rather than fails) when the cluster is absent, so `just test` on a
//! machine without Docker stays green — unless `OXY_OLTP_REQUIRE_DB=1` (which
//! CI sets), where absence is a red build.
//!
//! **That skip is load-bearing and once hid this suite from CI entirely.** The
//! DSN defaults to :15433 now, and CI runs a throwaway `oltp-postgres` service
//! on exactly that port and credentials (`ci.yaml`). Anything that changes the
//! default DSN has to change that service with it, or the suite goes quiet
//! again in the way that looks green.

mod branches_local;
mod grant_audit;
mod grants;
mod neon_live;

use std::sync::Arc;

use oxy_oltp::provider::{CreateProjectRequest, LocalProvider, OltpProvider};
use oxy_oltp::schema::{GrantLevel, WriterRef};
use oxy_oltp::sql::{PgSqlExecutor, TenantSqlExecutor};

/// Superuser DSN for this suite's OWN disposable cluster — NOT `oxy start`'s.
///
/// Defaults to :15433, off `oxy start`'s :15432, because the suite creates and
/// drops whole databases and mints cluster-global roles, and
/// `refuse_if_cluster_is_in_use` refuses any cluster already holding `oxy_org_*`
/// tenants. `oxy start` now provisions per-org OLTP into its own :15432 cluster
/// by default, so sharing the port meant one Provision click blocked the suite
/// for good. `just oltp-test-db` stands the :15433 cluster up; CI runs the
/// `oltp-postgres` service on the same port and credentials.
///
/// The platform fixture is the deliberate exception. `crates/app/tests/platform/
/// oltp_provisioner.rs` provisions through the real `OltpProvisioner`, so its
/// databases ARE `oxy_org_*` — it must stay on the control-plane cluster
/// (`OXY_DATABASE_URL`), because pointing it at :15433 would trip THIS suite's
/// guard the moment `--workspace` ran both. Its fixture doc records that.
///
/// Reads `OXY_OLTP_ADMIN_URL` so a developer can point it at another disposable
/// cluster; unset, it is :15433.
pub fn admin_dsn() -> String {
    std::env::var(oxy_oltp::config::OLTP_ADMIN_URL_VAR)
        .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost:15433/postgres".to_string())
}

/// `host:port` for the same cluster, derived rather than configured — a host
/// that disagreed with the DSN could only ever be a mistake.
pub fn host() -> String {
    oxy_oltp::provider::host_from_dsn(&admin_dsn())
}

/// Refuse to run against a cluster that already holds a provisioned tenant.
///
/// Collapsing `OXY_OLTP_TEST_ADMIN_URL` into the real variable removed a name
/// AND an opt-in: this suite creates and drops databases and mints a role in a
/// namespace that is still cluster-global for the analyst, and it now reads
/// whatever a developer exported for real provisioning.
///
/// **Cluster-scoped, via `pg_database`.** The first version probed
/// `information_schema.tables` for `oltp_tenants`, which only ever sees the
/// CURRENT database — so it fired only when the DSN happened to name the
/// control plane, and the default (`…/postgres`, used whenever the variable is
/// unset) walked straight past it onto the same cluster as a live tenant.
/// `pg_database` is shared cluster-wide and `oxy_org_<uuid>` is the product's
/// own derivation, so a hit is a live tenant whichever database the DSN names.
///
/// The `oltp_tenants` count stays as a second signal for the case where the DSN
/// IS the control plane — a tenant row with its database already dropped still
/// means someone is using this cluster.
pub async fn refuse_if_cluster_is_in_use() {
    let dsn = admin_dsn();
    let Ok(client) = oxy_oltp::connect::connect(&dsn, "oltp test guard").await else {
        // Unreachable cluster is the fixture's problem to report, not ours.
        return;
    };

    let tenant_dbs: i64 = client
        .query_one(
            r"SELECT count(*) FROM pg_database WHERE datname LIKE 'oxy\_org\_%'",
            &[],
        )
        .await
        .map(|r| r.get(0))
        .unwrap_or(0);

    // Only meaningful when the DSN names the control plane; absent elsewhere,
    // which is why it cannot be the primary check.
    let tenant_rows: i64 = match client
        .query_one(
            // Unfiltered by schema on purpose: an `oltp_tenants` in any schema
            // is a reason to stop, and being conservative here costs nothing.
            "SELECT count(*) FROM information_schema.tables WHERE table_name = 'oltp_tenants'",
            &[],
        )
        .await
        .map(|r| r.get::<_, i64>(0))
    {
        Ok(n) if n > 0 => client
            .query_one("SELECT count(*) FROM oltp_tenants", &[])
            .await
            .map(|r| r.get(0))
            .unwrap_or(0),
        _ => 0,
    };

    // Which cluster did we actually refuse — a dedicated one, or the control
    // plane the developer pointed `OXY_OLTP_ADMIN_URL` at? The remedy is
    // opposite in each case, so branch on what was observed rather than
    // assuming :15433.
    //
    // This only reaches the control-plane branch when the developer EXPORTED
    // `OXY_OLTP_ADMIN_URL` (nextest loads no `.env`, so a value that lives only
    // in `.env` reaches `oxy start` but not this process — the suite then
    // probes the :15433 default and takes the dedicated branch). Correct in
    // both, but the control-plane branch is the narrower path.
    let suite_host = host();
    let control_host = oxy_oltp::provider::host_from_dsn(
        &std::env::var("OXY_DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost:15432/oxy".to_string()),
    );
    let var = oxy_oltp::config::OLTP_ADMIN_URL_VAR;
    let remedy = if suite_host == control_host {
        // They aimed the suite at the control-plane cluster (what `.env.example`
        // recommends). `oltp-down` clears exactly this cluster, and is safe now
        // that it keeps `neon` rows.
        format!(
            "This IS the control-plane cluster ({suite_host}) — {var} points at it, and \
             `oxy start` provisions into it. Clear its LOCAL OLTP state with \
             `just oltp-down` (it keeps `neon` rows), or move the suite to a disposable \
             cluster: `just oltp-test-db` starts one on :15433, then `unset {var}`."
        )
    } else {
        // A dedicated cluster (the default :15433, or one they named). `oltp-down`
        // would hit a DIFFERENT cluster.
        format!(
            "This is a dedicated cluster ({suite_host}), not `oxy start`'s. Recreate it \
             fresh with `just oltp-test-db`. Do NOT run `just oltp-down`: it targets the \
             control-plane cluster (`OXY_DATABASE_URL`, {control_host}), a DIFFERENT one, \
             so it would delete real `oxy start` tenants — sealed passwords and all — and \
             leave this cluster untouched."
        )
    };
    assert!(
        tenant_dbs == 0 && tenant_rows == 0,
        "refusing to run the OLTP integration suite against {suite_host}, which is in use \
         ({tenant_dbs} oxy_org_* database(s), {tenant_rows} tenant row(s)). It creates and \
         drops databases and mints a cluster-scoped analyst role.\n\n{remedy}"
    );
}

/// A disposable database with the platform schema applied and writers created.
///
/// Named per test so cases can run concurrently without colliding on the
/// cluster-global role namespace — the limitation `LocalProvider` documents.
pub struct Fixture {
    pub provider: Arc<LocalProvider>,
    pub database: String,
    pub owner_dsn: String,
    pub owner_role: String,
    /// Suffix making role names unique across concurrently-running tests.
    /// Postgres roles are cluster-global while schemas are not — the exact
    /// limitation `LocalProvider` documents, and it bites tests first.
    suffix: String,
}

impl Fixture {
    /// The role's real name for this fixture's tenant — qualified, because the
    /// fixture's provider is `local` and roles are cluster-global.
    pub fn role_name(&self, writer: &WriterRef) -> String {
        oxy_oltp::schema::qualify_role(
            "local",
            &self.database,
            &writer.role_name(GrantLevel::ReadWrite),
        )
    }

    pub fn analyst_role(&self) -> String {
        oxy_oltp::schema::analyst_role_for("local", &self.database)
    }

    /// An app writer unique to this fixture.
    pub fn app(&self, base: &str) -> WriterRef {
        WriterRef::app(format!("{base}_{}", self.suffix)).expect("valid writer name")
    }

    /// A pipeline writer unique to this fixture.
    pub fn pipeline(&self, base: &str) -> WriterRef {
        WriterRef::pipeline(format!("{base}_{}", self.suffix)).expect("valid writer name")
    }
}

/// Set by CI to turn "no cluster" from a skip into a failure.
///
/// The skip below is what let this whole suite go quiet in CI while reporting
/// green. A comment saying "keep the service matching the default DSN" cannot
/// enforce that; this can. Where the variable is set, an unreachable cluster is
/// a red build naming the DSN it tried, which is the only signal that survives
/// someone changing one side and not the other.
pub(crate) const REQUIRE_DB_VAR: &str = "OXY_OLTP_REQUIRE_DB";

/// **Unset or explicitly off means optional; anything else means required, and
/// an unrecognised value panics.**
///
/// The obvious spelling — `matches!(var, Ok("1") | Ok("true"))` — fails OPEN,
/// which is the exact shape of the bug this variable exists to prevent.
/// `OXY_OLTP_REQUIRE_DB=TRUE`, `=yes`, `=on`, or `=1 ` with a space a YAML edit
/// left behind would all read as "not required", and the suite would go back to
/// skipping green. A switch whose whole job is to be loud must not be
/// disableable by typo, so the unknown value is the loudest outcome rather than
/// the quietest.
fn require_db() -> bool {
    match std::env::var(REQUIRE_DB_VAR) {
        Err(std::env::VarError::NotPresent) => false,
        // NOT folded into "optional": a mojibaked value is unreadable, and
        // unreadable must not mean may-skip — that is the fail-open shape the
        // rest of this function exists to remove.
        Err(e) => panic!("{REQUIRE_DB_VAR} is set but unreadable: {e}"),
        Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "" | "0" | "false" | "no" | "off" => false,
            "1" | "true" | "yes" | "on" => true,
            // `raw`, not the normalised form: this message exists to help find
            // a bad literal in a YAML file, and grepping for "maybe" will not
            // find `Maybe`.
            _ => panic!(
                "{REQUIRE_DB_VAR}={raw:?} is not a recognised boolean. \
                 Use 1/true/yes/on or 0/false/no/off — this switch decides \
                 whether the OLTP suite may skip, so an unreadable value must \
                 not quietly mean 'may skip'."
            ),
        },
    }
}

impl Fixture {
    /// `None` when the demo cluster is not running — the caller skips, unless
    /// [`REQUIRE_DB_VAR`] says the cluster is mandatory.
    pub async fn create(name: &str) -> Option<Self> {
        // Parsed BEFORE the probe, so the value is validated on every run.
        //
        // Called inside the failure arm instead, a typo (`=tru`) was invisible
        // while the cluster was up — never parsed — and surfaced only on the
        // day the DSN and the service drifted apart. Worse, on that day
        // `require_db()` panicked while evaluating the assert's condition, so
        // the connection error below never printed: the two fixes shadowed
        // each other in the one case that wants both.
        let required = require_db();

        // `crate::connect::connect`, NOT `tokio_postgres::connect(.., NoTls)`.
        //
        // A probe that gates a suite must never be WEAKER than what the suite
        // connects with, and it was: this path resolves a DSN with no
        // `sslmode` to `prefer` — TLS, falling back to plaintext — while the
        // old `NoTls` probe could not reach a cluster REQUIRING TLS
        // (`hostssl`, `rds.force_ssl`, an image shipping `ssl = on`). Such a
        // cluster skipped the suite green while every test in it would have
        // connected perfectly well.
        //
        // It is not yet the SAME path for everything, and the difference is in
        // the safe direction. `LocalProvider` still opens its admin
        // connections with `NoTls`, so on a TLS-only cluster this probe
        // succeeds and `create_project` fails a moment later — a real failure
        // rather than a silent skip, which is the point, but a reader should
        // not take "one path" as an invariant. Routing `LocalProvider` through
        // `connect::connect` is what would make it one, and is the same change
        // that would close `sslmode_for`'s plaintext-only local DSN.
        if let Err(e) = oxy_oltp::connect::connect(&admin_dsn(), "oltp fixture probe").await {
            // The cause, not just the fact. A TLS handshake failure, a rejected
            // password, a missing database and a refused connection are four
            // different fixes, and this panic is the only output of the red
            // build the variable exists to produce.
            assert!(
                !required,
                "{REQUIRE_DB_VAR} is set but no usable Postgres at {} \
                 (DSN from {}): {e}. This suite must not skip here: it is the \
                 only coverage of the confinement checks and the grant \
                 behaviour.",
                host(),
                oxy_oltp::config::OLTP_ADMIN_URL_VAR
            );
            eprintln!(
                "skipping: no Postgres at {} — run `oxy start --db-only`",
                host()
            );
            return None;
        }
        // Every fixture goes through here, so this is the one place the guard
        // has to be.
        refuse_if_cluster_is_in_use().await;

        let provider = Arc::new(LocalProvider::new(admin_dsn(), host()));
        let project_name = format!("oxytest-{name}");
        let db_name = oxy_oltp::provider::database_name_for(&project_name);

        // Previous run may have died mid-test.
        let _ = provider.delete_project(&db_name).await;

        let project = provider
            .create_project(CreateProjectRequest {
                name: project_name,
                region_id: "local".into(),
                pg_version: oxy_oltp::config::DEFAULT_PG_VERSION,
            })
            .await
            .expect("create project");

        let owner_password = project.owner_role.password.clone().expect("owner password");
        let owner_dsn = format!(
            "postgres://{}:{}@{}/{}?sslmode=disable",
            project.owner_role.name,
            // Encoded, like production: generated passwords contain URI-special
            // characters, and an unescaped `@` makes libpq read the rest as the
            // host — which surfaces as "error connecting to server".
            oxy_oltp::roles::encode_userinfo(&owner_password),
            host(),
            db_name
        );

        // Platform steps from 0 — the same call production makes.
        PgSqlExecutor
            .execute_batch(
                &owner_dsn,
                &oxy_oltp::platform::statements_since(0, "local", &db_name).expect("platform sql"),
            )
            .await
            .expect("apply platform schema");

        Some(Self {
            provider,
            database: db_name,
            owner_dsn,
            owner_role: project.owner_role.name,
            suffix: name.to_string(),
        })
    }

    /// Create a writer's role, schema and grants, returning its DSN.
    pub async fn add_writer(&self, writer: &WriterRef) -> String {
        // Qualified — the same name `ensure_writer_sql` grants to. Creating the
        // bare one and granting to the qualified one is the mismatch this
        // fixture hit first.
        let role_name = self.role_name(writer);
        let created = self
            .provider
            .create_role(&self.database, "local", &role_name)
            .await
            .expect("create role");
        let password = created.password.expect("role password");

        PgSqlExecutor
            .execute_batch(
                &self.owner_dsn,
                &oxy_oltp::schema::ensure_writer_sql(
                    writer,
                    GrantLevel::ReadWrite,
                    &self.owner_role,
                    &self.role_name(writer),
                )
                .expect("writer sql"),
            )
            .await
            .expect("apply writer grants");

        // Production's `apply_writer_ddl` adds this for pipelines and the
        // fixture did not, so a pipeline writer here could not create a schema
        // — `42501 permission denied for database`. Postgres checks CREATE on
        // the DATABASE before the schema, and Airway creates its dataset schema
        // on every load. Without it the squat test failed on its own premise
        // rather than on what it asserts.
        if matches!(writer, WriterRef::Pipeline(_)) {
            PgSqlExecutor
                .execute_batch(
                    &self.owner_dsn,
                    &[oxy_oltp::roles::grant_schema_creation_sql(&role_name)
                        .expect("schema creation grant")],
                )
                .await
                .expect("grant schema creation to the pipeline writer");
        }

        oxy_oltp::schema::with_search_path(
            &format!(
                "postgres://{}:{}@{}/{}?sslmode=disable",
                role_name,
                oxy_oltp::roles::encode_userinfo(&password),
                host(),
                self.database
            ),
            writer,
        )
    }

    /// Run SQL as the database owner — stands in for a migration.
    pub async fn as_owner(&self, sql: &str) {
        PgSqlExecutor
            .execute_batch(&self.owner_dsn, &[sql.to_string()])
            .await
            .expect("owner sql");
    }

    /// Drops the database **and** every role this fixture created. Roles are
    /// cluster-global, so leaving them behind makes the next run collide.
    pub async fn cleanup(&self, writers: &[&WriterRef]) {
        // QUALIFIED names, via the fixture's own helpers.
        //
        // These passed `w.role_name(..)` and the bare `ANALYST_ROLE` while
        // everything that creates roles here uses `role_name()` /
        // `analyst_role()`, which qualify with the database. `delete_role`
        // issues `DROP ROLE IF EXISTS "<name>"` verbatim, so both calls named
        // roles that never existed and succeeded at dropping nothing — leaving
        // the real, cluster-global ones behind on every green run. Harmless in
        // a container that dies with the job; not harmless on a shared cluster,
        // which is where a `CREATE ROLE` then meets a leftover it has no ADMIN
        // option on.
        for w in writers {
            let _ = self
                .provider
                .delete_role(&self.database, "local", &self.role_name(w))
                .await;
        }
        let _ = self
            .provider
            .delete_role(&self.database, "local", &self.analyst_role())
            .await;
        let _ = self.provider.delete_project(&self.database).await;
    }
}

/// Whether `dsn` can run `sql`. Used to assert a boundary rather than to
/// fetch data, so the error text matters more than the rows.
pub async fn attempt(dsn: &str, sql: &str) -> Result<(), String> {
    let (client, connection) = tokio_postgres::connect(dsn, tokio_postgres::NoTls)
        .await
        .map_err(|e| e.to_string())?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client.batch_execute(sql).await.map_err(render_pg_error)
}

/// `tokio_postgres::Error`'s Display is just "db error" — the message a human
/// needs lives in the DbError. Without this every assertion failure reads the
/// same.
pub fn render_pg_error(e: tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => format!("{}: {}", db.code().code(), db.message()),
        None => e.to_string(),
    }
}
