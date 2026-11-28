//! Local-mode seeding for the per-org OLTP POC.
//!
//! Mirrors `oxy-server`'s `commands::local_seed` (was `airhouse::local_seed`), which seeds the rows the provision flow needs
//! so local mode works untouched. This one goes a step further and produces a
//! **real, queryable Postgres**: the demo runs the same DDL generators as
//! production ([`crate::platform`], [`crate::schema`]), so what you query in the
//! IDE is the actual role model, not a mock-up of it.
//!
//! Deliberately does **not** touch Oxy's control plane. `OltpProvisioner` needs
//! `organizations` rows and a migrated database; this needs only a running
//! Postgres, which keeps `cargo run --example seed_local` a one-liner.
//!
//! Idempotent: safe to re-run. Pass `reset: true` to drop and rebuild.

use uuid::Uuid;

use crate::platform;
use crate::provider::{CreateProjectRequest, LocalProvider, OltpProvider, ProviderError};
use crate::provisioner::project_name_for;
use crate::schema::{GrantLevel, WriterRef};
use crate::sql::{PgSqlExecutor, TenantSqlExecutor};

#[derive(Debug, thiserror::Error)]
pub enum SeedError {
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("sql error: {0}")]
    Sql(#[from] crate::sql::SqlError),
    #[error("schema error: {0}")]
    Schema(#[from] crate::schema::SchemaError),
}

/// One seeded writer and how to connect as it.
#[derive(Debug, Clone)]
pub struct SeededWriter {
    pub writer: WriterRef,
    pub schema: String,
    pub role: String,
    pub dsn: String,
    /// Whether the analyst role can read this schema.
    pub analytics_visible: bool,
}

/// Everything the demo produced.
#[derive(Debug, Clone)]
pub struct SeededTenant {
    pub org_id: Uuid,
    pub database: String,
    pub host: String,
    pub owner_dsn: String,
    pub analyst_role: String,
    pub analyst_dsn: String,
    /// Carried, not recovered from `analyst_dsn`.
    ///
    /// `config_yml_block` used to parse it back out with
    /// `split(':').nth(2)` — which truncates the moment a drawn password
    /// contains `:` or `@`, emitting a config.yml holding a credential that
    /// does not authenticate. Third variant of "parse userinfo from the left"
    /// in this crate; the value was in hand all along.
    pub analyst_password: String,
    pub writers: Vec<SeededWriter>,
}

/// Provision a demo tenant on the local cluster and fill it with the restaurant
/// example from the design doc.
///
/// `expose_app_to_analytics` demonstrates the opt-in: `raw_*` schemas are
/// analyst-readable by default, `app_*` schemas are not. Pass `true` to see the
/// app's tables from the IDE as well.
pub async fn seed_local_demo(
    provider: &LocalProvider,
    org_id: Uuid,
    reset: bool,
    expose_app_to_analytics: bool,
) -> Result<SeededTenant, SeedError> {
    let project_name = project_name_for(org_id);
    let db_name = crate::provider::database_name_for(&project_name);

    if reset {
        provider.delete_project(&db_name).await?;
    }

    // The seed refuses where `create_project` would ADOPT.
    //
    // Deliberate, and the two answers differ for the same input now. The
    // seeder's name is derived (`project_name_for` above), so the provider
    // would take the existing database over and reset its owner password —
    // right for a half-provisioned tenant, wrong for a seed, which then layers
    // demo data onto whatever was already there. Refusing and naming `reset` is
    // the boring behaviour for a developer command; adoption is the boring
    // behaviour for a provisioner retry.
    //
    // There is a third answer now, and it is better than this guard intended:
    // `get_project` returns `None` for a database that exists and is NOT ours,
    // so a foreign database falls through here and `create_project` refuses
    // with `ProjectNotOwned`, naming the owner. `None` does not mean "absent".
    let project = match provider.get_project(&db_name).await? {
        Some(_) => {
            return Err(SeedError::Provider(ProviderError::ProjectNameTaken(
                format!("{db_name} (already seeded; re-run with reset to rebuild)"),
            )));
        }
        None => {
            provider
                .create_project(CreateProjectRequest {
                    name: project_name,
                    region_id: "local".to_string(),
                    pg_version: 18,
                })
                .await?
        }
    };

    let owner_password = project
        .owner_role
        .password
        .clone()
        .expect("local provider discloses the owner password on create");
    let owner_dsn = dsn(
        &project.host,
        &db_name,
        &project.owner_role.name,
        &owner_password,
    );

    let sql = PgSqlExecutor;

    // 1. Everything Oxy owns inside a tenant — the same declaration production
    //    uses, applied from version 0.
    sql.execute_batch(
        &owner_dsn,
        &platform::statements_since(0, "local", &db_name)?,
    )
    .await?;

    // 2. Writers: an Airway pipeline and a custom app.
    let mut writers = Vec::new();
    for (writer, tables) in [
        (WriterRef::pipeline("toast")?, TOAST_TABLES),
        (WriterRef::app("bookings")?, BOOKINGS_TABLES),
    ] {
        let role_name = crate::schema::qualify_role(
            "local",
            &db_name,
            &writer.role_name(GrantLevel::ReadWrite),
        );
        let analyst_role = crate::schema::analyst_role_for("local", &db_name);
        let created = provider
            .create_role(&project.id, &project.branch.id, &role_name)
            .await?;
        let password = created
            .password
            .expect("local provider discloses the role password");

        sql.execute_batch(
            &owner_dsn,
            &crate::schema::ensure_writer_sql(
                &writer,
                GrantLevel::ReadWrite,
                &project.owner_role.name,
                &role_name,
            )?,
        )
        .await?;

        // Create the demo tables **as the writer**, which is the real test of
        // the model: it proves the role can do DDL in its schema and that the
        // DSN-borne `search_path` resolves unqualified names there.
        let writer_dsn = crate::schema::with_search_path(
            &dsn(&project.host, &db_name, &role_name, &password),
            &writer,
        );
        let statements: Vec<String> = tables.iter().map(|s| (*s).to_string()).collect();
        sql.execute_batch(&writer_dsn, &statements).await?;

        let visible = writer.analytics_visible_by_default() || expose_app_to_analytics;
        if visible {
            // Two connections, because two roles own the objects: the database
            // owner owns the schema, the writer owns the tables it created.
            sql.execute_batch(
                &owner_dsn,
                &crate::schema::grant_analyst_schema_sql(&writer, &analyst_role),
            )
            .await?;
            sql.execute_batch(
                &writer_dsn,
                &crate::schema::grant_analyst_tables_sql(&writer, &analyst_role),
            )
            .await?;
        }

        writers.push(SeededWriter {
            schema: writer.schema_name(),
            role: role_name,
            dsn: writer_dsn,
            analytics_visible: visible,
            writer,
        });
    }

    // 3. The analyst login. `platform` created the role NOLOGIN; the provider
    //    mints its credential, exactly as Neon would.
    let analyst_name = crate::schema::analyst_role_for("local", &db_name);
    let analyst = provider
        .create_role(&project.id, &project.branch.id, &analyst_name)
        .await?;
    let analyst_password = analyst
        .password
        .expect("local provider discloses the role password");
    let analyst_dsn = dsn(&project.host, &db_name, &analyst_name, &analyst_password);

    Ok(SeededTenant {
        org_id,
        database: db_name,
        host: project.host,
        owner_dsn,
        analyst_role: analyst_name.clone(),
        analyst_password: analyst_password.clone(),
        analyst_dsn,
        writers,
    })
}

impl SeededTenant {
    /// A `config.yml` block that points Oxy's IDE query interface at this
    /// database as the read-only analyst.
    ///
    /// Uses `type: postgres` rather than `postgres_managed` because the managed
    /// variant resolves credentials from `oltp_tenants`, which the POC seed
    /// deliberately does not populate. Same connection, same role, same grants
    /// — just resolved statically.
    pub fn config_yml_block(&self) -> String {
        let (host, port) = match self.host.split_once(':') {
            Some((h, p)) => (h.to_string(), p.to_string()),
            None => (self.host.clone(), "5432".to_string()),
        };
        let password = self.analyst_password.clone();
        format!(
            "databases:\n  \
             - name: oltp\n    \
               type: postgres\n    \
               host: {host}\n    \
               port: \"{port}\"\n    \
               user: {analyst}\n    \
               password: {password}\n    \
               database: {db}\n",
            analyst = self.analyst_role,
            db = self.database,
        )
    }
}

fn dsn(host: &str, db: &str, user: &str, password: &str) -> String {
    // sslmode=disable: local cluster, no TLS. Production DSNs require it.
    // Encoded like every other builder — seed passwords are drawn too.
    format!(
        "postgres://{user}:{}@{host}/{db}?sslmode=disable",
        crate::roles::encode_userinfo(password)
    )
}

/// Airway-landed ETL data — analyst-readable by default.
const TOAST_TABLES: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS sales (
         id          BIGINT PRIMARY KEY,
         business_date DATE NOT NULL,
         net_sales   NUMERIC(12,2) NOT NULL,
         location    TEXT NOT NULL
     )",
    "INSERT INTO sales (id, business_date, net_sales, location) VALUES
         (1, DATE '2026-08-01', 4210.55, 'downtown'),
         (2, DATE '2026-08-02', 3980.10, 'downtown'),
         (3, DATE '2026-08-01', 2015.00, 'airport')
     ON CONFLICT (id) DO NOTHING",
];

/// Live application state — hidden from the analyst unless opted in.
const BOOKINGS_TABLES: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS orders (
         id        BIGSERIAL PRIMARY KEY,
         table_no  INTEGER NOT NULL,
         status    TEXT NOT NULL,
         placed_at TIMESTAMPTZ NOT NULL DEFAULT now()
     )",
    "CREATE TABLE IF NOT EXISTS order_items (
         id       BIGSERIAL PRIMARY KEY,
         order_id BIGINT NOT NULL REFERENCES orders(id),
         sku      TEXT NOT NULL,
         qty      INTEGER NOT NULL
     )",
    "CREATE TABLE IF NOT EXISTS inventory (
         sku     TEXT PRIMARY KEY,
         on_hand INTEGER NOT NULL
     )",
    "INSERT INTO inventory (sku, on_hand) VALUES
         ('poke-bowl', 40), ('miso-soup', 90), ('green-tea', 200)
     ON CONFLICT (sku) DO NOTHING",
    "INSERT INTO orders (table_no, status) SELECT 12, 'open'
     WHERE NOT EXISTS (SELECT 1 FROM orders)",
    "INSERT INTO order_items (order_id, sku, qty)
     SELECT o.id, 'poke-bowl', 2 FROM orders o
     WHERE NOT EXISTS (SELECT 1 FROM order_items)",
];
