//! Integration tests for `AirhouseTokenBroker`.
//!
//! Spins up a Postgres testcontainer, seeds an `airhouse_tenants` row with a
//! sealed SA bearer, and drives the broker against a wiremock-backed
//! Admin API.
//!
//! Run with: `cargo nextest run -p oxy-app --test airhouse -E 'test(airhouse_broker)'`

use airhouse::entity::tenants::{self as airhouse_tenants, TenantStatus};
use airhouse::{
    AirhouseAdminClient, AirhouseTokenBroker, BrokerError, BrokerSubject, SystemPurpose, UserRole,
};
use base64::{Engine as _, engine::general_purpose};
use chrono::Utc;
use entity::organizations;
use entity::workspaces::{self, WorkspaceStatus};
use oxy::adapters::secrets::envelope;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::time::Duration;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Seed the AES-GCM master key. Same fixed key as the provisioner tests so
/// envelope::open in the broker can decrypt what we sealed in seed_tenant.
pub(super) fn set_test_encryption_key() {
    let _g = ENV_LOCK.lock().unwrap();
    // SAFETY: single-threaded test guarded by ENV_LOCK; deterministic key.
    unsafe {
        std::env::set_var(
            "OXY_ENCRYPTION_KEY",
            general_purpose::STANDARD.encode([7u8; 32]),
        );
    }
}

/// A fresh per-test database carrying the central + airhouse schema, with the
/// global `establish_connection()` pool pointed at it.
///
/// The broker resolves its own connection through `establish_connection()`, so
/// the env write is load-bearing: without it `mint` fails with "OXY_DATABASE_URL
/// environment variable is required" rather than seeing the seeded rows.
///
/// The migration chain runs once per `cargo nextest run` into a template that
/// this clones; see `tests/common/mod.rs`.
pub(super) async fn test_db() -> DatabaseConnection {
    let (db, test_url) = crate::common::fresh_db(crate::common::Schema::CentralAirhouse).await;
    // SAFETY: single-threaded test setup before any other env access. nextest
    // runs each test in its own process, so this cannot race a sibling.
    unsafe { std::env::set_var("OXY_DATABASE_URL", &test_url) };
    db
}

pub(super) async fn seed_workspace(db: &DatabaseConnection, name: &str) -> Uuid {
    let now = Utc::now().fixed_offset();
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set(name.to_string()),
        slug: ActiveValue::Set(format!("{name}-{}", org_id.simple())),
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
        name: ActiveValue::Set(name.to_string()),
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
    }
    .insert(db)
    .await
    .expect("seed workspace");

    workspace_id
}

/// Seed an airhouse_tenants row with SA fields populated. Returns the
/// tenant id and the plaintext bearer (so tests can assert the broker
/// decrypts it correctly).
pub(super) async fn seed_tenant_with_sa(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    tenant_id: &str,
) -> (String, String) {
    let bearer_plaintext = format!("ahsa_{}_secret", Uuid::new_v4().simple());
    let bearer_ciphertext =
        envelope::seal(bearer_plaintext.as_bytes()).expect("seal bearer (key set?)");

    airhouse_tenants::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        workspace_id: ActiveValue::Set(workspace_id),
        airhouse_tenant_id: ActiveValue::Set(tenant_id.to_string()),
        bucket: ActiveValue::Set("test-bucket".into()),
        prefix: ActiveValue::Set(Some(format!("tenants/{tenant_id}"))),
        status: ActiveValue::Set(TenantStatus::Active),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
        service_account_id: ActiveValue::Set(Some(format!("sa_{tenant_id}"))),
        bearer_ciphertext: ActiveValue::Set(Some(bearer_ciphertext)),
        bearer_max_role: ActiveValue::Set(Some("admin".to_string())),
        bearer_max_ttl_secs: ActiveValue::Set(Some(86400)),
        sa_created_at: ActiveValue::Set(Some(Utc::now().fixed_offset())),
        sa_rotated_at: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed tenant");

    (tenant_id.to_string(), bearer_plaintext)
}

/// Seed an airhouse_tenants row WITHOUT SA fields — the pre-Phase-2 shape.
async fn seed_tenant_without_sa(db: &DatabaseConnection, workspace_id: Uuid, tenant_id: &str) {
    airhouse_tenants::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        workspace_id: ActiveValue::Set(workspace_id),
        airhouse_tenant_id: ActiveValue::Set(tenant_id.to_string()),
        bucket: ActiveValue::Set("test-bucket".into()),
        prefix: ActiveValue::Set(Some(format!("tenants/{tenant_id}"))),
        status: ActiveValue::Set(TenantStatus::Active),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed tenant without SA");
}

pub(super) fn mint_response(tenant: &str, role: &str, ttl_secs: i64) -> Value {
    let expires_at = Utc::now() + chrono::Duration::seconds(ttl_secs);
    json!({
        "username": format!("eph_{}", Uuid::new_v4().simple().to_string().get(..12).unwrap_or("0000")),
        "password": format!("tk_{}", Uuid::new_v4().simple()),
        "tenant": tenant,
        "role": role,
        "expires_at": expires_at.to_rfc3339(),
        "service_account_id": format!("sa_{tenant}"),
    })
}

pub(super) fn make_broker(server: &MockServer) -> AirhouseTokenBroker {
    let client = AirhouseAdminClient::new(server.uri(), "tok");
    AirhouseTokenBroker::new(client)
}

#[tokio::test]
async fn mint_for_user_happy_path() {
    set_test_encryption_key();
    let db = test_db().await;
    let workspace_id = seed_workspace(&db, "broker-happy").await;
    seed_tenant_with_sa(&db, workspace_id, "broker-happy").await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/broker-happy/tokens"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let role = body["role"].as_str().unwrap();
            let ttl = body["ttl_secs"].as_i64().unwrap();
            ResponseTemplate::new(201).set_body_json(mint_response("broker-happy", role, ttl))
        })
        .expect(1)
        .mount(&server)
        .await;

    let broker = make_broker(&server);
    let user_id = Uuid::new_v4();
    let cred = broker
        .mint_for_user(
            workspace_id,
            user_id,
            UserRole::Reader,
            Duration::from_secs(900),
        )
        .await
        .expect("mint");
    assert!(cred.username.starts_with("eph_"));
    assert!(cred.password.starts_with("tk_"));
    assert_eq!(cred.tenant, "broker-happy");
    assert_eq!(cred.role, "reader");
}

#[tokio::test]
async fn mint_caches_within_ttl() {
    set_test_encryption_key();
    let db = test_db().await;
    let workspace_id = seed_workspace(&db, "broker-cache").await;
    seed_tenant_with_sa(&db, workspace_id, "broker-cache").await;

    let server = MockServer::start().await;
    // Long TTL so the cache stays fresh well past CACHE_REFRESH_BUFFER.
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/broker-cache/tokens"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let role = body["role"].as_str().unwrap();
            ResponseTemplate::new(201).set_body_json(mint_response("broker-cache", role, 3600))
        })
        // Exactly one mint despite two calls — the second hits the cache.
        .expect(1)
        .mount(&server)
        .await;

    let broker = make_broker(&server);
    let user_id = Uuid::new_v4();
    let first = broker
        .mint_for_user(
            workspace_id,
            user_id,
            UserRole::Writer,
            Duration::from_secs(3600),
        )
        .await
        .expect("first mint");
    let second = broker
        .mint_for_user(
            workspace_id,
            user_id,
            UserRole::Writer,
            Duration::from_secs(3600),
        )
        .await
        .expect("second mint should hit cache");
    assert_eq!(first.username, second.username);
    assert_eq!(first.password, second.password);
}

#[tokio::test]
async fn evict_and_remint_drops_cache_entry() {
    set_test_encryption_key();
    let db = test_db().await;
    let workspace_id = seed_workspace(&db, "broker-evict").await;
    seed_tenant_with_sa(&db, workspace_id, "broker-evict").await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/broker-evict/tokens"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let role = body["role"].as_str().unwrap();
            ResponseTemplate::new(201).set_body_json(mint_response("broker-evict", role, 3600))
        })
        // Exactly two mints — the first call caches, evict_and_remint forces a fresh one.
        .expect(2)
        .mount(&server)
        .await;

    let broker = make_broker(&server);
    let user_id = Uuid::new_v4();
    let first = broker
        .mint_for_user(
            workspace_id,
            user_id,
            UserRole::Reader,
            Duration::from_secs(3600),
        )
        .await
        .expect("first mint");
    let second = broker
        .evict_and_remint(
            workspace_id,
            BrokerSubject::User(user_id),
            UserRole::Reader,
            Duration::from_secs(3600),
        )
        .await
        .expect("evict_and_remint");
    assert_ne!(
        first.username, second.username,
        "evict_and_remint must produce a distinct credential"
    );
}

#[tokio::test]
async fn mint_returns_tenant_not_found_when_no_row() {
    set_test_encryption_key();
    let _db = test_db().await; // creates per-test DB but no airhouse_tenants row
    let workspace_id = Uuid::new_v4();

    let server = MockServer::start().await;
    let broker = make_broker(&server);
    let err = broker
        .mint_for_user(
            workspace_id,
            Uuid::new_v4(),
            UserRole::Reader,
            Duration::from_secs(900),
        )
        .await
        .expect_err("must fail when no tenant row");
    assert!(
        matches!(err, BrokerError::TenantNotFound(w) if w == workspace_id),
        "got {err:?}"
    );
}

#[tokio::test]
async fn mint_returns_tenant_has_no_sa_when_columns_null() {
    set_test_encryption_key();
    let db = test_db().await;
    let workspace_id = seed_workspace(&db, "broker-no-sa").await;
    seed_tenant_without_sa(&db, workspace_id, "broker-no-sa").await;

    let server = MockServer::start().await;
    let broker = make_broker(&server);
    let err = broker
        .mint_for_user(
            workspace_id,
            Uuid::new_v4(),
            UserRole::Reader,
            Duration::from_secs(900),
        )
        .await
        .expect_err("must fail when SA fields are null");
    assert!(
        matches!(err, BrokerError::TenantHasNoServiceAccount(w) if w == workspace_id),
        "got {err:?}"
    );
}

#[tokio::test]
async fn mint_retries_on_429_and_succeeds() {
    set_test_encryption_key();
    let db = test_db().await;
    let workspace_id = seed_workspace(&db, "broker-429").await;
    seed_tenant_with_sa(&db, workspace_id, "broker-429").await;

    let server = MockServer::start().await;

    // First call: 429.
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/broker-429/tokens"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limit exceeded"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    // Second call: succeeds. wiremock tries mocks in registration order, so
    // the first one (with up_to_n_times(1)) drains after one hit and the
    // second takes over.
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/broker-429/tokens"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let role = body["role"].as_str().unwrap();
            ResponseTemplate::new(201).set_body_json(mint_response("broker-429", role, 900))
        })
        .expect(1)
        .mount(&server)
        .await;

    let broker = make_broker(&server);
    let cred = broker
        .mint_for_user(
            workspace_id,
            Uuid::new_v4(),
            UserRole::Reader,
            Duration::from_secs(900),
        )
        .await
        .expect("mint must succeed after 429 retry");
    assert!(cred.password.starts_with("tk_"));
}

#[tokio::test]
async fn mint_for_system_uses_system_subject() {
    set_test_encryption_key();
    let db = test_db().await;
    let workspace_id = seed_workspace(&db, "broker-sys").await;
    seed_tenant_with_sa(&db, workspace_id, "broker-sys").await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants/broker-sys/tokens"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let subject = body["subject"].as_str().unwrap_or_default();
            assert!(
                subject.starts_with("system:workspace:"),
                "system mints must carry system: subject prefix, got {subject:?}"
            );
            assert!(
                subject.ends_with(":scheduler"),
                "purpose must be embedded in subject, got {subject:?}"
            );
            ResponseTemplate::new(201).set_body_json(mint_response("broker-sys", "admin", 900))
        })
        .expect(1)
        .mount(&server)
        .await;

    let broker = make_broker(&server);
    broker
        .mint_for_system(
            workspace_id,
            SystemPurpose::Scheduler,
            UserRole::Admin,
            Duration::from_secs(900),
        )
        .await
        .expect("system mint");
}
