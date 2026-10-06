//! Tier-A wire smoke test for the camera-fleet control plane.
//!
//! Drives the real `/control/*` axum router via `tower::oneshot` against
//! a `testcontainers` Postgres. Migrators run end-to-end (central +
//! airhouse + cameras), so the schema is exactly what production sees.
//!
//! ## What this test catches
//!
//! - Device-token middleware (valid / missing / bad / revoked bearers)
//! - Token round-trip: `POST /control/register` → bearer → middleware
//!   lookup → `EdgeContext` resolution
//! - Route mounting + DTO wire shapes (the JSON the Python edge sends is
//!   what our handlers accept)
//! - Service layer dispatch with the right workspace_id / box_id
//! - Cross-box isolation: a valid bearer for box A cannot read box B's
//!   config
//!
//! ## What this test does NOT catch
//!
//! - Real Airhouse INSERTs (the Tier-B walkthrough doc covers that;
//!   here the broker is unconfigured, so `connect_and_ensure` returns
//!   `AirhouseError::Disabled`, which maps to 503 — **that's the
//!   success signal** for this layer: it proves we reached the
//!   service-layer boundary correctly).
//! - Pgwire / DuckLake schema-compatibility issues
//! - Token broker / SA mint behavior
//!
//! Run with `cargo nextest run -p oxy-cameras --test integration -E 'test(smoke_e2e)'`.

use axum::body::Body;
use axum::http::{Request, StatusCode, header::AUTHORIZATION};
use axum::response::Response;
use chrono::Utc;
use entity::organizations;
use entity::workspaces::{self, WorkspaceStatus};
use migration::{Migrator, MigratorTrait};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, Database, DatabaseConnection,
    EntityTrait, QueryFilter, Set,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use oxy_cameras::CamerasMigrator;
use oxy_cameras::entities::{cameras, sites};
use oxy_cameras::routes;

// ── Shared postgres testcontainer (one per `nextest` process tree) ──────────

static TEST_DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static TEST_CONTAINER: tokio::sync::OnceCell<
    std::sync::Arc<testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>>,
> = tokio::sync::OnceCell::const_new();

/// Create a fresh per-test database, run every migrator the production
/// stack runs, and return a connection scoped to that DB.
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
                            .expect("start postgres testcontainer (is Docker running?)"),
                    )
                })
                .await;
            let port = container
                .get_host_port_ipv4(5432_u16)
                .await
                .expect("get postgres port");
            format!("postgresql://postgres:postgres@127.0.0.1:{port}/postgres")
        })
        .await
        .clone();

    let admin = {
        let mut last_err = None;
        let mut conn = None;
        for attempt in 0..10 {
            match Database::connect(&admin_url).await {
                Ok(c) => {
                    conn = Some(c);
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    if attempt < 9 {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                }
            }
        }
        conn.unwrap_or_else(|| panic!("connect admin: {:?}", last_err))
    };

    let db_name = format!("cameras_smoke_{}", Uuid::new_v4().simple());
    admin
        .execute_unprepared(&format!("CREATE DATABASE \"{db_name}\""))
        .await
        .expect("create per-test database");

    let test_url = match admin_url.rfind('/') {
        Some(pos) => format!("{}/{db_name}", &admin_url[..pos]),
        None => panic!("admin_url missing path: {admin_url}"),
    };
    let db = Database::connect(&test_url)
        .await
        .expect("connect to per-test database");

    // Production runs three migrators in this order. Mirror that.
    Migrator::up(&db, None).await.expect("central migrations");
    airhouse::migration::up(&db)
        .await
        .expect("airhouse migrations");
    CamerasMigrator::up(&db, None)
        .await
        .expect("cameras migrations");
    db
}

// ── Seed helpers ────────────────────────────────────────────────────────────

/// Seed an org + workspace + site. Returns (workspace_id, site_id).
async fn seed_site(db: &DatabaseConnection) -> (Uuid, Uuid) {
    let now = Utc::now().fixed_offset();

    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set("smoke-org".into()),
        slug: ActiveValue::Set(format!("smoke-{}", org_id.simple())),
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
        name: ActiveValue::Set("smoke-ws".into()),
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

    let site_id = Uuid::new_v4();
    sites::ActiveModel {
        id: Set(site_id),
        workspace_id: Set(workspace_id),
        name: Set("smoke-site".into()),
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

    (workspace_id, site_id)
}

/// Seed a camera bound to a given site (and optionally an edge_box).
async fn seed_camera(
    db: &DatabaseConnection,
    site_id: Uuid,
    edge_box_id: Option<Uuid>,
    name: &str,
) -> Uuid {
    let now = Utc::now().fixed_offset();
    let camera_id = Uuid::new_v4();
    cameras::ActiveModel {
        id: Set(camera_id),
        site_id: Set(site_id),
        edge_box_id: Set(edge_box_id),
        name: Set(name.into()),
        rtsp_url: Set(Some("rtsp://example/stream".into())),
        substream_url: Set(None),
        credentials_ref: Set(None),
        zones_json: Set(json!([])),
        lines_json: Set(json!([])),
        analytics_consent: Set(true),
        active: Set(true),
        protect_camera_id: Set(None),
        mac_address: Set(None),
        model: Set(None),
        online: Set(None),
        role: Set(None),
        proposed_zones_json: Set(None),
        proposed_lines_json: Set(None),
        proposed_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .expect("seed camera");
    camera_id
}

// ── Test plumbing ───────────────────────────────────────────────────────────

/// The test router mounts:
/// - `routes::router(db)` at the root (the edge `/control/*` tree).
/// - `routes::workspace_routes(db)` nested under `/{workspace_id}`,
///   mirroring how `crates/app/src/server/router/workspace.rs` wires
///   it. In production this subtree is also behind
///   `workspace_middleware` (cloud) / `local_context_middleware`
///   (local), but for Tier-A we exercise the cameras crate in
///   isolation — the service-layer ownership checks are the assertion.
fn router(db: DatabaseConnection) -> axum::Router {
    axum::Router::new()
        .merge(routes::router::<()>(db.clone()))
        .nest("/{workspace_id}", routes::workspace_routes::<()>(db))
}

async fn body_json(resp: Response) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .expect("read body");
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("parse json: {e}; body={}", String::from_utf8_lossy(&bytes)))
}

/// Convenience: create one edge_box row for a site in
/// `workspace_id` and return its id. No device claim is attached, so
/// calls to `/control/*` against this box won't authenticate. Use
/// [`register_box`] when you need a JWT-authable box (which is the
/// common case).
async fn register_box_unbound(db: &DatabaseConnection, workspace_id: Uuid, site_id: Uuid) -> Uuid {
    let input = oxy_cameras::service::registration::RegisterEdgeBoxInput {
        site_id,
        hardware_model: "smoke-n100".into(),
        image_tag: None,
        cohort: None,
    };
    oxy_cameras::service::registration::register_edge_box(db, workspace_id, input)
        .await
        .expect("register edge box")
        .edge_box_id
}

/// Convenience: register one edge box for a site in `workspace_id`,
/// enroll a fresh IoT device, bind the two together, and return
/// `(edge_box_id, jwt)`. The JWT is signed with the device's HMAC
/// secret and verifies against the `/control/*` middleware exactly
/// like a worker-minted one would.
async fn register_box(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    site_id: Uuid,
) -> (Uuid, String) {
    let edge_box_id = register_box_unbound(db, workspace_id, site_id).await;
    let (device_id, secret, _claim_code) = enroll_test_device(db).await;
    oxy_cameras::service::fleet::bind_existing(db, workspace_id, device_id, edge_box_id, None)
        .await
        .expect("bind device to edge box");
    let exp = chrono::Utc::now().timestamp() + 3600;
    let jwt = oxy_cameras::auth::jwt::sign(device_id, &secret, exp);
    (edge_box_id, jwt)
}

/// Make sure Airhouse env vars aren't leaking from the host into the
/// test — we depend on `AirhouseConfig::from_env()` returning Disabled.
/// Var names match `crates/airhouse/src/config.rs`.
fn assert_airhouse_disabled_in_env() {
    for var in [
        "AIRHOUSE_BASE_URL",
        "AIRHOUSE_ADMIN_TOKEN",
        "AIRHOUSE_WIRE_HOST",
    ] {
        assert!(
            std::env::var(var).is_err(),
            "{var} is set in test env — these tests assume Airhouse is disabled"
        );
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// End-to-end happy path up to the airhouse boundary.
///
/// 1. POST /control/register → returns bearer.
/// 2. GET /control/boxes/{id}/config with that bearer → 200 + [] (no cameras yet).
/// 3. POST /control/events with that bearer → 503 (Airhouse disabled).
///
/// The 503 is the success signal: we exercised auth, middleware,
/// routing, DTO deserialization, and `service::ingest::write_events`
/// dispatch — everything except the pgwire write itself.
///
/// Note: the in-memory write-coalescing buffer is disabled for this test
/// (`OXY_CAMERAS_INGEST_BUFFER_DISABLED=1`) so the events POST takes the
/// synchronous Airhouse path and reaches the expected 503 boundary.
#[tokio::test]
async fn happy_path_register_then_ingest_reaches_airhouse_boundary() {
    assert_airhouse_disabled_in_env();
    // Disable the write-coalescing buffer so the events POST takes the
    // synchronous Airhouse path and returns the expected 503 immediately.
    //
    // SAFETY: `std::env::set_var` is unsafe in Rust 2024 because concurrent
    // env reads in the same process can race. We guard against that here with
    // a process-wide OnceLock — the closure is guaranteed to execute exactly
    // once, so there is exactly one writer. No other test in this binary reads
    // OXY_CAMERAS_INGEST_BUFFER_DISABLED via `buffering_disabled()`: every
    // other /control/events caller in this file either carries an empty batch
    // (rejected before the service layer) or an invalid bearer (401 before any
    // env read), so there is no concurrent reader racing against this write.
    static BUFFER_DISABLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    BUFFER_DISABLED.get_or_init(|| unsafe {
        std::env::set_var("OXY_CAMERAS_INGEST_BUFFER_DISABLED", "1");
    });
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let app = router(db.clone());

    let (edge_box_id, bearer) = register_box(&db, workspace_id, site_id).await;
    assert_ne!(edge_box_id, Uuid::nil(), "edge_box_id should be assigned");

    // Config fetch with bearer.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/control/boxes/{edge_box_id}/config"))
                .header(AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "config fetch should be 200");
    let config = body_json(resp).await;
    // Response is now an EdgeConfigDto wrapper carrying cameras +
    // active domain pack. The pack auto-seeds on first read (see
    // `service::packs::get_active`).
    assert_eq!(
        config["cameras"].as_array().map(|a| a.len()),
        Some(0),
        "no cameras seeded yet"
    );
    assert!(
        config["domain_pack"].is_object(),
        "domain_pack should be inlined for the worker: {config}"
    );
    assert!(
        config["domain_pack"]["roles"].is_array(),
        "domain_pack body shape should include roles"
    );

    // Events POST — non-empty batch must reach the Airhouse boundary.
    let event_body = json!({
        "events": [{
            "event_id":      Uuid::new_v4(),
            "ts":            Utc::now(),
            "camera_id":     Uuid::new_v4(),
            "event_type":    "person_in_zone",
            "track_id":      "track-1",
            "dwell_seconds": 3.2,
            "confidence":    0.85,
        }]
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/events")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::from(event_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "valid bearer + non-empty batch should reach the service layer; \
         Airhouse-disabled = 503 SERVICE_UNAVAILABLE"
    );
    let err_body = body_json(resp).await;
    assert_eq!(
        err_body["code"].as_str(),
        Some("airhouse_disabled"),
        "503 must carry the airhouse_disabled code so operators can tell \
         this apart from generic 5xx"
    );
}

#[tokio::test]
async fn missing_bearer_returns_401() {
    let db = test_db().await;
    let (_workspace_id, _site_id) = seed_site(&db).await;
    let app = router(db);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/events")
                .header("content-type", "application/json")
                .body(Body::from(json!({"events": []}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bad_bearer_returns_401() {
    let db = test_db().await;
    let (_workspace_id, _site_id) = seed_site(&db).await;
    let app = router(db);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/events")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, "Bearer not-a-real-token-just-random-bytes")
                .body(Body::from(json!({"events": []}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn malformed_auth_header_returns_401() {
    let db = test_db().await;
    let (_workspace_id, _site_id) = seed_site(&db).await;
    let app = router(db);

    // Wrong scheme.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/events")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, "Basic abc123")
                .body(Body::from(json!({"events": []}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Empty after the scheme.
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/events")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, "Bearer ")
                .body(Body::from(json!({"events": []}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn config_returns_cameras_bound_to_this_box_and_claims_unbound_at_site() {
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let app = router(db.clone());

    let (edge_box_id, bearer) = register_box(&db, workspace_id, site_id).await;

    // One camera already bound to this box, one unbound (site-only).
    // The original contract here was "only bound cameras appear"; that
    // changed in this same commit — first config poll auto-claims
    // unbound cameras at the box's site so the operator doesn't need
    // to bind manually after register + deploy. Both should now
    // appear; the unbound one because it gets claimed.
    let bound = seed_camera(&db, site_id, Some(edge_box_id), "bound-cam").await;
    let claimed = seed_camera(&db, site_id, None, "unbound-cam").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/control/boxes/{edge_box_id}/config"))
                .header(AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let cams = body_json(resp).await;
    let ids: std::collections::HashSet<Uuid> = cams["cameras"]
        .as_array()
        .expect("cameras array")
        .iter()
        .map(|c| c["id"].as_str().unwrap().parse().unwrap())
        .collect();
    let expected: std::collections::HashSet<Uuid> = [bound, claimed].into_iter().collect();
    assert_eq!(
        ids, expected,
        "first config poll should return both the already-bound camera \
         AND the now-auto-claimed unbound one"
    );
}

#[tokio::test]
async fn cross_box_config_fetch_is_forbidden() {
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, bearer_a) = register_box(&db, workspace_id, site_id).await;
    let (box_b, _bearer_b) = register_box(&db, workspace_id, site_id).await;
    assert_ne!(box_a, box_b);

    // Using box A's bearer to fetch box B's config must 403, even though
    // both boxes are in the same workspace.
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/control/boxes/{box_b}/config"))
                .header(AUTHORIZATION, format!("Bearer {bearer_a}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "URL box_id must match the token's box_id"
    );
}

// ── List / detail endpoints ─────────────────────────────────────────────────

/// `GET /{wid}/cameras/sites` returns only sites belonging to the URL's
/// workspace, with the serialized fields the UI table needs.
#[tokio::test]
async fn list_sites_returns_workspace_scoped_rows() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    // Seed a second workspace + site so we can prove ws_a's listing
    // doesn't leak ws_b's row.
    let (_ws_b, _site_b) = seed_site(&db).await;
    let app = router(db);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/sites"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let arr = body_json(resp).await;
    let rows = arr.as_array().expect("array");
    assert_eq!(rows.len(), 1, "listing should be workspace-scoped");
    assert_eq!(rows[0]["id"].as_str().unwrap(), site_a.to_string());
    assert_eq!(rows[0]["workspace_id"].as_str().unwrap(), ws_a.to_string());
    assert_eq!(rows[0]["name"].as_str(), Some("smoke-site"));
}

/// `GET /{wid}/cameras/sites/{site_id}` returns 200 for own-workspace
/// sites and 403 (not 404) when reaching across — that 403 matches the
/// write-side guards and is the more honest "you have no business
/// touching this" signal vs. probing for existence.
#[tokio::test]
async fn get_site_enforces_workspace_ownership() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let (ws_b, site_b) = seed_site(&db).await;
    let app = router(db);

    // own-workspace → 200
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/sites/{site_a}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // cross-workspace → 403
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/sites/{site_b}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "site B fetched through workspace A's URL must 403"
    );
    let err = body_json(resp).await;
    assert_eq!(err["code"].as_str(), Some("forbidden"));

    // missing site → 404 (so callers can distinguish "doesn't exist"
    // from "exists but yours-not")
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_b}/cameras/sites/{}", Uuid::new_v4()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// `GET /{wid}/cameras/edge-boxes` returns edge boxes + folded
/// `site_name`. Other workspaces' edge boxes must not leak.
#[tokio::test]
async fn list_edge_boxes_returns_workspace_scoped_with_site_name() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let (ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());

    // Register one box in each workspace.
    let (box_a, _) = register_box(&db, ws_a, site_a).await;
    let (_box_b, _) = register_box(&db, ws_b, site_b).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/edge-boxes"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let arr = body_json(resp).await;
    let rows = arr.as_array().expect("array");
    assert_eq!(rows.len(), 1, "should only see ws_a's edge box");
    assert_eq!(rows[0]["id"].as_str().unwrap(), box_a.to_string());
    assert_eq!(rows[0]["site_id"].as_str().unwrap(), site_a.to_string());
    assert_eq!(
        rows[0]["site_name"].as_str(),
        Some("smoke-site"),
        "summary DTO must fold site_name in for the UI"
    );
}

/// `GET /{wid}/cameras/edge-boxes/{box_id}` — 200 own, 403 cross.
#[tokio::test]
async fn get_edge_box_enforces_workspace_ownership() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let (ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, _) = register_box(&db, ws_a, site_a).await;
    let (box_b, _) = register_box(&db, ws_b, site_b).await;

    // Own → 200
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/edge-boxes/{box_a}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let row = body_json(resp).await;
    assert_eq!(row["id"].as_str().unwrap(), box_a.to_string());

    // Cross → 403
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/edge-boxes/{box_b}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// `GET /{wid}/cameras` returns cameras + folded site_name +
/// edge_box_name. Cross-workspace cameras must not leak.
#[tokio::test]
async fn list_cameras_returns_workspace_scoped_with_joined_names() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let (_ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, _) = register_box(&db, ws_a, site_a).await;

    // ws_a's cameras: one bound to box_a, one unbound; ws_b: one camera.
    let bound = seed_camera(&db, site_a, Some(box_a), "bound-cam").await;
    let unbound = seed_camera(&db, site_a, None, "unbound-cam").await;
    let _foreign = seed_camera(&db, site_b, None, "foreign-cam").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let arr = body_json(resp).await;
    let rows = arr.as_array().expect("array");
    let ids: Vec<Uuid> = rows
        .iter()
        .map(|r| r["id"].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(rows.len(), 2, "should only see ws_a's two cameras");
    assert!(ids.contains(&bound), "bound camera should be in list");
    assert!(ids.contains(&unbound), "unbound camera should be in list");

    // Verify the bound row has both joined names; the unbound row has
    // a site_name but no edge_box_name (null).
    let bound_row = rows
        .iter()
        .find(|r| r["id"].as_str().unwrap().parse::<Uuid>().unwrap() == bound)
        .expect("bound row in response");
    assert_eq!(bound_row["site_name"].as_str(), Some("smoke-site"));
    assert!(
        bound_row["edge_box_name"].is_string(),
        "bound camera should have edge_box_name; got {bound_row:#}"
    );

    let unbound_row = rows
        .iter()
        .find(|r| r["id"].as_str().unwrap().parse::<Uuid>().unwrap() == unbound)
        .expect("unbound row in response");
    assert_eq!(unbound_row["site_name"].as_str(), Some("smoke-site"));
    assert!(
        unbound_row["edge_box_name"].is_null(),
        "unbound camera's edge_box_name must be null; got {unbound_row:#}"
    );

    // edge_box_status is a string for bound cameras (carries the
    // current Postgres value — pending/active/offline/retired) and
    // null for unbound. UI checks this to flag "edge offline"
    // warnings on rows whose box hasn't been heard from in 90s.
    assert!(
        bound_row["edge_box_status"].is_string(),
        "bound camera should expose edge_box_status; got {bound_row:#}"
    );
    assert!(
        unbound_row["edge_box_status"].is_null(),
        "unbound camera's edge_box_status must be null; got {unbound_row:#}"
    );
}

/// `GET /{wid}/cameras/{cam_id}` — 200 own, 403 cross, 404 missing.
#[tokio::test]
async fn get_camera_enforces_workspace_ownership() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let (ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());

    let cam_a = seed_camera(&db, site_a, None, "a-cam").await;
    let cam_b = seed_camera(&db, site_b, None, "b-cam").await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/{cam_a}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/{cam_b}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_b}/cameras/{}", Uuid::new_v4()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Preview snapshot proxy ──────────────────────────────────────────────────

/// Cross-workspace probe — workspace A asks for workspace B's camera's
/// snapshot. The proxy must 403 with code "forbidden", same as the
/// detail-get path. This is the most security-critical test of the
/// preview surface: a leak here would let a caller view video from any
/// workspace they can guess a camera_id for.
#[tokio::test]
async fn snapshot_proxy_rejects_cross_workspace() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (_ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());

    let cam_b = seed_camera(&db, site_b, None, "b-cam").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/{cam_b}/preview/snapshot.jpg"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let err = body_json(resp).await;
    assert_eq!(err["code"].as_str(), Some("forbidden"));
}

/// Unknown camera → 404. Catches a regression where a bug in the proxy
/// silently maps "no such camera in DB" into something else (e.g. an
/// upstream 502 if it tries to fetch from a null tailscale_ip).
#[tokio::test]
async fn snapshot_proxy_unknown_camera_returns_404() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{ws_a}/cameras/{}/preview/snapshot.jpg",
                    Uuid::new_v4()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// Camera exists but isn't bound to an edge box yet (e.g. UniFi
/// inventory imported it but no worker has been installed). 503 with
/// code "unavailable" — distinct from upstream errors so the UI can
/// render a "no edge connected" placeholder rather than "something
/// broke".
#[tokio::test]
async fn snapshot_proxy_unbound_camera_returns_503() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    // No edge_box_id — UniFi inventory import case.
    let cam = seed_camera(&db, site_a, None, "unbound").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/{cam}/preview/snapshot.jpg"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let err = body_json(resp).await;
    assert_eq!(err["code"].as_str(), Some("unavailable"));
}

/// Camera bound to an edge box but the edge box has no `tailscale_ip`
/// (just-registered, not yet on the overlay network). Different
/// message but same 503 + "unavailable" code as the unbound case —
/// UI treats both as "preview not ready".
#[tokio::test]
async fn snapshot_proxy_edge_without_tailscale_ip_returns_503() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    // Register an edge box (tailscale_ip is NotSet at registration).
    let (box_a, _) = register_box(&db, ws_a, site_a).await;
    let cam = seed_camera(&db, site_a, Some(box_a), "bound-no-ip").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/{cam}/preview/snapshot.jpg"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let err = body_json(resp).await;
    assert_eq!(err["code"].as_str(), Some("unavailable"));
}

// ── HLS preview proxy ───────────────────────────────────────────────────────
//
// Same shape as the snapshot tests: cover the auth boundary + the
// not-yet-ready cases. The actual MediaMTX roundtrip isn't tested at
// this layer (no fake upstream), but every code path that could leak
// across workspaces is.

#[tokio::test]
async fn hls_proxy_rejects_cross_workspace() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (_ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());

    let cam_b = seed_camera(&db, site_b, None, "b-cam").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/{cam_b}/preview/hls/index.m3u8"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let err = body_json(resp).await;
    assert_eq!(err["code"].as_str(), Some("forbidden"));
}

#[tokio::test]
async fn hls_proxy_unknown_camera_returns_404() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{ws_a}/cameras/{}/preview/hls/index.m3u8",
                    Uuid::new_v4()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn hls_proxy_unbound_camera_returns_503() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let cam = seed_camera(&db, site_a, None, "unbound").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/{cam}/preview/hls/index.m3u8"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let err = body_json(resp).await;
    assert_eq!(err["code"].as_str(), Some("unavailable"));
}

#[tokio::test]
async fn hls_proxy_edge_without_tailscale_ip_returns_503() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, _) = register_box(&db, ws_a, site_a).await;
    let cam = seed_camera(&db, site_a, Some(box_a), "bound-no-ip").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/{cam}/preview/hls/index.m3u8"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let err = body_json(resp).await;
    assert_eq!(err["code"].as_str(), Some("unavailable"));
}

/// The same wildcard handler must match the playlist AND every
/// segment URL. `seg000.ts` is structurally indistinguishable from
/// `index.m3u8` at this layer — both 503 here because no Tailscale IP
/// is set, but the test confirms the `{*tail}` wildcard route is
/// mounted correctly.
#[tokio::test]
async fn hls_proxy_handles_wildcard_segment_paths() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, _) = register_box(&db, ws_a, site_a).await;
    let cam = seed_camera(&db, site_a, Some(box_a), "segs").await;

    for tail in ["index.m3u8", "seg000.ts", "init.mp4"] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/{ws_a}/cameras/{cam}/preview/hls/{tail}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "wildcard route should match {tail}; got status {}",
            resp.status()
        );
    }
}

// ── Recording-clip proxy ────────────────────────────────────────────────────
//
// Mirrors the HLS-proxy coverage: every workspace-isolation /
// edge-binding code path that could leak across boundaries or surface
// a confusing error needs an explicit test. The actual upstream
// (MediaMTX :9996 playback server) isn't faked here — the tests stop
// at the guard chain, which is the only Oxy-side behavior worth
// asserting.

#[tokio::test]
async fn recording_proxy_rejects_cross_workspace() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (_ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());

    let cam_b = seed_camera(&db, site_b, None, "b-cam").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{ws_a}/cameras/{cam_b}/preview/recording?start=2026-05-27T10:00:00Z&duration=10"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let err = body_json(resp).await;
    assert_eq!(err["code"].as_str(), Some("forbidden"));
}

#[tokio::test]
async fn recording_proxy_unknown_camera_returns_404() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{ws_a}/cameras/{}/preview/recording?start=2026-05-27T10:00:00Z&duration=10",
                    Uuid::new_v4()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn recording_proxy_unbound_camera_returns_503() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let cam = seed_camera(&db, site_a, None, "unbound").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{ws_a}/cameras/{cam}/preview/recording?start=2026-05-27T10:00:00Z&duration=10"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let err = body_json(resp).await;
    assert_eq!(err["code"].as_str(), Some("unavailable"));
}

#[tokio::test]
async fn recording_proxy_edge_without_tailscale_ip_returns_503() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, _) = register_box(&db, ws_a, site_a).await;
    let cam = seed_camera(&db, site_a, Some(box_a), "bound-no-ip").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{ws_a}/cameras/{cam}/preview/recording?start=2026-05-27T10:00:00Z&duration=10"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let err = body_json(resp).await;
    assert_eq!(err["code"].as_str(), Some("unavailable"));
}

/// The clip-playback handler rejects out-of-range durations before
/// ever calling the upstream — keeps an operator from accidentally
/// pulling a 24-hour MP4 through Oxy by typoing `duration=86400`.
#[tokio::test]
async fn recording_proxy_rejects_oversized_duration() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let cam = seed_camera(&db, site_a, None, "for-duration-check").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{ws_a}/cameras/{cam}/preview/recording?start=2026-05-27T10:00:00Z&duration=99999"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let err = body_json(resp).await;
    assert_eq!(err["code"].as_str(), Some("invalid_input"));
}

// ── Auto-claim on first config fetch ────────────────────────────────────────
//
// Closes the "no per-camera manual binding" loop in the onboarding
// flow. Operator imports UniFi inventory (cameras land with
// edge_box_id = NULL), registers an edge box, deploys it; the edge
// box's first config poll auto-binds every unbound camera at its
// site. These tests pin that contract.

use oxy_cameras::entities::edge_boxes;

#[tokio::test]
async fn config_fetch_auto_claims_unbound_cameras() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    // 3 unbound cameras at this site, all imported from UniFi.
    let c1 = seed_camera(&db, site_a, None, "cam-1").await;
    let c2 = seed_camera(&db, site_a, None, "cam-2").await;
    let c3 = seed_camera(&db, site_a, None, "cam-3").await;

    let (box_a, bearer) = register_box(&db, ws_a, site_a).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/control/boxes/{box_a}/config"))
                .header(AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp).await;
    let arr = body["cameras"].as_array().expect("cameras array");
    let returned_ids: std::collections::HashSet<Uuid> = arr
        .iter()
        .map(|c| c["id"].as_str().unwrap().parse().unwrap())
        .collect();
    let expected: std::collections::HashSet<Uuid> = [c1, c2, c3].into_iter().collect();
    assert_eq!(
        returned_ids, expected,
        "first config poll should claim + return all unbound cameras at the site"
    );

    // Verify the DB-side binding actually happened (not just the
    // returned list).
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    for cam_id in [c1, c2, c3] {
        let cam = cameras::Entity::find_by_id(cam_id)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            cam.edge_box_id,
            Some(box_a),
            "camera {cam_id} should be bound to box {box_a}"
        );
    }

    // Sanity: a camera at a DIFFERENT site shouldn't have been touched.
    // (We don't seed one here, but the SiteId filter is the protection.)
    let _ = cameras::Entity::find()
        .filter(cameras::Column::SiteId.eq(site_a))
        .filter(cameras::Column::EdgeBoxId.is_null())
        .one(&db)
        .await
        .unwrap();
}

#[tokio::test]
async fn config_fetch_is_idempotent_on_repoll() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let c1 = seed_camera(&db, site_a, None, "cam").await;
    let (box_a, bearer) = register_box(&db, ws_a, site_a).await;

    // First poll claims.
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/control/boxes/{box_a}/config"))
                .header(AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Second poll — same result, no thrash. Most importantly: no
    // duplicate UPDATEs hitting any new cameras (there aren't any to
    // claim, but the SQL still runs).
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/control/boxes/{box_a}/config"))
                .header(AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let ids: Vec<Uuid> = body["cameras"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(ids, vec![c1], "second poll returns the same camera set");
}

#[tokio::test]
async fn auto_claim_is_fcfs_across_two_boxes_at_same_site() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let _c1 = seed_camera(&db, site_a, None, "shared-1").await;
    let _c2 = seed_camera(&db, site_a, None, "shared-2").await;

    let (box_first, bearer_first) = register_box(&db, ws_a, site_a).await;
    let (box_second, bearer_second) = register_box(&db, ws_a, site_a).await;
    assert_ne!(box_first, box_second);

    // First box claims everything.
    let resp1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/control/boxes/{box_first}/config"))
                .header(AUTHORIZATION, format!("Bearer {bearer_first}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);
    let body1 = body_json(resp1).await;
    assert_eq!(
        body1["cameras"].as_array().map(|a| a.len()),
        Some(2),
        "first box should claim both cameras"
    );

    // Second box's poll finds nothing left to claim.
    let resp2 = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/control/boxes/{box_second}/config"))
                .header(AUTHORIZATION, format!("Bearer {bearer_second}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let body2 = body_json(resp2).await;
    assert_eq!(
        body2["cameras"].as_array().map(|a| a.len()),
        Some(0),
        "FCFS: second box gets nothing — operator can manually rebind later if needed"
    );
}

// ── Auto-update of edge_boxes.tailscale_ip ──────────────────────────────────

/// Edge box calls /control/boxes/{id}/config with `X-Forwarded-For`
/// set to the IP it's calling from. Middleware should update
/// `edge_boxes.tailscale_ip` to that value. Pins the proxy-able-by-Oxy
/// invariant — without this, snapshot + HLS proxies would stay 503
/// forever after install.
#[tokio::test]
async fn tailscale_ip_auto_updates_from_x_forwarded_for() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, bearer) = register_box(&db, ws_a, site_a).await;
    // tailscale_ip is NotSet at registration.
    let before = edge_boxes::Entity::find_by_id(box_a)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.tailscale_ip, None);

    let _ = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/control/boxes/{box_a}/config"))
                .header(AUTHORIZATION, format!("Bearer {bearer}"))
                .header("x-forwarded-for", "100.64.1.42")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let after = edge_boxes::Entity::find_by_id(box_a)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.tailscale_ip.as_deref(),
        Some("100.64.1.42"),
        "middleware should write the X-Forwarded-For value into tailscale_ip"
    );
}

/// `X-Edge-Tailscale-IP` is the explicit-override header. It wins
/// over `X-Forwarded-For` so a Tailscale-aware edge can tell Oxy
/// "my Tailscale IP is X" even if Oxy is behind a proxy whose XFF
/// would normally reflect the proxy's view.
#[tokio::test]
async fn tailscale_ip_prefers_explicit_header_over_xff() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, bearer) = register_box(&db, ws_a, site_a).await;

    let _ = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/control/boxes/{box_a}/config"))
                .header(AUTHORIZATION, format!("Bearer {bearer}"))
                .header("x-forwarded-for", "10.1.2.3, 10.4.5.6")
                .header("x-edge-tailscale-ip", "100.64.1.42")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let after = edge_boxes::Entity::find_by_id(box_a)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.tailscale_ip.as_deref(), Some("100.64.1.42"));
}

/// Idempotent — repeated calls with the same IP don't churn the
/// `tailscale_ip` itself stays stable across same-IP polls.
///
/// History: this test used to also assert `updated_at` stayed stable
/// (the UPDATE was short-circuited when nothing changed). That contract
/// went away in the same commit that added `last_seen_at` auto-updates
/// — every /control/* call now writes the row to refresh `last_seen_at`
/// for the dashboard's "Last seen" column. Re-narrowed the assertion
/// to what callers actually care about: the IP itself doesn't churn.
#[tokio::test]
async fn tailscale_ip_stays_stable_on_repeated_polls() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, bearer) = register_box(&db, ws_a, site_a).await;
    let xff = "100.64.1.99";

    for _ in 0..3 {
        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/control/boxes/{box_a}/config"))
                    .header(AUTHORIZATION, format!("Bearer {bearer}"))
                    .header("x-forwarded-for", xff)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    let after = edge_boxes::Entity::find_by_id(box_a)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.tailscale_ip.as_deref(),
        Some(xff),
        "tailscale_ip should remain the observed value across repeated polls"
    );
}

/// Closes the "edge box stuck on `pending` even though it's running"
/// gap. Status starts as `pending` at registration; the first
/// authenticated /control/* call should flip it to `active`.
#[tokio::test]
async fn status_flips_from_pending_to_active_on_first_call() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, bearer) = register_box(&db, ws_a, site_a).await;

    // Pre-call: registration leaves status = "pending".
    let before = edge_boxes::Entity::find_by_id(box_a)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.status, "pending");
    assert_eq!(before.last_seen_at, None);

    let _ = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/control/boxes/{box_a}/config"))
                .header(AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let after = edge_boxes::Entity::find_by_id(box_a)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.status, "active",
        "first /control/* call should flip status pending → active"
    );
    assert!(
        after.last_seen_at.is_some(),
        "first /control/* call should populate last_seen_at"
    );
}

/// UniFi-controller edge_boxes are an implementation detail — the
/// import flow stashes per-site `unifi_public_ip` + `unifi_console_id`
/// in an edge_box row to avoid a migration. They MUST NOT show up in
/// the operator-facing Edge Boxes list or detail-get; otherwise the
/// dashboard reads like "the controller is one of my edge boxes,"
/// which is wrong and confusing.
#[tokio::test]
async fn list_edge_boxes_filters_out_unifi_controllers() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    // One real edge box (via the normal register path).
    let (real_box, _) = register_box(&db, ws_a, site_a).await;

    // One UniFi-controller-shaped row — what the import would create.
    let now = Utc::now().fixed_offset();
    let controller_id = Uuid::new_v4();
    oxy_cameras::entities::edge_boxes::ActiveModel {
        id: Set(controller_id),
        site_id: Set(site_a),
        hardware_model: Set("unifi-controller".into()),
        image_tag: Set("n/a".into()),
        cohort: Set("stable".into()),
        tailscale_ip: Set(None),
        funnel_hostname: Set(None),
        bandwidth_5min_bytes: Set(None),
        bandwidth_reported_at: Set(None),
        target_image_tag: Set(None),
        current_image_tag: Set(None),
        held_until: Set(None),
        last_update_result: Set(None),
        last_update_at: Set(None),
        edge_compatibility_json: Set(None),
        incompatible_reason: Set(None),
        status: Set("active".into()),
        unifi_console_id: Set(Some("F492…".into())),
        unifi_public_ip: Set(Some("73.95.12.34".into())),
        unifi_rtsp_reachable: Set(true),
        registered_at: Set(now),
        last_seen_at: Set(None),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();

    // List: only the real box.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/edge-boxes"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(resp).await;
    let ids: Vec<Uuid> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(ids, vec![real_box], "controller must be filtered out");

    // Detail-get on the controller — 404.
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/edge-boxes/{controller_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── PATCH /{wid}/cameras/{cam_id}/consent ───────────────────────────────────

/// Operator toggles a camera's `analytics_consent`. The edge worker
/// picks it up on its next config poll and skips inference / event
/// emission accordingly. The route delegates to
/// `service::config::set_analytics_consent`, so all the workspace-
/// ownership guards from that service fn apply here too.
#[tokio::test]
async fn patch_consent_flips_the_flag() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    // seed_camera sets analytics_consent=true; flip to false then back ON.
    let cam = seed_camera(&db, site_a, None, "cam").await;
    let mut am: cameras::ActiveModel = cameras::Entity::find_by_id(cam)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.analytics_consent = Set(false);
    am.update(&db).await.unwrap();

    // Flip ON.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/{cam}/consent"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"analytics_consent": true}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let row = cameras::Entity::find_by_id(cam)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.analytics_consent, "PATCH true should write true");

    // Flip OFF.
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/{cam}/consent"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"analytics_consent": false}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let row = cameras::Entity::find_by_id(cam)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(!row.analytics_consent, "PATCH false should write false");
}

/// PATCH /cameras/{id}/role writes the role and rejects unknown
/// values. The edge worker keys into its prompt registry by this
/// string; the backend validates so a typo doesn't silently degrade
/// to the general prompt.
#[tokio::test]
async fn patch_role_writes_and_validates() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let cam = seed_camera(&db, site_a, None, "cam").await;

    // Known role → stored verbatim.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/{cam}/role"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"role": "kitchen"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let row = cameras::Entity::find_by_id(cam)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.role.as_deref(), Some("kitchen"));

    // Null body clears.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/{cam}/role"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"role": null}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let row = cameras::Entity::find_by_id(cam)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.role.is_none(), "null body should clear role");

    // Unknown role → 400 (mapped from InvalidInput).
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/{cam}/role"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"role": "drive-thru"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn patch_consent_rejects_cross_workspace() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (_ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());

    let cam_b = seed_camera(&db, site_b, None, "b-cam").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/{cam_b}/consent"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"analytics_consent": true}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ── Stale checker ───────────────────────────────────────────────────────────
//
// `service::stale::sweep_once` is the inverse of the auth-middleware's
// status flip — when an edge box stops calling in, we mark it
// `offline` so operators see reality on the dashboard. The function
// is exercised directly (no spawn) so we don't depend on tick timing.

async fn seed_edge_box(
    db: &DatabaseConnection,
    site_id: Uuid,
    status: &str,
    last_seen_secs_ago: Option<i64>,
) -> Uuid {
    use chrono::Duration as ChronoDuration;
    use sea_orm::ActiveModelTrait;

    let id = Uuid::new_v4();
    let now = Utc::now().fixed_offset();
    let last_seen = last_seen_secs_ago.map(|s| now + ChronoDuration::seconds(-s));
    oxy_cameras::entities::edge_boxes::ActiveModel {
        id: Set(id),
        site_id: Set(site_id),
        hardware_model: Set("smoke-stale".into()),
        image_tag: Set("test".into()),
        cohort: Set("stable".into()),
        tailscale_ip: Set(None),
        funnel_hostname: Set(None),
        bandwidth_5min_bytes: Set(None),
        bandwidth_reported_at: Set(None),
        target_image_tag: Set(None),
        current_image_tag: Set(None),
        held_until: Set(None),
        last_update_result: Set(None),
        last_update_at: Set(None),
        edge_compatibility_json: Set(None),
        incompatible_reason: Set(None),
        status: Set(status.into()),
        unifi_console_id: Set(None),
        unifi_public_ip: Set(None),
        unifi_rtsp_reachable: Set(false),
        registered_at: Set(now),
        last_seen_at: Set(last_seen),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn load_status(db: &DatabaseConnection, id: Uuid) -> String {
    edge_boxes::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .status
}

#[tokio::test]
async fn stale_sweep_flips_only_silent_active_boxes() {
    use oxy_cameras::service::stale;

    let db = test_db().await;
    let (_ws, site) = seed_site(&db).await;

    // Far past threshold — must flip.
    let stale_active = seed_edge_box(&db, site, "active", Some(300)).await;
    // Polled recently — must stay active (well under the 90s threshold).
    let fresh_active = seed_edge_box(&db, site, "active", Some(10)).await;
    // Already offline + stale — must NOT be re-flipped (would churn
    // updated_at and trigger spurious "edge box went offline" alerts).
    let already_offline = seed_edge_box(&db, site, "offline", Some(300)).await;
    // Never seen — pending box that was just registered; must NOT be
    // marked offline (the auth middleware will flip it active on the
    // first poll). The filter on `last_seen_at IS NOT NULL` is what
    // guarantees this.
    let never_seen = seed_edge_box(&db, site, "pending", None).await;

    let flipped = stale::sweep_once(&db, stale::STALE_THRESHOLD)
        .await
        .unwrap();
    assert_eq!(flipped, 1, "exactly one stale-active box should flip");

    assert_eq!(load_status(&db, stale_active).await, "offline");
    assert_eq!(load_status(&db, fresh_active).await, "active");
    assert_eq!(load_status(&db, already_offline).await, "offline");
    assert_eq!(load_status(&db, never_seen).await, "pending");
}

#[tokio::test]
async fn stale_sweep_is_idempotent() {
    use oxy_cameras::service::stale;

    let db = test_db().await;
    let (_ws, site) = seed_site(&db).await;
    seed_edge_box(&db, site, "active", Some(300)).await;

    let first = stale::sweep_once(&db, stale::STALE_THRESHOLD)
        .await
        .unwrap();
    let second = stale::sweep_once(&db, stale::STALE_THRESHOLD)
        .await
        .unwrap();
    assert_eq!(first, 1);
    assert_eq!(
        second, 0,
        "second sweep is a no-op once everything is offline"
    );
}

// ── Compliance-reports list endpoint ────────────────────────────────────────
//
// Mirrors the ingest/preview coverage: every workspace-isolation +
// input-validation path that could leak across boundaries or surface
// confusing errors needs an explicit test. The Airhouse read itself is
// not exercised here (broker unconfigured → 503), but we DO cover the
// service-layer guard chain that gates whether we even reach the broker.

#[tokio::test]
async fn compliance_list_rejects_cross_workspace() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (_ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());

    let cam_b = seed_camera(&db, site_b, None, "b-cam").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/{cam_b}/compliance-reports"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let err = body_json(resp).await;
    assert_eq!(err["code"].as_str(), Some("forbidden"));
}

#[tokio::test]
async fn compliance_list_unknown_camera_returns_404() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{ws_a}/cameras/{}/compliance-reports",
                    Uuid::new_v4()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn compliance_list_rejects_bad_since() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let cam = seed_camera(&db, site_a, None, "for-since-check").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{ws_a}/cameras/{cam}/compliance-reports?since=not-a-timestamp"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let err = body_json(resp).await;
    assert_eq!(err["code"].as_str(), Some("invalid_input"));
}

/// Guard chain passes (camera exists, belongs to caller's workspace,
/// `since` parses) → we reach Airhouse, which is disabled in tests,
/// which surfaces as 503. Same success signal we use for the ingest
/// tests above: 503 here means the SELECT path is wired to the right
/// service layer.
#[tokio::test]
async fn compliance_list_reaches_airhouse_boundary() {
    assert_airhouse_disabled_in_env();

    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let cam = seed_camera(&db, site_a, None, "for-airhouse-boundary").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/{cam}/compliance-reports?limit=10"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ── Site rollup endpoint ────────────────────────────────────────────────────
//
// Same shape as the per-camera list: workspace-ownership +
// input-validation gate before any Airhouse traffic. 503 with
// Airhouse disabled means we reached the broker boundary cleanly,
// which is the success signal we use throughout this suite.

#[tokio::test]
async fn compliance_summary_rejects_cross_workspace() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (_ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/sites/{site_b}/compliance-summary"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let err = body_json(resp).await;
    assert_eq!(err["code"].as_str(), Some("forbidden"));
}

#[tokio::test]
async fn compliance_summary_unknown_site_returns_404() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{ws_a}/cameras/sites/{}/compliance-summary",
                    Uuid::new_v4()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn compliance_summary_rejects_bad_since() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{ws_a}/cameras/sites/{site_a}/compliance-summary?since=not-a-timestamp"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let err = body_json(resp).await;
    assert_eq!(err["code"].as_str(), Some("invalid_input"));
}

/// Site exists with no cameras: short-circuit returns an empty array
/// without ever touching Airhouse. Important because a freshly-onboarded
/// workspace will land on the Compliance tab before any camera is
/// imported; the UI shouldn't 503 in that case.
#[tokio::test]
async fn compliance_summary_empty_site_returns_empty_without_airhouse() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/sites/{site_a}/compliance-summary"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body.as_array().map(|a| a.len()), Some(0));
}

/// Site has cameras → we reach Airhouse → broker is unconfigured → 503.
/// Confirms the SELECT path is wired to the right service-layer fn.
#[tokio::test]
async fn compliance_summary_with_cameras_reaches_airhouse_boundary() {
    assert_airhouse_disabled_in_env();

    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let _ = seed_camera(&db, site_a, None, "cam-1").await;
    let _ = seed_camera(&db, site_a, None, "cam-2").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/sites/{site_a}/compliance-summary"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ── WebRTC session endpoint ─────────────────────────────────────────────────
//
// Same guard-chain shape as the other /preview/* routes. Tests
// here pin every gate that has to pass before we hand out a TURN
// credential, plus the secret-not-set case where the whole feature
// must refuse to mint anything.

/// SAFETY: setting / clearing env vars is process-wide. The tests
/// read OXY_CAMERAS_TURN_AUTH_SECRET; this guard restores the prior
/// value on Drop so other tests aren't affected.
fn set_turn_secret(val: Option<&str>) -> EnvVarGuard {
    let prev = std::env::var("OXY_CAMERAS_TURN_AUTH_SECRET").ok();
    match val {
        Some(v) => unsafe { std::env::set_var("OXY_CAMERAS_TURN_AUTH_SECRET", v) },
        None => unsafe { std::env::remove_var("OXY_CAMERAS_TURN_AUTH_SECRET") },
    }
    EnvVarGuard { prev }
}

struct EnvVarGuard {
    prev: Option<String>,
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => unsafe { std::env::set_var("OXY_CAMERAS_TURN_AUTH_SECRET", v) },
            None => unsafe { std::env::remove_var("OXY_CAMERAS_TURN_AUTH_SECRET") },
        }
    }
}

/// Stamp `funnel_hostname` on an edge box. In Phase 1 production
/// this gets populated by the auth middleware on every /control/*
/// call (mirrors tailscale_ip), so for tests we set it directly.
async fn set_funnel_hostname(db: &DatabaseConnection, edge_box_id: Uuid, hostname: &str) {
    use sea_orm::ActiveModelTrait;
    let mut am: oxy_cameras::entities::edge_boxes::ActiveModel =
        edge_boxes::Entity::find_by_id(edge_box_id)
            .one(db)
            .await
            .unwrap()
            .unwrap()
            .into();
    am.funnel_hostname = Set(Some(hostname.into()));
    am.update(db).await.unwrap();
}

#[tokio::test]
async fn webrtc_session_returns_503_when_secret_not_set() {
    let _guard = set_turn_secret(None);

    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, _) = register_box(&db, ws_a, site_a).await;
    set_funnel_hostname(&db, box_a, "video-poc-edge.tail666f9b.ts.net").await;
    let cam = seed_camera(&db, site_a, Some(box_a), "with-funnel").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/cameras/{cam}/preview/webrtc/session"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn webrtc_session_rejects_cross_workspace() {
    let _guard = set_turn_secret(Some("test-secret"));

    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (_ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());

    let cam_b = seed_camera(&db, site_b, None, "b-cam").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/cameras/{cam_b}/preview/webrtc/session"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn webrtc_session_unknown_camera_returns_404() {
    let _guard = set_turn_secret(Some("test-secret"));

    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/{ws_a}/cameras/{}/preview/webrtc/session",
                    Uuid::new_v4()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn webrtc_session_unbound_camera_returns_503() {
    let _guard = set_turn_secret(Some("test-secret"));

    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let cam = seed_camera(&db, site_a, None, "unbound").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/cameras/{cam}/preview/webrtc/session"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn webrtc_session_no_funnel_hostname_returns_503() {
    let _guard = set_turn_secret(Some("test-secret"));

    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, _) = register_box(&db, ws_a, site_a).await;
    // Intentionally do NOT call set_funnel_hostname — simulates a
    // box that's registered but hasn't checked in yet, so the auth
    // middleware hasn't stamped the hostname.
    let cam = seed_camera(&db, site_a, Some(box_a), "no-funnel").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/cameras/{cam}/preview/webrtc/session"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn webrtc_session_happy_path_returns_well_formed_dto() {
    let _guard = set_turn_secret(Some("test-secret"));

    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, _) = register_box(&db, ws_a, site_a).await;
    let hostname = "video-poc-edge.tail666f9b.ts.net";
    set_funnel_hostname(&db, box_a, hostname).await;
    let cam = seed_camera(&db, site_a, Some(box_a), "happy").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/cameras/{cam}/preview/webrtc/session"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp).await;
    let whep = body["whep_url"].as_str().unwrap();
    assert!(whep.starts_with(&format!("https://{hostname}/cam-")));
    // The path ends in `/whep` before the query string (the WHEP
    // token + expiry get appended as ?token=...&expiry=... so
    // MTX's HTTP-auth callback can validate the inbound request).
    assert!(whep.contains("/whep?"));
    assert!(whep.contains("token="));
    assert!(whep.contains("expiry="));

    let ice = body["ice_servers"].as_array().unwrap();
    assert_eq!(ice.len(), 1);
    let turn_url = ice[0]["urls"][0].as_str().unwrap();
    assert!(turn_url.starts_with(&format!("turns:{hostname}:8443")));
    assert!(turn_url.contains("transport=tcp"));

    let username = ice[0]["username"].as_str().unwrap();
    let parts: Vec<&str> = username.split(':').collect();
    assert_eq!(parts.len(), 2, "username should be <expiry>:<id>");
    assert!(parts[0].parse::<u64>().is_ok());
    assert!(parts[1].starts_with("cam-"));

    use base64::Engine as _;
    let cred = ice[0]["credential"].as_str().unwrap();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(cred)
        .expect("credential is valid base64");
    assert_eq!(decoded.len(), 20);
}

// ── MTX HTTP-auth callback ──────────────────────────────────────────────────
//
// MTX POSTs every action to /control/mtx-auth for an allow/deny
// decision. We only require a token for WebRTC reads (the Funnel-
// exposed surface); other actions are allowed unconditionally
// because the tailnet ACL already gates who can reach them.

#[tokio::test]
async fn mtx_auth_allows_non_webrtc_actions_without_token() {
    let _guard = set_turn_secret(None); // Secret not even needed
    let db = test_db().await;
    let app = router(db);

    for (action, protocol) in [
        ("publish", "rtsp"),
        ("read", "hls"),
        ("read", "rtsp"),
        ("playback", "hls"),
        ("api", ""),
        ("metrics", ""),
    ] {
        let body = json!({
            "action": action,
            "protocol": protocol,
            "path": "cam-protect-01",
            "query": ""
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/control/mtx-auth")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "{action}/{protocol} should be allowed without a token"
        );
    }
}

#[tokio::test]
async fn mtx_auth_rejects_webrtc_read_without_secret_configured() {
    let _guard = set_turn_secret(None);
    let db = test_db().await;
    let app = router(db);

    let body = json!({
        "action": "read",
        "protocol": "webrtc",
        "path": "cam-protect-01",
        "query": "token=anything&expiry=999999999999"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/mtx-auth")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mtx_auth_rejects_webrtc_read_without_token_query() {
    let _guard = set_turn_secret(Some("test-secret"));
    let db = test_db().await;
    let app = router(db);

    let body = json!({
        "action": "read",
        "protocol": "webrtc",
        "path": "cam-protect-01",
        "query": ""
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/mtx-auth")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mtx_auth_rejects_webrtc_read_with_bad_token() {
    let _guard = set_turn_secret(Some("test-secret"));
    let db = test_db().await;
    let app = router(db);

    let body = json!({
        "action": "read",
        "protocol": "webrtc",
        "path": "cam-protect-01",
        "query": "token=deadbeef&expiry=999999999999"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/mtx-auth")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mtx_auth_full_mint_then_validate_loop_happy_path() {
    // Mint a session through the public endpoint, parse the
    // ?token=...&expiry=... out of the whep_url, then POST it to
    // mtx-auth. The chain must round-trip 200 — that's the
    // production flow MTX exercises on every operator-side WHEP.
    let _guard = set_turn_secret(Some("test-secret-loop"));

    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, _) = register_box(&db, ws_a, site_a).await;
    set_funnel_hostname(&db, box_a, "video-poc-edge.tail666f9b.ts.net").await;
    let cam = seed_camera(&db, site_a, Some(box_a), "loop-cam").await;

    let session_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/cameras/{cam}/preview/webrtc/session"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(session_resp.status(), StatusCode::OK);
    let body = body_json(session_resp).await;
    let whep_url = body["whep_url"].as_str().unwrap().to_string();

    // Parse: https://<funnel>/cam-<id>/whep?token=<tok>&expiry=<exp>
    let url = url_lite_parse(&whep_url);
    let mtx_path = url.path.trim_start_matches('/').trim_end_matches("/whep");

    let cb_body = json!({
        "action": "read",
        "protocol": "webrtc",
        "path": mtx_path,
        "query": url.query
    });
    let cb_resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/mtx-auth")
                .header("content-type", "application/json")
                .body(Body::from(cb_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        cb_resp.status(),
        StatusCode::OK,
        "mtx-auth must accept the freshly minted whep token"
    );
}

/// Tiny URL splitter — only what's needed to pull `path` and
/// `query` out of the whep_url for the round-trip test. The
/// `url` crate is a heavy dep just for this and `httparse` etc
/// don't help here; this is enough.
struct LiteUrl {
    path: String,
    query: String,
}
fn url_lite_parse(url: &str) -> LiteUrl {
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    let after_host = after_scheme.split_once('/').map(|(_, p)| p).unwrap_or("");
    let (path, query) = match after_host.split_once('?') {
        Some((p, q)) => (format!("/{p}"), q.to_string()),
        None => (format!("/{after_host}"), String::new()),
    };
    LiteUrl { path, query }
}

// ── Bandwidth stamp ─────────────────────────────────────────────────────────
//
// Worker's bandwidth reporter posts here every ~5min. Stores the
// delta + timestamp on the edge_boxes row so the cameras UI can
// surface "outbound (5min)" per box without going to Airhouse.

#[tokio::test]
async fn post_bandwidth_stamps_edge_box_row() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (edge_box_id, bearer) = register_box(&db, ws_a, site_a).await;

    let body = json!({
        "delta_bytes": 1_234_567_890u64,
        "interval_s": 300u32,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/control/boxes/{edge_box_id}/bandwidth"))
                .header(AUTHORIZATION, format!("Bearer {bearer}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify it landed on the edge_boxes row.
    let row = oxy_cameras::entities::edge_boxes::Entity::find_by_id(edge_box_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.bandwidth_5min_bytes, Some(1_234_567_890));
    assert!(row.bandwidth_reported_at.is_some());
}

#[tokio::test]
async fn post_bandwidth_rejects_cross_box_bearer() {
    // Token issued for box_a can't stamp box_b's row.
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, bearer_a) = register_box(&db, ws_a, site_a).await;
    let (box_b, _) = register_box(&db, ws_a, site_a).await;
    assert_ne!(box_a, box_b);

    let body = json!({ "delta_bytes": 100u64, "interval_s": 300u32 });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/control/boxes/{box_b}/bandwidth"))
                .header(AUTHORIZATION, format!("Bearer {bearer_a}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ── Domain packs ────────────────────────────────────────────────────────────
//
// `packs::get_active` auto-seeds the `restaurant` starter on first
// read. We exercise the seed → list → update_body → set_active
// loop end-to-end against a real DB so the partial unique index +
// transaction discipline are covered.

#[tokio::test]
async fn packs_get_active_seeds_restaurant_starter() {
    use oxy_cameras::service::packs;
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;

    // First read: no pack exists → service inserts the restaurant
    // starter as is_active=true.
    let active = packs::get_active(&db, ws_a).await.expect("seed");
    assert_eq!(active.pack_key, "restaurant");
    assert_eq!(active.starter_key, "restaurant");
    // Bumping this assert when the restaurant starter version bumps
    // is fine — what we care about is "the seed used the latest."
    assert_eq!(
        active.starter_version,
        packs::starter::version_for("restaurant").unwrap()
    );
    assert!(active.is_active);
    assert!(!active.locally_modified);

    // Second read returns the same row, no duplicate insert.
    let second = packs::get_active(&db, ws_a).await.expect("re-read");
    assert_eq!(second.id, active.id);

    // list() returns exactly one row.
    let all = packs::list(&db, ws_a).await.expect("list");
    assert_eq!(all.len(), 1);
}

/// GET /camera-packs/active returns the active pack DTO. Exercises
/// the auto-seed path AND the body shape the UI consumes.
#[tokio::test]
async fn get_active_pack_returns_seeded_starter() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/camera-packs/active"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["pack_key"].as_str(), Some("restaurant"));
    assert_eq!(
        body["starter_version"].as_str(),
        Some(oxy_cameras::service::packs::starter::version_for("restaurant").unwrap())
    );
    assert_eq!(body["is_active"].as_bool(), Some(true));
    assert_eq!(body["locally_modified"].as_bool(), Some(false));
    assert!(body["body"]["roles"].is_array());
    let roles = body["body"]["roles"].as_array().unwrap();
    assert!(!roles.is_empty(), "starter body should ship with roles");
}

/// PUT /camera-packs/{key}/body replaces the body, validates the
/// templates, and flips locally_modified. A malformed template must
/// 400 with the role_key called out by the service-layer error.
#[tokio::test]
async fn put_pack_body_validates_and_persists() {
    use oxy_cameras::service::packs::body::PackBody;
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db.clone());

    // Seed the active pack by reading it.
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/camera-packs/active"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Valid edit: change the first role's hint.
    let mut body: PackBody = oxy_cameras::service::packs::starter::body_for("restaurant").unwrap();
    body.roles[0].hint = "Edited".into();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/{ws_a}/camera-packs/restaurant/body"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"body": body}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["locally_modified"].as_bool(), Some(true));
    assert_eq!(json["body"]["roles"][0]["hint"].as_str(), Some("Edited"));

    // Invalid template → 400 (render validation).
    let mut bad = oxy_cameras::service::packs::starter::body_for("restaurant").unwrap();
    bad.roles[0].prompt = "Hello {{ variables.nonexistent }}".into();
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/{ws_a}/camera-packs/restaurant/body"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"body": bad}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Exercises the fork → list → activate loop. The auto-seeded
/// restaurant starts active; we fork retail_lp (becomes active);
/// then activate back to restaurant. Asserts the partial unique
/// index never trips along the way.
#[tokio::test]
async fn fork_then_activate_round_trip() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db.clone());

    // Seed the active pack by reading it (auto-seeds restaurant).
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/camera-packs/active"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Fork retail_lp.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/camera-packs"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"starter_key": "retail_lp"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let new_pack = body_json(resp).await;
    assert_eq!(new_pack["pack_key"].as_str(), Some("retail_lp"));
    assert_eq!(new_pack["is_active"].as_bool(), Some(true));

    // List should show two packs; retail_lp active, restaurant inactive.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/camera-packs"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let list = body_json(resp).await;
    let arr = list.as_array().expect("array");
    assert_eq!(arr.len(), 2, "should have two packs after fork");
    let by_key: std::collections::HashMap<&str, bool> = arr
        .iter()
        .map(|p| {
            (
                p["pack_key"].as_str().unwrap(),
                p["is_active"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(by_key.get("retail_lp"), Some(&true));
    assert_eq!(by_key.get("restaurant"), Some(&false));

    // Activate restaurant.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/camera-packs/restaurant/activate"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let activated = body_json(resp).await;
    assert_eq!(activated["pack_key"].as_str(), Some("restaurant"));
    assert!(activated["is_active"].as_bool().unwrap());

    // Active endpoint now returns restaurant.
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/camera-packs/active"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let active = body_json(resp).await;
    assert_eq!(active["pack_key"].as_str(), Some("restaurant"));
}

/// Exercises the starter-updates flow end-to-end. We seed the
/// active pack (which auto-grabs the latest version), then
/// directly poke `starter_version` back to "0.0.0" so the diff
/// service reports a pending update. Then apply it via the route
/// and assert the version bumps.
#[tokio::test]
async fn starter_updates_flow_lists_and_applies() {
    use oxy_cameras::entities::domain_packs;
    use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db.clone());

    // Seed the active pack so the row exists.
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/camera-packs/active"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Roll `starter_version` back to a fake older value so the
    // diff service flags an update.
    let row = domain_packs::Entity::find()
        .filter(domain_packs::Column::WorkspaceId.eq(ws_a))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let row_id = row.id;
    let mut am: domain_packs::ActiveModel = row.into();
    am.starter_version = Set("0.0.0".into());
    am.update(&db).await.unwrap();

    // GET /camera-packs/starter-updates → one entry for restaurant.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/camera-packs/starter-updates"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let updates = body_json(resp).await;
    let arr = updates.as_array().expect("array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["pack_key"].as_str(), Some("restaurant"));
    assert_eq!(arr[0]["current_version"].as_str(), Some("0.0.0"));
    let upstream_version = oxy_cameras::service::packs::starter::version_for("restaurant").unwrap();
    assert_eq!(arr[0]["available_version"].as_str(), Some(upstream_version));

    // Apply the update with default actions (empty map → keep_mine
    // for existing roles, leave upstream-only out). The current pack
    // body IS the upstream body though (we only changed
    // starter_version), so the merge result will equal current,
    // which still render-validates fine. Result: starter_version
    // bumps to upstream, body stays the same.
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/camera-packs/restaurant/apply-update"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"actions": {}}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let after = body_json(resp).await;
    assert_eq!(after["starter_version"].as_str(), Some(upstream_version));

    // The row in the DB now reflects the bumped version.
    let refreshed = domain_packs::Entity::find_by_id(row_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refreshed.starter_version, upstream_version);
}

/// GET /camera-packs/starters returns the published library. Spot-
/// check the keys + that descriptions ship non-empty.
#[tokio::test]
async fn list_starters_returns_library() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/camera-packs/starters"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let arr = body_json(resp).await;
    let keys: Vec<&str> = arr
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["key"].as_str().unwrap())
        .collect();
    assert!(keys.contains(&"restaurant"));
    assert!(keys.contains(&"retail_lp"));
    assert!(keys.contains(&"factory_safety"));
    for s in arr.as_array().unwrap() {
        assert!(
            !s["description"].as_str().unwrap().is_empty(),
            "starter `{}` shipped without description",
            s["key"].as_str().unwrap()
        );
    }
}

#[tokio::test]
async fn packs_update_body_validates_and_flags_locally_modified() {
    use oxy_cameras::service::packs;
    use oxy_cameras::service::packs::body::PackBody;

    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    packs::get_active(&db, ws_a).await.unwrap();

    // Build a valid edit: change the kitchen hint, keep prompts.
    let mut body = packs::starter::body_for("restaurant").unwrap();
    body.roles[0].hint = "Edited".into();
    packs::update_body(&db, ws_a, "restaurant", body.clone())
        .await
        .expect("valid update");

    let row = packs::get_active(&db, ws_a).await.unwrap();
    assert!(row.locally_modified, "edit should flip locally_modified");
    let stored: PackBody = serde_json::from_value(row.body).unwrap();
    assert_eq!(stored.roles[0].hint, "Edited");

    // Now a bad edit: undefined variable in a prompt → 400.
    let mut bad = packs::starter::body_for("restaurant").unwrap();
    bad.roles[0].prompt = "Hello {{ variables.does_not_exist }}".into();
    let err = packs::update_body(&db, ws_a, "restaurant", bad)
        .await
        .unwrap_err();
    assert!(
        matches!(err, oxy_cameras::service::ServiceError::InvalidInput(_)),
        "got: {err:?}"
    );
}

// ── Phase 1B + 1C — full claim → bootstrap → /control ─────────────────────

/// Helper: enroll a device via the service layer (the public
/// `POST /fleet/factory-enroll` route has been removed) and
/// return (device_id, secret, claim_code).
async fn enroll_test_device(db: &DatabaseConnection) -> (Uuid, Vec<u8>, String) {
    let device_id = Uuid::new_v4();
    let secret = oxy_cameras::service::fleet::generate_device_secret();
    let input = oxy_cameras::service::fleet::EnrollInput {
        device_id,
        device_secret: &secret,
        sku: "oxy-edge-v1",
        hardware_revision: "rev-A",
        manufactured_at: chrono::Utc::now(),
    };
    let out = oxy_cameras::service::fleet::enroll_device(db, b"test-salt", input)
        .await
        .expect("enroll device");
    (device_id, secret, out.claim_code)
}

// ── IoT Phase 2 — software-update channel ───────────────────────────────────
//
// Operator sets target_image_tag via the workspace-scoped PATCH route.
// Device reads it via /control/boxes/{id}/target (bearer-auth). Device
// reports back current_image_tag + last_update_result through the
// health endpoint after applying.

#[tokio::test]
async fn update_channel_set_target_then_device_reads_then_reports_back() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());
    let (edge_box_id, bearer) = register_box(&db, ws_a, site_a).await;

    // Operator sets target.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/edge-boxes/{edge_box_id}/target"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"target_image_tag": "ghcr.io/oxy/edge:3.4.2"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(
        body["target_image_tag"].as_str(),
        Some("ghcr.io/oxy/edge:3.4.2")
    );

    // Device polls /target.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/control/boxes/{edge_box_id}/target"))
                .header(AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let target = body_json(resp).await;
    assert_eq!(
        target["target_image_tag"].as_str(),
        Some("ghcr.io/oxy/edge:3.4.2")
    );
    assert!(target["current_image_tag"].is_null());

    // Device reports back via the health endpoint.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/control/boxes/{edge_box_id}/health"))
                .header(AUTHORIZATION, format!("Bearer {bearer}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "image_tag": "ghcr.io/oxy/edge:3.4.2",
                        "last_update_result": "applied",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    // With write-coalescing on (the default), the health POST buffers the row
    // and returns 202 Accepted. 503 stays acceptable for the synchronous path
    // (Airhouse ingest can be unconfigured in the test env); either way the
    // edge_boxes row is still stamped (we run record_health_update first).
    assert!(
        resp.status() == StatusCode::ACCEPTED || resp.status() == StatusCode::SERVICE_UNAVAILABLE,
        "got {}",
        resp.status()
    );

    let row = oxy_cameras::entities::edge_boxes::Entity::find_by_id(edge_box_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.current_image_tag.as_deref(),
        Some("ghcr.io/oxy/edge:3.4.2")
    );
    assert_eq!(row.last_update_result.as_deref(), Some("applied"));
    assert!(row.last_update_at.is_some());
}

#[tokio::test]
async fn update_channel_patch_target_rejects_cross_workspace() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());

    let (edge_box_b, _bearer) = register_box(&db, ws_b, site_b).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/edge-boxes/{edge_box_b}/target"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"target_image_tag": "evil-tag"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn update_channel_patch_target_rejects_empty_string() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());
    let (edge_box_id, _bearer) = register_box(&db, ws_a, site_a).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/edge-boxes/{edge_box_id}/target"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"target_image_tag": ""}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn update_channel_get_target_rejects_cross_box_bearer() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (_box_a, bearer_a) = register_box(&db, ws_a, site_a).await;
    let (box_b, _bearer_b) = register_box(&db, ws_a, site_a).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/control/boxes/{box_b}/target"))
                .header(AUTHORIZATION, format!("Bearer {bearer_a}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ── Bulk set-target ────────────────────────────────────────────────────────

#[tokio::test]
async fn bulk_set_target_updates_all_boxes_in_workspace() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (box1, _) = register_box(&db, ws_a, site_a).await;
    let (box2, _) = register_box(&db, ws_a, site_a).await;
    let (box3, _) = register_box(&db, ws_a, site_a).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/edge-boxes/bulk/target"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "box_ids": [box1, box2, box3],
                        "target_image_tag": "ghcr.io/oxy/edge:5.0.0",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["updated_count"].as_u64(), Some(3));

    for box_id in [box1, box2, box3] {
        let row = oxy_cameras::entities::edge_boxes::Entity::find_by_id(box_id)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.target_image_tag.as_deref(),
            Some("ghcr.io/oxy/edge:5.0.0")
        );
    }
}

#[tokio::test]
async fn bulk_set_target_rejects_cross_workspace_atomically() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let (ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, _) = register_box(&db, ws_a, site_a).await;
    let (box_b, _) = register_box(&db, ws_b, site_b).await;

    // Caller in ws_a sneaks ws_b's box into the list. The whole call
    // must reject — partial updates would leave the operator with
    // half their fleet on a new image.
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/edge-boxes/bulk/target"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "box_ids": [box_a, box_b],
                        "target_image_tag": "evil",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // No row touched.
    let row_a = oxy_cameras::entities::edge_boxes::Entity::find_by_id(box_a)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(row_a.target_image_tag.is_none());
    let row_b = oxy_cameras::entities::edge_boxes::Entity::find_by_id(box_b)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(row_b.target_image_tag.is_none());
}

#[tokio::test]
async fn bulk_set_target_unknown_id_404s_without_touching_others() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, _) = register_box(&db, ws_a, site_a).await;
    let bogus_id = Uuid::new_v4();

    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/edge-boxes/bulk/target"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "box_ids": [box_a, bogus_id],
                        "target_image_tag": "ghcr.io/oxy/edge:5.0.0",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let row = oxy_cameras::entities::edge_boxes::Entity::find_by_id(box_a)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.target_image_tag.is_none());
}

#[tokio::test]
async fn bulk_set_target_empty_list_is_noop() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/edge-boxes/bulk/target"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "box_ids": [],
                        "target_image_tag": "ghcr.io/oxy/edge:5.0.0",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["updated_count"].as_u64(), Some(0));
}

#[tokio::test]
async fn bulk_set_target_deduplicates_repeated_ids() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (box_a, _) = register_box(&db, ws_a, site_a).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/edge-boxes/bulk/target"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "box_ids": [box_a, box_a, box_a],
                        "target_image_tag": "ghcr.io/oxy/edge:5.0.0",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    // Dedup matters — count reflects unique IDs, not input length.
    assert_eq!(body["updated_count"].as_u64(), Some(1));
}

// ── Manual site + camera provisioning ──────────────────────────────────────

#[tokio::test]
async fn create_site_persists_with_manual_source() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/cameras/sites"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "name": "Pokehouse Soma",
                        "timezone": "America/Los_Angeles",
                        "region": "us-west",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["name"].as_str(), Some("Pokehouse Soma"));
    assert_eq!(body["source"].as_str(), Some("manual"));
    assert_eq!(body["timezone"].as_str(), Some("America/Los_Angeles"));
    assert_eq!(body["region"].as_str(), Some("us-west"));
}

#[tokio::test]
async fn create_site_rejects_duplicate_name_in_same_workspace() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let req = || {
        Request::builder()
            .method("POST")
            .uri(format!("/{ws_a}/cameras/sites"))
            .header("content-type", "application/json")
            .body(Body::from(json!({"name": "Pokehouse Soma"}).to_string()))
            .unwrap()
    };
    let resp1 = app.clone().oneshot(req()).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);
    let resp2 = app.oneshot(req()).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn create_site_rejects_empty_name() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/cameras/sites"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"name": "   "}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_camera_attaches_to_owned_site() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/cameras"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "site_id": site_a,
                        "name": "Kitchen line",
                        "rtsp_url": "rtsp://10.0.0.42:554/Streaming/Channels/101",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["name"].as_str(), Some("Kitchen line"));
    assert_eq!(
        body["rtsp_url"].as_str(),
        Some("rtsp://10.0.0.42:554/Streaming/Channels/101")
    );
    // edge_box_id null at creation; the auto-claim path will bind
    // it on the next config poll from a box at the same site.
    assert!(body["edge_box_id"].is_null());
}

#[tokio::test]
async fn create_camera_rejects_cross_workspace_site() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (_ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/cameras"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "site_id": site_b,
                        "name": "Sneaky",
                        "rtsp_url": "rtsp://10.0.0.42:554/Streaming/Channels/101",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_camera_rejects_empty_rtsp_url() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/cameras"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "site_id": site_a,
                        "name": "Kitchen line",
                        "rtsp_url": "  ",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_camera_rejects_duplicate_name_at_same_site() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let req = || {
        Request::builder()
            .method("POST")
            .uri(format!("/{ws_a}/cameras"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "site_id": site_a,
                    "name": "Kitchen line",
                    "rtsp_url": "rtsp://10.0.0.42:554/Streaming/Channels/101",
                })
                .to_string(),
            ))
            .unwrap()
    };
    let resp1 = app.clone().oneshot(req()).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);
    let resp2 = app.oneshot(req()).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::CONFLICT);
}

// ── Cost rollup ─────────────────────────────────────────────────────────────
//
// Without an Airhouse broker the rollup short-circuits to "empty"
// (cost::aggregate_by_model treats a missing table as no-data).
// We exercise the workspace-scoped ownership chain + range
// validation here; the actual SUM math runs in
// `service::cost::tests` against synthetic inputs.

#[tokio::test]
async fn get_edge_box_cost_short_circuits_when_no_cameras() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let (edge_box_id, _bearer) = register_box(&db, ws_a, site_a).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/edge-boxes/{edge_box_id}/cost"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["range"].as_str(), Some("24h"));
    assert_eq!(body["reports"].as_i64(), Some(0));
    assert_eq!(body["cost_usd_micros"].as_i64(), Some(0));
    assert!(body["by_model"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn get_edge_box_cost_rejects_cross_workspace() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());

    let (edge_box_b, _bearer) = register_box(&db, ws_b, site_b).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/edge-boxes/{edge_box_b}/cost"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn get_edge_box_cost_rejects_invalid_range() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());
    let (edge_box_id, _bearer) = register_box(&db, ws_a, site_a).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{ws_a}/cameras/edge-boxes/{edge_box_id}/cost?range=42d"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Axum's Query extractor returns 400 on a serde failure.
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_workspace_cost_short_circuits_when_empty() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/cost?range=7d"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["range"].as_str(), Some("7d"));
    assert_eq!(body["reports"].as_i64(), Some(0));
    assert!(body["by_box"].as_array().unwrap().is_empty());
}

// ── Device logs (Phase 4b) ──────────────────────────────────────────────────
//
// Without Airhouse the read path collapses to empty; the write
// path 503s at the broker. We cover the workspace-scoped
// ownership chain + severity validation here; the ingest/read
// SQL exercises against a synthetic tenant in service::logs::tests.

#[tokio::test]
async fn get_edge_box_logs_short_circuits_when_no_data() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());
    let (edge_box_id, _bearer) = register_box(&db, ws_a, site_a).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/edge-boxes/{edge_box_id}/logs"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn get_edge_box_logs_rejects_cross_workspace() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());
    let (edge_box_b, _bearer) = register_box(&db, ws_b, site_b).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/edge-boxes/{edge_box_b}/logs"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn post_logs_rejects_cross_box_bearer() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());
    let (_box_a, bearer_a) = register_box(&db, ws_a, site_a).await;
    let (box_b, _bearer_b) = register_box(&db, ws_a, site_a).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/control/boxes/{box_b}/logs"))
                .header(AUTHORIZATION, format!("Bearer {bearer_a}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "logs": [{
                            "ts": chrono::Utc::now(),
                            "severity": "info",
                            "event": "camera.vlm_call",
                            "fields_json": "{}"
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn post_logs_rejects_invalid_severity() {
    assert_airhouse_disabled_in_env();
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());
    let (edge_box_id, bearer) = register_box(&db, ws_a, site_a).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/control/boxes/{edge_box_id}/logs"))
                .header(AUTHORIZATION, format!("Bearer {bearer}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "logs": [{
                            "ts": chrono::Utc::now(),
                            "severity": "CRITICAL",
                            "event": "camera.vlm_call",
                            "fields_json": "{}"
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ── Budget (Phase 4c) ───────────────────────────────────────────────────────
//
// Smoke covers the ownership chain + the read/write round-trip,
// validation rejects, and that the empty-workspace status comes
// back with sane defaults. Projection math lives in
// service::budget::tests.

#[tokio::test]
async fn get_budget_status_defaults_when_unset() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/budget"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(body["budget_micros"].is_null());
    assert_eq!(body["month_to_date_micros"].as_i64(), Some(0));
    assert_eq!(body["projected_month_micros"].as_i64(), Some(0));
    assert!(body["percent_consumed"].is_null());
    assert!(body["percent_projected"].is_null());
    assert!(body["day_of_month"].as_i64().unwrap() >= 1);
    assert!(body["days_in_month"].as_i64().unwrap() >= 28);
}

#[tokio::test]
async fn patch_budget_round_trips() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/budget"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "monthly_budget_micros": 50_000_000_i64 }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let set_body = body_json(resp).await;
    assert_eq!(set_body["monthly_budget_micros"].as_i64(), Some(50_000_000));

    // Read it back via GET.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/cameras/budget"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["budget_micros"].as_i64(), Some(50_000_000));

    // Clear it.
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/budget"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "monthly_budget_micros": null }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(body["monthly_budget_micros"].is_null());
}

#[tokio::test]
async fn patch_budget_rejects_negative() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/budget"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "monthly_budget_micros": -1_i64 }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ── Phase 5 — fleet device listing ──────────────────────────────────────────

#[tokio::test]
async fn list_fleet_devices_returns_empty_for_unbound_workspace() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/fleet/devices"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn list_fleet_devices_surfaces_pending_claim() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());

    // Operator "Add device" → pending_claim row.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/fleet/prepared-devices"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"site_id": site_a}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let prep = body_json(resp).await;
    let device_id: Uuid = prep["device_id"].as_str().unwrap().parse().unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/fleet/devices"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(
        row["device_id"].as_str().unwrap().parse::<Uuid>().unwrap(),
        device_id
    );
    assert_eq!(row["status"].as_str(), Some("pending_claim"));
    assert_eq!(
        row["site_id"].as_str().unwrap().parse::<Uuid>().unwrap(),
        site_a
    );
    assert!(row["site_name"].as_str().is_some());
    assert!(row["edge_box_id"].is_null());
}

#[tokio::test]
async fn list_fleet_devices_scoped_to_workspace() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let (ws_b, _site_b) = seed_site(&db).await;
    let app = router(db.clone());

    // Prepare a device in ws_a.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/fleet/prepared-devices"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"site_id": site_a}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // ws_b's list must NOT see ws_a's claim.
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_b}/fleet/devices"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 0);
}

// ── Retroactive claim — attach a self-enrolled device to an
//    operator-registered edge_box ─────────────────────────────────

#[tokio::test]
async fn bind_existing_succeeds_for_clean_pair() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());
    let edge_box_id = register_box_unbound(&db, ws_a, site_a).await;
    let (device_id, _secret, _code) = enroll_test_device(&db).await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/fleet/claims/bind-existing"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "device_id": device_id, "edge_box_id": edge_box_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["status"].as_str(), Some("claimed"));
    assert_eq!(
        body["device_id"].as_str().unwrap().parse::<Uuid>().unwrap(),
        device_id
    );

    // Verify the device shows up in the fleet list as claimed with
    // edge_box_id set — this is the operator-visible signal that
    // the retroactive bind landed.
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_a}/fleet/devices"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let rows = body_json(resp).await;
    let row = &rows.as_array().unwrap()[0];
    assert_eq!(row["status"].as_str(), Some("claimed"));
    assert_eq!(
        row["edge_box_id"]
            .as_str()
            .unwrap()
            .parse::<Uuid>()
            .unwrap(),
        edge_box_id
    );
}

#[tokio::test]
async fn bind_existing_rejects_cross_workspace_edge_box() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (_ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());
    let edge_box_b = register_box_unbound(&db, _ws_b, site_b).await;
    let (device_id, _secret, _code) = enroll_test_device(&db).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/fleet/claims/bind-existing"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "device_id": device_id, "edge_box_id": edge_box_b }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn bind_existing_404s_for_unenrolled_device() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());
    let edge_box_id = register_box_unbound(&db, ws_a, site_a).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/fleet/claims/bind-existing"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "device_id": Uuid::new_v4(), "edge_box_id": edge_box_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bind_existing_409s_when_edge_box_already_linked() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());
    let edge_box_id = register_box_unbound(&db, ws_a, site_a).await;
    let (device_a, _, _) = enroll_test_device(&db).await;
    let (device_b, _, _) = enroll_test_device(&db).await;

    // First bind goes through.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/fleet/claims/bind-existing"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "device_id": device_a, "edge_box_id": edge_box_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Second attempt with a different device hitting the same
    // edge_box conflicts.
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/fleet/claims/bind-existing"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "device_id": device_b, "edge_box_id": edge_box_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

// ── JWT auth path ──────────────────────────────────────────────────────────
//
// These tests exercise the per-device JWT path end-to-end against
// /control/boxes/{id}/config (a representative GET behind the
// middleware). `register_box` already mints a JWT-backed credential
// via `enroll_test_device` + `bind_existing`; the additional tests
// below cover the rejection paths.

fn mint_test_jwt(device_id: Uuid, secret: &[u8]) -> String {
    let exp = chrono::Utc::now().timestamp() + 3600;
    oxy_cameras::auth::jwt::sign(device_id, secret, exp)
}

#[tokio::test]
async fn jwt_auth_rejects_expired_token() {
    assert_airhouse_disabled_in_env();
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let app = router(db.clone());
    let (edge_box_id, _jwt) = register_box(&db, workspace_id, site_id).await;

    // Look up the device_id from the materialized claim so we can
    // sign a deliberately expired token against the right secret.
    let claim = oxy_cameras::entities::device_claims::Entity::find()
        .filter(oxy_cameras::entities::device_claims::Column::EdgeBoxId.eq(edge_box_id))
        .one(&db)
        .await
        .unwrap()
        .expect("claim row");
    let device_id = claim.device_id;
    let device = oxy_cameras::entities::device_registry::Entity::find_by_id(device_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let secret = oxy_cameras::secrets::open(&device.device_secret).unwrap();

    let expired_exp = chrono::Utc::now().timestamp() - 600;
    let jwt = oxy_cameras::auth::jwt::sign(device_id, &secret, expired_exp);
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/control/boxes/{edge_box_id}/config"))
                .header(AUTHORIZATION, format!("Bearer {jwt}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn jwt_auth_rejects_signature_with_wrong_secret() {
    assert_airhouse_disabled_in_env();
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let app = router(db.clone());
    let (edge_box_id, _jwt) = register_box(&db, workspace_id, site_id).await;
    let claim = oxy_cameras::entities::device_claims::Entity::find()
        .filter(oxy_cameras::entities::device_claims::Column::EdgeBoxId.eq(edge_box_id))
        .one(&db)
        .await
        .unwrap()
        .expect("claim row");

    // Sign with the wrong secret — looks structurally valid, fails
    // verification.
    let other_secret = oxy_cameras::service::fleet::generate_device_secret();
    let jwt = mint_test_jwt(claim.device_id, &other_secret);
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/control/boxes/{edge_box_id}/config"))
                .header(AUTHORIZATION, format!("Bearer {jwt}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn jwt_auth_rejects_when_no_active_claim() {
    assert_airhouse_disabled_in_env();
    let db = test_db().await;
    let (_workspace_id, _site_id) = seed_site(&db).await;
    let app = router(db.clone());

    // Enroll a device but never claim it. JWT signs correctly but
    // has no active claim → 401.
    let (device_id, secret, _claim_code) = enroll_test_device(&db).await;
    let jwt = mint_test_jwt(device_id, &secret);
    // Hit any /control/boxes/{id}/config — the box_id won't match
    // anything, and we never get past the claim lookup anyway.
    let any_box_id = Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/control/boxes/{any_box_id}/config"))
                .header(AUTHORIZATION, format!("Bearer {jwt}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── Tier B #7 — audit log ───────────────────────────────────────────────────

#[tokio::test]
async fn audit_log_records_set_target() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let app = router(db.clone());
    let (edge_box_id, _bearer) = register_box(&db, ws_a, _site_a).await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/{ws_a}/cameras/edge-boxes/{edge_box_id}/target"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"target_image_tag": "oxy-edge:v1.2.3"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{ws_a}/cameras/audit-events?action_prefix=edge_box.set_target"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["action"].as_str(), Some("edge_box.set_target"));
    assert_eq!(
        row["target_id"].as_str().unwrap().parse::<Uuid>().unwrap(),
        edge_box_id
    );
    assert_eq!(
        row["details_json"]["target_image_tag"].as_str(),
        Some("oxy-edge:v1.2.3")
    );
}

#[tokio::test]
async fn audit_log_scoped_per_workspace() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let (ws_b, _site_b) = seed_site(&db).await;
    let app = router(db.clone());
    let (_edge_box_id, _bearer) = register_box(&db, ws_a, site_a).await;

    // ws_b's audit feed should be empty even though ws_a has an event.
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_b}/cameras/audit-events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn audit_log_records_bind_existing() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let app = router(db.clone());
    let edge_box_id = register_box_unbound(&db, ws_a, site_a).await;
    let (device_id, _secret, _code) = enroll_test_device(&db).await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/fleet/claims/bind-existing"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "device_id": device_id, "edge_box_id": edge_box_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{ws_a}/cameras/audit-events?action_prefix=device."
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["action"].as_str(), Some("device.bind_existing"));
}

// ── Tier B #6 follow-up — presigned URL clip archival ────────────────────────

#[tokio::test]
async fn sign_clip_503s_when_s3_disabled() {
    assert_airhouse_disabled_in_env();
    // SAFETY: tests run single-threaded under nextest's default
    // and this test owns the env var.
    unsafe {
        std::env::remove_var("OXY_CAMERAS_CLIPS_S3_BUCKET");
    }
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let app = router(db.clone());
    let (edge_box_id, bearer) = register_box(&db, workspace_id, site_id).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/clips/sign")
                .header(AUTHORIZATION, format!("Bearer {bearer}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "report_id": Uuid::new_v4(),
                        "segment_start": chrono::Utc::now(),
                        "content_length": 1024,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let _ = edge_box_id;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn get_clip_url_rejects_cross_workspace_key() {
    // SAFETY: see above.
    unsafe {
        std::env::set_var("OXY_CAMERAS_CLIPS_S3_BUCKET", "test-bucket");
    }
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (ws_b, _site_b) = seed_site(&db).await;
    let app = router(db.clone());

    // Operator at ws_a tries to fetch a key that lives under
    // ws_b's prefix. The clips service rejects with 403 even
    // though we never reach S3.
    let foreign_key = format!("compliance/{}/2026-05-30/abc.mp4", ws_b);
    let report_id = Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{ws_a}/cameras/clips/{report_id}/url?key={foreign_key}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("OXY_CAMERAS_CLIPS_S3_BUCKET");
    }
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn external_clip_route_mounted_and_rejects_cross_workspace_key() {
    // Guards `oxy_cameras::routes::external_clip_routes` — the builder the app
    // crate merges into the EXTERNAL API surface so customer apps (not just the
    // operator UI) can play clips. Asserts the /cameras/clips/{id}/url route is
    // present AND keeps the operator route's cross-tenant guard. (The app-crate
    // `.merge` that mounts it can't be unit-tested here — it sits behind
    // API-key auth that 401s before routing — so this pins the builder +
    // tenancy; workspace.rs carries a comment against un-mounting it.)
    // SAFETY: nextest runs each test in its own process, so this write has no
    // concurrent reader. clips::config() latches S3Config into a process-wide
    // OnceLock, so the set_var only takes effect *because of* that per-test
    // isolation (a fresh process ⇒ unlatched cache); the trailing remove_var
    // is cosmetic — the cache has already latched by the time we hit it.
    unsafe {
        std::env::set_var("OXY_CAMERAS_CLIPS_S3_BUCKET", "test-bucket");
    }
    let ws_a = Uuid::new_v4();
    let ws_b = Uuid::new_v4();
    // Raw external builder, nested under /{workspace_id} exactly as
    // build_external_workspace_routes mounts it — no auth/db layer.
    let app = axum::Router::<()>::new().nest(
        "/{workspace_id}",
        oxy_cameras::routes::external_clip_routes::<()>(),
    );
    let foreign_key = format!("compliance/{}/2026-05-30/abc.mp4", ws_b);
    let report_id = Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/{ws_a}/cameras/clips/{report_id}/url?key={foreign_key}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("OXY_CAMERAS_CLIPS_S3_BUCKET");
    }
    // 403 (not 404) proves the route is mounted AND rejects the foreign key.
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ── Camera health alerting ─────────────────────────────────────────────────

#[tokio::test]
async fn camera_health_summary_empty_workspace() {
    let db = test_db().await;
    let (workspace_id, _site_id) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{workspace_id}/cameras/health-summary"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn camera_health_summary_scoped_per_workspace() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (ws_b, _site_b) = seed_site(&db).await;
    let app = router(db.clone());

    // ws_b's feed must be empty even if ws_a had cameras.
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_b}/cameras/health-summary"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = ws_a;
    assert!(body_json(resp).await.as_array().unwrap().is_empty());
}

// ── Operator zone approval workflow ─────────────────────────────────────────

#[tokio::test]
async fn propose_geometry_writes_pending_proposal() {
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let cam_id = seed_camera(&db, site_id, None, "test-cam").await;
    let app = router(db.clone());

    let proposed = json!([{
        "zone_id": "z1",
        "name": "Fryer",
        "polygon": [[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]],
    }]);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/{workspace_id}/cameras/{cam_id}/proposed-geometry"
                ))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "zones_json": proposed }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["proposed_zones_json"], proposed);
    assert!(body["proposed_at"].is_string());
    // Live zones still empty — proposal doesn't touch them.
    assert_eq!(body["zones_json"], json!([]));
}

#[tokio::test]
async fn approve_proposed_geometry_promotes_and_clears() {
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let cam_id = seed_camera(&db, site_id, None, "test-cam").await;
    let app = router(db.clone());

    let proposed = json!([{ "zone_id": "z1", "name": "X", "polygon": [[0.0, 0.0]] }]);
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/{workspace_id}/cameras/{cam_id}/proposed-geometry"
                ))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "zones_json": proposed }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/{workspace_id}/cameras/{cam_id}/proposed-geometry/approve"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    // Live zones now carry the proposed payload.
    assert_eq!(body["zones_json"], proposed);
    // Proposal cleared.
    assert!(body["proposed_zones_json"].is_null());
    assert!(body["proposed_at"].is_null());
}

#[tokio::test]
async fn reject_proposed_geometry_clears_without_promoting() {
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let cam_id = seed_camera(&db, site_id, None, "test-cam").await;
    let app = router(db.clone());

    let proposed = json!([{ "zone_id": "z1", "name": "X", "polygon": [[0.0, 0.0]] }]);
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/{workspace_id}/cameras/{cam_id}/proposed-geometry"
                ))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "zones_json": proposed }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/{workspace_id}/cameras/{cam_id}/proposed-geometry/reject"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    // Live zones untouched (still empty).
    assert_eq!(body["zones_json"], json!([]));
    assert!(body["proposed_zones_json"].is_null());
}

#[tokio::test]
async fn approve_with_no_proposal_400s() {
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let cam_id = seed_camera(&db, site_id, None, "test-cam").await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/{workspace_id}/cameras/{cam_id}/proposed-geometry/approve"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn propose_geometry_cross_workspace_403() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (_ws_b, site_b) = seed_site(&db).await;
    let cam_b = seed_camera(&db, site_b, None, "in-ws-b").await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/{ws_a}/cameras/{cam_b}/proposed-geometry"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "zones_json": [] }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ── Add-device wizard (Option 3) ────────────────────────────────────────────

#[tokio::test]
async fn prepare_device_returns_identity_and_pending_claim() {
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{workspace_id}/fleet/prepared-devices"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "site_id": site_id, "hardware_label": "Intel N100" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let device_id = body["device_id"].as_str().unwrap().parse::<Uuid>().unwrap();
    assert!(!body["device_secret_b64"].as_str().unwrap().is_empty());
    assert!(!body["claim_code"].as_str().unwrap().is_empty());
    assert_eq!(body["sku"].as_str(), Some("self-installed"));
    assert_eq!(body["hardware_revision"].as_str(), Some("Intel N100"));
    let snippet = body["install_snippet"].as_str().unwrap();
    assert!(snippet.contains("/var/lib/oxy/device.json"));
    assert!(snippet.contains(&device_id.to_string()));

    // Should immediately appear in the fleet listing as
    // pending_claim — the wizard creates the claim atomically.
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{workspace_id}/fleet/devices"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let rows = body_json(resp).await;
    let arr = rows.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(
        arr[0]["device_id"]
            .as_str()
            .unwrap()
            .parse::<Uuid>()
            .unwrap(),
        device_id
    );
    assert_eq!(arr[0]["status"].as_str(), Some("pending_claim"));
}

#[tokio::test]
async fn prepare_device_rejects_cross_workspace_site() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (_ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{ws_a}/fleet/prepared-devices"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "site_id": site_b }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn prepare_device_defaults_hardware_revision_when_blank() {
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{workspace_id}/fleet/prepared-devices"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "site_id": site_id, "hardware_label": "   " }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["hardware_revision"].as_str(), Some("unspecified"));
}

#[tokio::test]
async fn prepared_device_can_bootstrap_directly() {
    // End-to-end: prepare the device server-side, then drive
    // /fleet/bootstrap on its behalf as if the worker's first
    // boot just happened. Should materialize an edge_boxes row +
    // flip the claim to `claimed`.
    use base64::Engine as _;
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let app = router(db.clone());

    let prep = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{workspace_id}/fleet/prepared-devices"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "site_id": site_id }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let prep_body = body_json(prep).await;
    let device_id: Uuid = prep_body["device_id"].as_str().unwrap().parse().unwrap();
    let secret_b64 = prep_body["device_secret_b64"].as_str().unwrap();
    let secret = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(secret_b64)
        .unwrap();

    // Compute the bootstrap signature the way the worker would.
    let ts = chrono::Utc::now().timestamp();
    let sig = oxy_cameras::service::fleet::expected_announce_signature(&secret, device_id, ts);
    let sig_b64 = base64::engine::general_purpose::STANDARD_NO_PAD.encode(&sig);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/fleet/bootstrap/{device_id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "timestamp": ts, "signature_b64": sig_b64 }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let boot = body_json(resp).await;
    assert!(boot["edge_box_id"].as_str().is_some());
    assert_eq!(
        boot["workspace_id"]
            .as_str()
            .unwrap()
            .parse::<Uuid>()
            .unwrap(),
        workspace_id
    );
}

// ── Merged fleet view ───────────────────────────────────────────────────────

#[tokio::test]
async fn fleet_list_surfaces_pending_claim_without_edge_box() {
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let app = router(db.clone());

    // "Add device" — pending claim with no edge_box yet.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{workspace_id}/fleet/prepared-devices"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "site_id": site_id, "hardware_label": "test box" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{workspace_id}/cameras/fleet"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert!(row["edge_box"].is_null(), "no edge_box yet");
    assert_eq!(row["claim_status"].as_str(), Some("pending_claim"));
    assert!(row["device_id"].is_string());
    assert!(row["claim_code"].is_string());
    assert_eq!(row["hardware_revision"].as_str(), Some("test box"));
    assert_eq!(row["site_name"].as_str(), Some("smoke-site"));
}

#[tokio::test]
async fn fleet_list_returns_claimed_edge_box() {
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let app = router(db.clone());
    let (edge_box_id, _jwt) = register_box(&db, workspace_id, site_id).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{workspace_id}/cameras/fleet"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(resp).await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1, "one row for the registered box");
    assert_eq!(rows[0]["claim_status"].as_str(), Some("claimed"));
    assert_eq!(
        rows[0]["edge_box"]["id"]
            .as_str()
            .unwrap()
            .parse::<Uuid>()
            .unwrap(),
        edge_box_id
    );
    assert!(rows[0]["device_id"].is_string());
}

#[tokio::test]
async fn fleet_list_scoped_per_workspace() {
    let db = test_db().await;
    let (ws_a, site_a) = seed_site(&db).await;
    let (ws_b, _site_b) = seed_site(&db).await;
    let app = router(db.clone());
    let (_edge_box_id, _bearer) = register_box(&db, ws_a, site_a).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{ws_b}/cameras/fleet"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert!(body.as_array().unwrap().is_empty());
}

// ── Remove device ──────────────────────────────────────────────────────────

#[tokio::test]
async fn remove_member_retires_edge_box_and_revokes_claim() {
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let app = router(db.clone());
    let (edge_box_id, _jwt) = register_box(&db, workspace_id, site_id).await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/{workspace_id}/cameras/fleet"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "edge_box_id": edge_box_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["edge_box_retired"].as_bool(), Some(true));
    // `register_box` binds a device claim alongside the edge_box, so
    // removing the box also revokes the linked claim.
    assert_eq!(body["claim_revoked"].as_bool(), Some(true));

    // Fleet list should no longer include the retired box.
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{workspace_id}/cameras/fleet"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let rows = body_json(resp).await;
    assert!(rows.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn remove_member_revokes_pending_claim() {
    let db = test_db().await;
    let (workspace_id, site_id) = seed_site(&db).await;
    let app = router(db.clone());

    // Pre-claim a device — pending_claim with no edge_box.
    let prep = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{workspace_id}/fleet/prepared-devices"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "site_id": site_id }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let prep_body = body_json(prep).await;
    let device_id: Uuid = prep_body["device_id"].as_str().unwrap().parse().unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/{workspace_id}/cameras/fleet"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "device_id": device_id }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["claim_revoked"].as_bool(), Some(true));
    assert_eq!(body["edge_box_retired"].as_bool(), Some(false));

    // Pending row is gone from the merged list.
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/{workspace_id}/cameras/fleet"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let rows = body_json(resp).await;
    assert!(rows.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn remove_member_rejects_cross_workspace_edge_box() {
    let db = test_db().await;
    let (ws_a, _site_a) = seed_site(&db).await;
    let (ws_b, site_b) = seed_site(&db).await;
    let app = router(db.clone());
    let (edge_box_b, _bearer) = register_box(&db, ws_b, site_b).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/{ws_a}/cameras/fleet"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "edge_box_id": edge_box_b }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn remove_member_requires_some_id() {
    let db = test_db().await;
    let (workspace_id, _site_id) = seed_site(&db).await;
    let app = router(db.clone());

    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/{workspace_id}/cameras/fleet"))
                .header("content-type", "application/json")
                .body(Body::from(json!({}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
