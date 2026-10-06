//! Tier-B smoke test — drives the full /control/* flow against a *real*
//! Airhouse, provisions a fresh tenant, and verifies the row landed by
//! reading it back over pgwire.
//!
//! ## When this runs
//!
//! Marked `#[ignore]` so it never fires in the default `cargo nextest`
//! run. Invoke explicitly:
//!
//! ```bash
//! set -a; . internal_demo/.env; set +a   # AIRHOUSE_BASE_URL + friends
//! cargo nextest run -p oxy-cameras --test integration -E 'test(smoke_real_airhouse)' \
//!     --run-ignored only --no-capture
//! ```
//!
//! ## What this catches that Tier-A does not
//!
//! - Real `TenantProvisioner::provision` against the Airhouse admin API.
//! - The token broker minting an actual SA-backed ephemeral credential.
//! - The pgwire roundtrip: tokio-postgres + rustls + DuckLake.
//! - `CREATE TABLE IF NOT EXISTS` against DuckLake (catches any
//!   `PRIMARY KEY` / `UNIQUE` / `CREATE INDEX` that snuck back into the
//!   schema — DuckLake rejects all three).
//! - Multi-row VALUES INSERT via `simple_query` (no `$N` placeholders).
//! - That a Reader-role credential can read what a Writer-role insert wrote.
//!
//! ## Cleanup
//!
//! `TenantProvisioner::deprovision` runs after the test body, even on
//! failure (we wrap the test in a closure and run cleanup
//! unconditionally before unwrapping the result). If the test panics
//! mid-cleanup the tenant is orphaned — that's a manual followup, log
//! shows the workspace_id.

#![allow(clippy::expect_used)]

use airhouse::{
    AirhouseAdminClient, DEFAULT_INTERNAL_TTL, SystemPurpose, TenantProvisioner, UserRole,
    token_broker, wire_endpoint,
};
use axum::body::Body;
use axum::http::{Request, StatusCode, header::AUTHORIZATION};
use base64::{Engine, engine::general_purpose};
use chrono::Utc;
use entity::organizations;
use entity::workspaces::{self, WorkspaceStatus};
use migration::{Migrator, MigratorTrait};
use rustls::ClientConfig;
use sea_orm::{ActiveModelTrait, ActiveValue, ConnectionTrait, Database, DatabaseConnection, Set};
use serde_json::{Value, json};
use std::sync::Mutex;
use tokio_postgres_rustls::MakeRustlsConnect;
use tower::ServiceExt;
use uuid::Uuid;
use webpki_roots::TLS_SERVER_ROOTS;

use oxy_cameras::CamerasMigrator;
use oxy_cameras::entities::sites;
use oxy_cameras::routes;

// Required env vars — fail-fast if missing rather than silently skip,
// because someone who invokes `--run-ignored only` clearly *intends* to
// hit real Airhouse.
const REQUIRED_ENV: &[&str] = &[
    "AIRHOUSE_BASE_URL",
    "AIRHOUSE_ADMIN_TOKEN",
    "AIRHOUSE_WIRE_HOST",
    "AIRHOUSE_WIRE_PORT",
];

fn assert_env_set() {
    let missing: Vec<&&str> = REQUIRED_ENV
        .iter()
        .filter(|v| std::env::var(v).is_err() || std::env::var(v).unwrap().is_empty())
        .collect();
    if !missing.is_empty() {
        panic!(
            "Tier-B requires real Airhouse env vars. Missing: {missing:?}\n\
             Source a project .env that has them, e.g.:\n  \
             set -a; . internal_demo/.env; set +a"
        );
    }
}

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Same idea as the airhouse_provisioner test: the SA-bearer envelope
/// crypto needs a deterministic key during tests so the process doesn't
/// scatter `~/.local/share/oxy/encryption_key.txt` files.
fn set_test_encryption_key() {
    let _g = ENV_LOCK.lock().unwrap();
    // SAFETY: single-threaded under ENV_LOCK; deterministic value.
    unsafe {
        std::env::set_var(
            "OXY_ENCRYPTION_KEY",
            general_purpose::STANDARD.encode([7u8; 32]),
        );
    }
}

// ── Per-test app DB (same shape as Tier-A) ──────────────────────────────────

static TEST_DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static TEST_CONTAINER: tokio::sync::OnceCell<
    std::sync::Arc<testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>>,
> = tokio::sync::OnceCell::const_new();

async fn test_db() -> DatabaseConnection {
    let admin_url = TEST_DB_URL
        .get_or_init(|| async {
            if let Ok(url) = std::env::var("OXY_DATABASE_URL") {
                return url;
            }
            use testcontainers::runners::AsyncRunner;
            use testcontainers::{ImageExt, ReuseDirective};
            use testcontainers_modules::postgres::Postgres;

            let container = TEST_CONTAINER
                .get_or_init(|| async {
                    std::sync::Arc::new(
                        Postgres::default()
                            .with_tag("18-alpine")
                            // 64 MB (Docker default) is too small: a parallel plan wants a 32 MB
                            // DSM segment and a REUSED container accumulates them.
                            // Must match at every setup site — reuse hashes the config.
                            // See internal-docs/workspace-source.md.
                            .with_shm_size(1024 * 1024 * 1024)
                            .with_reuse(ReuseDirective::Always)
                            .start()
                            .await
                            .expect("start postgres testcontainer"),
                    )
                })
                .await;
            let port = container
                .get_host_port_ipv4(5432_u16)
                .await
                .expect("postgres port");
            format!("postgresql://postgres:postgres@127.0.0.1:{port}/postgres")
        })
        .await
        .clone();

    let admin = Database::connect(&admin_url).await.expect("admin connect");
    let db_name = format!("cameras_tierb_{}", Uuid::new_v4().simple());
    admin
        .execute_unprepared(&format!("CREATE DATABASE \"{db_name}\""))
        .await
        .expect("create per-test db");
    let test_url = match admin_url.rfind('/') {
        Some(pos) => format!("{}/{db_name}", &admin_url[..pos]),
        None => panic!("admin_url missing path: {admin_url}"),
    };

    // The broker calls `oxy_platform::db::establish_connection()` to read
    // the airhouse_tenants row, which reads OXY_DATABASE_URL globally and
    // caches the resulting pool. Point it at the per-test DB *before* the
    // first mint call lands. (Once the pool is initialised it sticks for
    // the rest of the process — that's fine because this whole binary is
    // dedicated to Tier B.)
    let _env_g = ENV_LOCK.lock().unwrap();
    // SAFETY: single-threaded under ENV_LOCK; only this test binary runs.
    unsafe { std::env::set_var("OXY_DATABASE_URL", &test_url) };
    drop(_env_g);

    let db = Database::connect(&test_url)
        .await
        .expect("per-test connect");

    Migrator::up(&db, None).await.expect("central migrations");
    airhouse::migration::up(&db)
        .await
        .expect("airhouse migrations");
    CamerasMigrator::up(&db, None)
        .await
        .expect("cameras migrations");
    db
}

async fn seed_workspace(db: &DatabaseConnection) -> Uuid {
    let now = Utc::now().fixed_offset();
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set("tierb-org".into()),
        slug: ActiveValue::Set(format!("tierb-{}", org_id.simple())),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed org");

    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set("tierb-ws".into()),
        git_namespace_id: ActiveValue::Set(None),
        git_remote_url: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        path: ActiveValue::Set(None),
        last_opened_at: ActiveValue::Set(None),
        created_by: ActiveValue::Set(None),
        org_id: ActiveValue::Set(Some(org_id)),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        error: ActiveValue::Set(None),
        monthly_vlm_budget_micros: ActiveValue::Set(None),
        current_revision_id: ActiveValue::Set(None),
        default_branch: ActiveValue::Set(None),
        repo_subdir: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed workspace");
    workspace_id
}

async fn seed_site(db: &DatabaseConnection, workspace_id: Uuid) -> Uuid {
    let now = Utc::now().fixed_offset();
    let site_id = Uuid::new_v4();
    sites::ActiveModel {
        id: Set(site_id),
        workspace_id: Set(workspace_id),
        name: Set("tierb-site".into()),
        timezone: Set("UTC".into()),
        region: Set(None),
        source: Set("manual".into()),
        public_ip: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .expect("seed site");
    site_id
}

// ── tokio-postgres reader using broker-minted Reader credential ────────────

async fn verify_event_landed(workspace_id: Uuid, event_id: Uuid) -> i64 {
    let broker = token_broker().expect("broker (env should be set)");
    let endpoint = wire_endpoint().expect("wire endpoint (env should be set)");
    let cred = broker
        .mint_for_system(
            workspace_id,
            SystemPurpose::EdgeIngest, // separate cache key per role; Reader below
            UserRole::Reader,
            DEFAULT_INTERNAL_TTL,
        )
        .await
        .expect("mint reader");

    // TLS the same way `cameras::airhouse::connect` does it. AIRHOUSE_OBS_INSECURE
    // would let us turn this off for localhost airhouse — opting in only here.
    let insecure = std::env::var("OXY_AIRHOUSE_OBS_INSECURE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    let mut pg = tokio_postgres::Config::new();
    pg.host(&endpoint.host);
    pg.port(endpoint.port);
    pg.user(&cred.username);
    pg.password(&cred.password);
    pg.dbname(&cred.tenant);

    let (client, conn_fut) = if insecure {
        let (c, conn) = pg
            .connect(tokio_postgres::NoTls)
            .await
            .expect("pgwire connect (insecure)");
        let conn_handle = tokio::spawn(async move {
            if let Err(e) = conn.await {
                eprintln!("pg conn err: {e}");
            }
        });
        (c, conn_handle)
    } else {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(TLS_SERVER_ROOTS.iter().cloned());
        let tls_config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let tls = MakeRustlsConnect::new(tls_config);
        let (c, conn) = pg.connect(tls).await.expect("pgwire connect (tls)");
        let conn_handle = tokio::spawn(async move {
            if let Err(e) = conn.await {
                eprintln!("pg conn err: {e}");
            }
        });
        (c, conn_handle)
    };

    // Simple-query — DuckLake doesn't speak `$N` placeholders.
    let sql = format!(
        "SELECT COUNT(*) FROM oxy_cam_events WHERE event_id = '{}'",
        event_id
    );
    let rows = client
        .simple_query(&sql)
        .await
        .expect("simple_query select");

    let mut count: i64 = -1;
    for msg in rows {
        if let tokio_postgres::SimpleQueryMessage::Row(r) = msg {
            if let Some(v) = r.get(0) {
                count = v.parse().unwrap_or(-1);
            }
        }
    }
    drop(client);
    let _ = conn_handle_drop(conn_fut).await; // best effort
    count
}

async fn conn_handle_drop(h: tokio::task::JoinHandle<()>) -> Result<(), tokio::task::JoinError> {
    h.abort();
    match h.await {
        Ok(()) => Ok(()),
        Err(e) if e.is_cancelled() => Ok(()),
        Err(e) => Err(e),
    }
}

/// Probe the tenant for the camera-fleet tables. Returns the set of
/// table names that exist. Uses a Reader credential — if the post-
/// provision hook ran, all 4 tables should be present even though we
/// haven't done any ingest yet.
async fn list_camera_tables(workspace_id: Uuid) -> std::collections::HashSet<String> {
    let broker = token_broker().expect("broker");
    let endpoint = wire_endpoint().expect("endpoint");
    let cred = broker
        .mint_for_system(
            workspace_id,
            SystemPurpose::EdgeIngest,
            UserRole::Reader,
            DEFAULT_INTERNAL_TTL,
        )
        .await
        .expect("mint reader");

    let insecure = std::env::var("OXY_AIRHOUSE_OBS_INSECURE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    let mut pg = tokio_postgres::Config::new();
    pg.host(&endpoint.host);
    pg.port(endpoint.port);
    pg.user(&cred.username);
    pg.password(&cred.password);
    pg.dbname(&cred.tenant);

    let (client, conn_handle) = if insecure {
        let (c, conn) = pg
            .connect(tokio_postgres::NoTls)
            .await
            .expect("pgwire (insecure)");
        let h = tokio::spawn(async move {
            if let Err(e) = conn.await {
                eprintln!("pg conn err: {e}");
            }
        });
        (c, h)
    } else {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(TLS_SERVER_ROOTS.iter().cloned());
        let tls_config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let tls = MakeRustlsConnect::new(tls_config);
        let (c, conn) = pg.connect(tls).await.expect("pgwire (tls)");
        let h = tokio::spawn(async move {
            if let Err(e) = conn.await {
                eprintln!("pg conn err: {e}");
            }
        });
        (c, h)
    };

    // information_schema.tables works in DuckDB / DuckLake. Filter to
    // our prefix so we don't pull the entire catalog.
    let rows = client
        .simple_query(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_name LIKE 'oxy_cam_%'",
        )
        .await
        .expect("information_schema query");

    let mut found = std::collections::HashSet::new();
    for msg in rows {
        if let tokio_postgres::SimpleQueryMessage::Row(r) = msg
            && let Some(v) = r.get(0)
        {
            found.insert(v.to_string());
        }
    }
    drop(client);
    let _ = conn_handle_drop(conn_handle).await;
    found
}

// ── Test ────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "Tier B: requires real Airhouse — run with --run-ignored only"]
async fn ingest_one_event_through_real_airhouse() {
    assert_env_set();
    set_test_encryption_key();
    // This Tier-B test verifies a row actually lands in real Airhouse, so it
    // needs the SYNCHRONOUS write path — the default write-coalescing buffer
    // would 202-ACK and flush asynchronously, racing `verify_event_landed`.
    // SAFETY: set before any Tokio work; #[ignore] Tier-B tests run individually
    // (nextest process-per-test), so there is no concurrent env reader.
    unsafe { std::env::set_var("OXY_CAMERAS_INGEST_BUFFER_DISABLED", "1") };

    let db = test_db().await;
    let workspace_id = seed_workspace(&db).await;
    let site_id = seed_site(&db, workspace_id).await;

    // ── Provision a fresh tenant on real Airhouse ──────────────────────────
    let admin_client = AirhouseAdminClient::new(
        std::env::var("AIRHOUSE_BASE_URL").unwrap(),
        std::env::var("AIRHOUSE_ADMIN_TOKEN").unwrap(),
    );
    let tenant_prov = TenantProvisioner::new(db.clone(), admin_client);
    let tenant_name = format!("oxy-cameras-tierb-{}", workspace_id.simple());
    tenant_prov
        .provision(workspace_id, tenant_name.clone())
        .await
        .expect("provision tenant");
    eprintln!("provisioned airhouse tenant: workspace_id={workspace_id} name={tenant_name}");

    // ── Run the camera flow ───────────────────────────────────────────────
    // Wrap in a closure so we can run deprovision unconditionally.
    // Same mounting shape as Tier-A: edge `/control/*` at the root,
    // operator workspace tree under `/{workspace_id}/...`. Production
    // wraps the latter in `workspace_middleware` — irrelevant here
    // since we already seeded the workspace and trust the URL.
    let app: axum::Router = axum::Router::new()
        .merge(routes::router::<()>(db.clone()))
        .nest(
            "/{workspace_id}",
            routes::workspace_routes::<()>(db.clone()),
        );
    let event_id = Uuid::new_v4();

    let result = async {
        // 1. Register
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/{workspace_id}/cameras/edge-boxes"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "site_id": site_id,
                            "hardware_model": "tierb-smoke",
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        if resp.status() != StatusCode::OK {
            return Err(format!("register failed: status={}", resp.status()));
        }
        let body_bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body_bytes).unwrap();
        let bearer = body["token"].as_str().expect("bearer").to_string();

        // 2. POST one event
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/events")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {bearer}"))
                    .body(Body::from(
                        json!({
                            "events": [{
                                "event_id":      event_id,
                                "ts":            Utc::now(),
                                "camera_id":     Uuid::new_v4(),
                                "event_type":    "tierb_test_event",
                                "track_id":      "tierb-track",
                                "dwell_seconds": 2.5,
                                "confidence":    0.99,
                            }]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        // 202 Accepted: the ingest handler ACKs before the (here synchronous, via
        // OXY_CAMERAS_INGEST_BUFFER_DISABLED above) airhouse write completes.
        if resp.status() != StatusCode::ACCEPTED {
            let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
                .await
                .unwrap();
            return Err(format!(
                "ingest failed: body={}",
                String::from_utf8_lossy(&bytes)
            ));
        }

        // 3. Verify via SELECT
        let count = verify_event_landed(workspace_id, event_id).await;
        if count != 1 {
            return Err(format!(
                "expected exactly 1 row for event_id={event_id}; got count={count}"
            ));
        }
        Ok::<(), String>(())
    }
    .await;

    // ── Deprovision regardless of outcome ─────────────────────────────────
    let dep_result = tenant_prov.deprovision(workspace_id).await;
    if let Err(e) = &dep_result {
        eprintln!(
            "WARN: deprovision failed (manual cleanup may be needed for workspace_id={workspace_id} tenant={tenant_name}): {e}"
        );
    }

    // ── Surface the original failure (if any) after cleanup ───────────────
    result.unwrap();
    dep_result.expect("deprovision tenant");
    eprintln!("✓ Tier-B passed: event {event_id} round-tripped through Airhouse");
}

/// Verifies the camera-fleet schema is **gated on camera intent**:
///
/// - **After `TenantProvisioner::provision`** (no camera-related
///   action taken yet) → the `oxy_cam_*` tables MUST NOT exist.
///   Provisioning Airhouse alone is "this workspace wants a data
///   warehouse"; it shouldn't pollute the tenant with empty tables
///   for features the workspace doesn't use.
///
/// - **After `register_edge_box`** (explicit camera intent) → all
///   four tables MUST exist. This is the soft `ensure_schema` trigger
///   inside `service::registration::register_edge_box` doing its job.
///
/// Regression catcher: if anyone re-introduces the unconditional
/// post-provision hook for cameras, the first half of this test
/// fails. If the camera-intent trigger ever stops firing, the second
/// half fails.
#[tokio::test]
#[ignore = "Tier B: requires real Airhouse — run with --run-ignored only"]
async fn ddl_gated_on_camera_intent() {
    assert_env_set();
    set_test_encryption_key();
    // This Tier-B test verifies a row actually lands in real Airhouse, so it
    // needs the SYNCHRONOUS write path — the default write-coalescing buffer
    // would 202-ACK and flush asynchronously, racing `verify_event_landed`.
    // SAFETY: set before any Tokio work; #[ignore] Tier-B tests run individually
    // (nextest process-per-test), so there is no concurrent env reader.
    unsafe { std::env::set_var("OXY_CAMERAS_INGEST_BUFFER_DISABLED", "1") };

    let db = test_db().await;
    let workspace_id = seed_workspace(&db).await;
    let site_id = seed_site(&db, workspace_id).await;

    let admin_client = AirhouseAdminClient::new(
        std::env::var("AIRHOUSE_BASE_URL").unwrap(),
        std::env::var("AIRHOUSE_ADMIN_TOKEN").unwrap(),
    );
    let tenant_prov = TenantProvisioner::new(db.clone(), admin_client);
    let tenant_name = format!("oxy-cameras-gating-{}", workspace_id.simple());
    tenant_prov
        .provision(workspace_id, tenant_name.clone())
        .await
        .expect("provision tenant");

    let assertion_result = async {
        // 1. Right after provision — no camera intent yet. Tables MUST NOT exist.
        let tables_post_provision = list_camera_tables(workspace_id).await;
        if !tables_post_provision.is_empty() {
            return Err(format!(
                "tenant should be empty of oxy_cam_* tables right after provision \
                 (camera intent not signalled yet); found: {tables_post_provision:?}"
            ));
        }

        // 2. Register an edge box via the public route — signals intent.
        let app: axum::Router = axum::Router::new()
            .merge(routes::router::<()>(db.clone()))
            .nest(
                "/{workspace_id}",
                routes::workspace_routes::<()>(db.clone()),
            );
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/{workspace_id}/cameras/edge-boxes"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "site_id": site_id,
                            "hardware_model": "tierb-gating",
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        if resp.status() != StatusCode::OK {
            return Err(format!("register failed: status={}", resp.status()));
        }

        // 3. Tables MUST now exist (via the soft ensure_schema trigger).
        let tables_post_register = list_camera_tables(workspace_id).await;
        let expected: std::collections::HashSet<String> = [
            "oxy_cam_events",
            "oxy_cam_camera_health",
            "oxy_cam_box_health",
            "oxy_cam_compliance_reports",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let missing: Vec<&String> = expected.difference(&tables_post_register).collect();
        if !missing.is_empty() {
            return Err(format!(
                "register_edge_box should have triggered ensure_schema; \
                 missing: {missing:?}. Found: {tables_post_register:?}"
            ));
        }
        Ok::<(), String>(())
    }
    .await;

    let dep_result = tenant_prov.deprovision(workspace_id).await;
    if let Err(e) = &dep_result {
        eprintln!("WARN: deprovision failed for workspace_id={workspace_id}: {e}");
    }
    assertion_result.unwrap();
    dep_result.expect("deprovision tenant");
    eprintln!("✓ Tier-B passed: DDL absent after provision, present after register_edge_box");
}
