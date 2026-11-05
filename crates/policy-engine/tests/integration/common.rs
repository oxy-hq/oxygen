//! A per-org OLTP tenant per test, built the way production builds one, on a
//! fresh Postgres container standing in for Neon:
//!
//! - `LocalProvider::create_project` makes the tenant database and its owner;
//! - `create_role` and `ensure_writer_sql` make the app writer and give it
//!   `app_pricing`, and nothing in `public`;
//! - `with_search_path` pins the writer's DSN to that schema, as the resolver's
//!   `WriterConnection` does.
//!
//! The writer then creates and seeds `policy_overrides` itself, unqualified, the
//! way an app's migration would. Nothing here touches `crates/migration`: the
//! table belongs to the app's schema, never to Oxy's own database.

use std::path::Path;

use oxy_oltp::provider::{CreateProjectRequest, LocalProvider, OltpProvider, database_name_for};
use oxy_oltp::resolver::WriterConnection;
use oxy_oltp::roles::encode_userinfo;
use oxy_oltp::schema::{GrantLevel, WriterRef, ensure_writer_sql, qualify_role, with_search_path};
use oxy_oltp::sql::{PgSqlExecutor, TenantSqlExecutor};
use oxy_policy_engine::{OltpWriter, PolicyEngine, PolicySource};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::postgres::Postgres;
use tokio_postgres::Client;

pub const POLICY_JS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/js/policy.js");

/// The cluster superuser's password: a quote and a percent sign, so URL
/// decoding is exercised on the provider's admin connection.
const SUPERUSER_PASSWORD: &str = "pw'with%quote";

/// The app whose schema holds the table.
pub const APP: &str = "pricing";
pub const SCHEMA: &str = "app_pricing";

pub const SPRING_SALE: &str = r#"{
  "id": "spring_sale",
  "variants": [
    { "name": "control", "weight": 1, "priceMultiplier": 1.0 },
    { "name": "deep_discount", "weight": 3, "priceMultiplier": 0.8 }
  ],
  "bounds": { "minMultiplier": 0.75, "maxMultiplier": 1.1 }
}"#;

pub struct Tenant {
    // Held so the container lives as long as the test.
    pub container: ContainerAsync<Postgres>,
    /// Superuser, on the tenant database: for inspecting locks and planting
    /// decoys, never for the policy's own reads or writes.
    pub admin: Client,
    /// The app writer, as `oxy_oltp`'s resolver hands it out.
    pub writer: OltpWriter,
    /// Minted by the provider (CSPRNG, URI-special characters included).
    pub writer_password: String,
    /// A session as the writer — the app editing its own table.
    pub app: Client,
}

pub async fn start() -> Tenant {
    let container = Postgres::default()
        .with_password(SUPERUSER_PASSWORD)
        .with_tag("18-alpine")
        // The default 60 s is not enough on a loaded machine running several
        // of these at once.
        .with_startup_timeout(std::time::Duration::from_secs(240))
        .start()
        .await
        .expect("start Postgres testcontainer — is Docker running?");
    let port = container
        .get_host_port_ipv4(5432_u16)
        .await
        .expect("mapped port");
    let host = format!("127.0.0.1:{port}");
    let superuser = |db: &str| {
        format!(
            "postgresql://postgres:{}@{host}/{db}?sslmode=disable",
            urlencoding::encode(SUPERUSER_PASSWORD)
        )
    };

    let provider = LocalProvider::new(superuser("postgres"), host.clone());
    let project_name = "policy-engine";
    let db = database_name_for(project_name);
    let project = provider
        .create_project(CreateProjectRequest {
            name: project_name.into(),
            region_id: "local".into(),
            pg_version: oxy_oltp::config::DEFAULT_PG_VERSION,
        })
        .await
        .expect("create the tenant database");
    let owner = &project.owner_role;
    let owner_dsn = format!(
        "postgres://{}:{}@{host}/{db}?sslmode=disable",
        owner.name,
        encode_userinfo(owner.password.as_deref().expect("owner password"))
    );

    let writer = WriterRef::app(APP).expect("valid slug");
    let role = qualify_role("local", &db, &writer.role_name(GrantLevel::ReadWrite));
    let writer_password = provider
        .create_role(&db, "local", &role)
        .await
        .expect("create the writer role")
        .password
        .expect("writer password");
    PgSqlExecutor
        .execute_batch(
            &owner_dsn,
            &ensure_writer_sql(&writer, GrantLevel::ReadWrite, &owner.name, &role)
                .expect("writer sql"),
        )
        .await
        .expect("give the writer its schema");
    let dsn = with_search_path(
        &format!(
            "postgres://{role}:{}@{host}/{db}?sslmode=disable",
            encode_userinfo(&writer_password)
        ),
        &writer,
    );
    let writer = OltpWriter::from(WriterConnection {
        schema: writer.schema_name(),
        role,
        dsn,
        verify_tls: false,
    });
    assert_eq!(writer.schema(), SCHEMA);

    let app = connect(&writer).await;
    app.batch_execute(
        "CREATE TABLE policy_overrides (
             policy_id  text PRIMARY KEY,
             policy     jsonb NOT NULL,
             updated_at timestamptz NOT NULL DEFAULT now()
         )",
    )
    .await
    .expect("the writer creates policy_overrides");
    upsert(&app, "spring_sale", SPRING_SALE).await;

    let admin = oxy_oltp::connect::connect(&superuser(&db), "test admin")
        .await
        .expect("superuser on the tenant database");
    // The premise every test rests on: the unqualified CREATE landed in the
    // writer's schema and nowhere else.
    let schemas: Vec<String> = admin
        .query(
            "SELECT table_schema::text FROM information_schema.tables
             WHERE table_name = 'policy_overrides'",
            &[],
        )
        .await
        .expect("information_schema")
        .iter()
        .map(|r| r.get(0))
        .collect();
    assert_eq!(schemas, vec![SCHEMA.to_string()]);

    Tenant {
        container,
        admin,
        writer,
        writer_password,
        app,
    }
}

/// A new session as the writer.
pub async fn connect(writer: &OltpWriter) -> Client {
    oxy_oltp::connect::connect(writer.dsn(), "test writer")
        .await
        .expect("connect as the writer")
}

pub async fn upsert(client: &Client, id: &str, policy: &str) {
    client
        .execute(
            "INSERT INTO policy_overrides (policy_id, policy) VALUES ($1, $2::text::jsonb)
             ON CONFLICT (policy_id) DO UPDATE SET policy = EXCLUDED.policy, updated_at = now()",
            &[&id, &policy],
        )
        .await
        .expect("upsert policy");
}

pub async fn engine(t: &Tenant) -> PolicyEngine {
    let source = PolicySource::connect(&t.writer)
        .await
        .expect("connect as the writer");
    PolicyEngine::boot(Path::new(POLICY_JS), source)
        .await
        .expect("boots")
}
