//! Integration tests for `TenantProvisioner`.
//!
//! Spins up a Postgres testcontainer, applies the central migrator, and
//! drives `provision` / `deprovision` against a wiremock-backed Airhouse
//! admin client.
//!
//! Run with: `cargo nextest run -p oxy-app --test airhouse -E 'test(airhouse_provisioner)'`

use airhouse::entity::Tenants as AirhouseTenants;
use airhouse::entity::tenants::{self as airhouse_tenants, TenantStatus};
use airhouse::{AirhouseAdminClient, TenantProvisioner};
use base64::{Engine as _, engine::general_purpose};
use chrono::Utc;
use entity::organizations;
use entity::workspaces::{self, WorkspaceStatus};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    QueryFilter, TransactionTrait,
};
use serde_json::{Value, json};
use std::sync::Mutex;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Seed the AES-GCM master key so the SA-bearer envelope crypto round-trips
/// in-process. The provisioner now seals the SA bearer to `bearer_ciphertext`
/// on every successful provision; without a key in the env or state-dir
/// `oxy_platform::secrets::envelope::seal` would generate a random key and
/// write it to disk under the runner's `~/.local/share/oxy`, polluting the
/// dev machine.
fn set_test_encryption_key() {
    let _g = ENV_LOCK.lock().unwrap();
    // SAFETY: single-threaded test guarded by ENV_LOCK; deterministic key.
    unsafe {
        std::env::set_var(
            "OXY_ENCRYPTION_KEY",
            general_purpose::STANDARD.encode([7u8; 32]),
        );
    }
}

/// A fresh per-test database carrying the central + airhouse schema.
///
/// The migration chain runs once per `cargo nextest run` into a template that
/// this clones; see `tests/common/mod.rs`.
async fn test_db() -> DatabaseConnection {
    crate::common::fresh_db(crate::common::Schema::CentralAirhouse)
        .await
        .0
}

/// Create an org + workspace row and return the workspace id.
async fn seed_workspace(db: &DatabaseConnection, name: &str) -> Uuid {
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
        default_branch: ActiveValue::Set(None),
        repo_subdir: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed workspace");

    workspace_id
}

fn tenant_body(id: &str, prefix: &str) -> Value {
    json!({
        "id": id,
        "pg_url": "postgres://h/d",
        "bucket": "test-bucket",
        "prefix": prefix,
        "role": format!("airhouse_tenant_{id}"),
        "status": "active",
        "created_at": "2026-04-29T10:00:00Z",
    })
}

fn make_provisioner(db: DatabaseConnection, server: &MockServer) -> TenantProvisioner {
    let client = AirhouseAdminClient::new(server.uri(), "tok");
    TenantProvisioner::new(db, client)
}

fn sa_record_body(sa_id: &str, name: &str, tenant_id: &str) -> Value {
    json!({
        "id": sa_id,
        "name": name,
        "tenant_id": tenant_id,
        "max_role": "admin",
        "max_ttl_secs": 86400,
        "created_at": "2026-05-07T10:00:00Z",
        "revoked_at": null,
        "last_used_at": null,
    })
}

/// Mount the default SA endpoints used by `TenantProvisioner::provision` for
/// tests that don't care about the SA flow specifically. Returns nothing —
/// add per-test mocks before mounting these for finer control.
async fn mount_default_sa_mocks(server: &MockServer) {
    // No orphan SAs to adopt.
    Mock::given(method("GET"))
        .and(path("/admin/v1/service-accounts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(server)
        .await;
    // Mint a fresh SA on first call. Tenant id is echoed so the response
    // matches whatever tenant the provisioner just created.
    Mock::given(method("POST"))
        .and(path("/admin/v1/service-accounts"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let tenant_id = body["tenant_id"].as_str().unwrap_or_default().to_string();
            let name = body["name"].as_str().unwrap_or_default().to_string();
            let sa_id = format!(
                "sa_{}",
                Uuid::new_v4()
                    .simple()
                    .to_string()
                    .get(..16)
                    .unwrap_or("0000000000000000")
            );
            let mut record = sa_record_body(&sa_id, &name, &tenant_id);
            record.as_object_mut().unwrap().insert(
                "bearer".into(),
                json!(format!("ahsa_{}", Uuid::new_v4().simple())),
            );
            ResponseTemplate::new(201).set_body_json(record)
        })
        .mount(server)
        .await;
}

#[tokio::test]
async fn provision_fresh_creates_remote_and_local_row() {
    set_test_encryption_key();
    let db = test_db().await;
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let id = body["id"].as_str().unwrap().to_string();
            let prefix = format!("tenants/{id}");
            ResponseTemplate::new(201).set_body_json(tenant_body(&id, &prefix))
        })
        .mount(&server)
        .await;
    mount_default_sa_mocks(&server).await;

    let workspace_id = seed_workspace(&db, "acme").await;
    let prov = make_provisioner(db.clone(), &server);

    let rec = prov
        .provision(workspace_id, "acme".to_string())
        .await
        .expect("provision");
    assert_eq!(rec.bucket, "test-bucket");

    let local = AirhouseTenants::find()
        .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
        .one(&db)
        .await
        .unwrap()
        .expect("local row written");
    assert_eq!(local.status, TenantStatus::Active);
    assert_eq!(local.airhouse_tenant_id, rec.id);
    assert!(
        local.service_account_id.is_some(),
        "SA id should be populated"
    );
    assert!(
        local.bearer_ciphertext.is_some(),
        "bearer ciphertext should be populated"
    );
    assert_eq!(local.bearer_max_role.as_deref(), Some("admin"));
    assert_eq!(local.bearer_max_ttl_secs, Some(86400));
}

#[tokio::test]
async fn provision_is_idempotent_when_local_and_remote_exist() {
    set_test_encryption_key();
    let db = test_db().await;
    let server = MockServer::start().await;

    let create_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let create_calls_clone = create_calls.clone();
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants"))
        .respond_with(move |req: &wiremock::Request| {
            create_calls_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let id = body["id"].as_str().unwrap().to_string();
            ResponseTemplate::new(201).set_body_json(tenant_body(&id, "tenants/x"))
        })
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path_regex_admin_get())
        .respond_with(move |req: &wiremock::Request| {
            let id = req.url.path().rsplit('/').next().unwrap().to_string();
            ResponseTemplate::new(200).set_body_json(tenant_body(&id, "tenants/x"))
        })
        .mount(&server)
        .await;
    mount_default_sa_mocks(&server).await;

    let workspace_id = seed_workspace(&db, "idem").await;
    let prov = make_provisioner(db.clone(), &server);

    prov.provision(workspace_id, "idem".to_string())
        .await
        .unwrap();
    prov.provision(workspace_id, "idem".to_string())
        .await
        .unwrap();

    assert_eq!(create_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let count = AirhouseTenants::find()
        .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
        .all(&db)
        .await
        .unwrap()
        .len();
    assert_eq!(count, 1, "exactly one local row per workspace");
}

/// Two provisions of one workspace at once (workspace creation's background
/// provision and the demo, two uploads, a staff re-provision) must end with one
/// tenant row and one service account, and must not revoke the account the
/// other caller is still saving. `provision` holds a per-workspace advisory lock
/// (`TenantLock`); without it the second caller lists service accounts in the
/// first one's mint window, revokes that account as an orphan and mints its own.
/// The mock Airhouse remembers the accounts it minted and answers slowly, so
/// the two calls overlap.
#[tokio::test]
async fn concurrent_provisions_of_one_workspace_mint_one_service_account() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    set_test_encryption_key();
    let db = test_db().await;
    let server = MockServer::start().await;

    let tenant_creates = Arc::new(AtomicUsize::new(0));
    let creates = tenant_creates.clone();
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants"))
        .respond_with(move |req: &wiremock::Request| {
            creates.fetch_add(1, Ordering::SeqCst);
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let id = body["id"].as_str().unwrap().to_string();
            ResponseTemplate::new(201)
                .set_body_json(tenant_body(&id, "tenants/race"))
                .set_delay(Duration::from_millis(200))
        })
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex_admin_get())
        .respond_with(move |req: &wiremock::Request| {
            let id = req.url.path().rsplit('/').next().unwrap().to_string();
            ResponseTemplate::new(200).set_body_json(tenant_body(&id, "tenants/race"))
        })
        .mount(&server)
        .await;

    // Service accounts Airhouse has minted and not revoked: the list a second
    // caller would see, and revoke as orphans, in the first one's mint window.
    let minted: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let listed = minted.clone();
    Mock::given(method("GET"))
        .and(path("/admin/v1/service-accounts"))
        .respond_with(move |_: &wiremock::Request| {
            let accounts = listed.lock().unwrap().clone();
            ResponseTemplate::new(200).set_body_json(Value::Array(accounts))
        })
        .mount(&server)
        .await;
    let sa_creates = Arc::new(AtomicUsize::new(0));
    let (mints, saved) = (sa_creates.clone(), minted.clone());
    Mock::given(method("POST"))
        .and(path("/admin/v1/service-accounts"))
        .respond_with(move |req: &wiremock::Request| {
            let n = mints.fetch_add(1, Ordering::SeqCst);
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let tenant_id = body["tenant_id"].as_str().unwrap_or_default().to_string();
            let name = body["name"].as_str().unwrap_or_default().to_string();
            let record = sa_record_body(&format!("sa_race_{n}"), &name, &tenant_id);
            saved.lock().unwrap().push(record.clone());
            let mut with_bearer = record;
            with_bearer
                .as_object_mut()
                .unwrap()
                .insert("bearer".into(), json!(format!("ahsa_race_{n}")));
            ResponseTemplate::new(201)
                .set_body_json(with_bearer)
                .set_delay(Duration::from_millis(200))
        })
        .mount(&server)
        .await;
    let revokes = Arc::new(AtomicUsize::new(0));
    let revoked = revokes.clone();
    Mock::given(method("DELETE"))
        .and(wiremock::matchers::path_regex(
            r"^/admin/v1/service-accounts/[^/]+$",
        ))
        .respond_with(move |_: &wiremock::Request| {
            revoked.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(204)
        })
        .mount(&server)
        .await;

    let workspace_id = seed_workspace(&db, "race").await;
    let first = make_provisioner(db.clone(), &server);
    let second = make_provisioner(db.clone(), &server);
    let (a, b) = tokio::join!(
        first.provision(workspace_id, "race".to_string()),
        second.provision(workspace_id, "race".to_string()),
    );
    a.expect("first provision");
    b.expect("second provision");

    assert_eq!(
        tenant_creates.load(Ordering::SeqCst),
        1,
        "one tenant created"
    );
    assert_eq!(
        sa_creates.load(Ordering::SeqCst),
        1,
        "one service account minted"
    );
    assert_eq!(
        revokes.load(Ordering::SeqCst),
        0,
        "no service account revoked"
    );
    let rows = AirhouseTenants::find()
        .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "exactly one local row per workspace");
    assert_eq!(rows[0].service_account_id.as_deref(), Some("sa_race_0"));
    assert!(rows[0].bearer_ciphertext.is_some());
}

/// Provision against an already-taken tenant name must surface a typed
/// TenantNameTaken error rather than silently adopting the existing
/// remote tenant — that path could grant cross-workspace data access if
/// two users picked the same name. The local row stays at
/// status=failed so the operator can drive the recovery flow from the
/// runbook (delete the orphan, re-provision under the same name).
#[tokio::test]
async fn provision_returns_name_taken_error_on_409() {
    set_test_encryption_key();
    let db = test_db().await;
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants"))
        .respond_with(
            ResponseTemplate::new(409).set_body_string("{\"error\":\"already exists: taken\"}"),
        )
        .expect(1)
        .mount(&server)
        .await;

    // Note: no SA mocks registered. If the provisioner tried to adopt,
    // ensure_service_account_for_workspace would fire GET /service-accounts
    // and POST /service-accounts — wiremock would log an unmatched
    // request and the provisioner would fail with a different error.
    // Verifying we never get there confirms the early-rejection path.

    let workspace_id = seed_workspace(&db, "taken").await;
    let prov = make_provisioner(db.clone(), &server);

    let err = prov
        .provision(workspace_id, "taken".to_string())
        .await
        .expect_err("409 must reject, not silently adopt");
    assert!(
        matches!(&err, airhouse::ProvisionerError::TenantNameTaken(name) if name == "taken"),
        "got {err:?}"
    );

    // No local row is written on a name-collision 409. Writing a Failed
    // row pointing at the colliding tenant id would let a subsequent
    // provision call hit `reconcile_existing` first and silently adopt
    // the foreign tenant — the cross-workspace leak this branch is
    // designed to prevent.
    let local = AirhouseTenants::find()
        .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
        .one(&db)
        .await
        .unwrap();
    assert!(
        local.is_none(),
        "no local row should be written on a 409; got {local:?}"
    );
}

#[tokio::test]
async fn provision_recreates_when_remote_missing() {
    set_test_encryption_key();
    let db = test_db().await;
    let server = MockServer::start().await;

    // Pre-seed a local row pointing at a tenant that "used to exist".
    let workspace_id = seed_workspace(&db, "drift").await;
    let local_id = Uuid::new_v4();
    let stale_tenant_id = "drift-stale".to_string();
    airhouse_tenants::ActiveModel {
        id: ActiveValue::Set(local_id),
        workspace_id: ActiveValue::Set(workspace_id),
        airhouse_tenant_id: ActiveValue::Set(stale_tenant_id.clone()),
        bucket: ActiveValue::Set("test-bucket".into()),
        prefix: ActiveValue::Set(Some("tenants/drift-stale".into())),
        status: ActiveValue::Set(TenantStatus::Failed),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();

    Mock::given(method("GET"))
        .and(path(format!("/admin/v1/tenants/{stale_tenant_id}")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let id = body["id"].as_str().unwrap().to_string();
            ResponseTemplate::new(201).set_body_json(tenant_body(&id, "tenants/drift-stale"))
        })
        .mount(&server)
        .await;
    mount_default_sa_mocks(&server).await;

    let prov = make_provisioner(db.clone(), &server);
    // Re-provision: the tenant name is ignored since a local row already exists.
    prov.provision(workspace_id, "drift".to_string())
        .await
        .expect("provision recreates");

    let local = AirhouseTenants::find_by_id(local_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(local.status, TenantStatus::Active);
    assert_eq!(local.airhouse_tenant_id, stale_tenant_id);
}

#[tokio::test]
async fn deprovision_removes_local_and_calls_remote() {
    set_test_encryption_key();
    let db = test_db().await;
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let id = body["id"].as_str().unwrap().to_string();
            ResponseTemplate::new(201).set_body_json(tenant_body(&id, "tenants/x"))
        })
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex_admin_delete())
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    // Deprovision now also revokes the SA before deleting the tenant.
    Mock::given(method("DELETE"))
        .and(wiremock::matchers::path_regex(
            r"^/admin/v1/service-accounts/[^/]+$",
        ))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    mount_default_sa_mocks(&server).await;

    let workspace_id = seed_workspace(&db, "del").await;
    let prov = make_provisioner(db.clone(), &server);
    prov.provision(workspace_id, "del".to_string())
        .await
        .unwrap();
    prov.deprovision(workspace_id).await.unwrap();

    let count = AirhouseTenants::find()
        .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
        .all(&db)
        .await
        .unwrap()
        .len();
    assert_eq!(count, 0);
}

/// What a workspace delete does: read the tenant, then delete the workspace
/// and record the tenant in one transaction (the cascade takes the tenant row,
/// the record survives it). The sweep then removes the tenant and its service
/// account in Airhouse by what was recorded, and drops the record.
#[tokio::test]
async fn a_deleted_workspaces_tenant_is_removed_from_airhouse() {
    set_test_encryption_key();
    let db = test_db().await;
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let id = body["id"].as_str().unwrap().to_string();
            ResponseTemplate::new(201).set_body_json(tenant_body(&id, "tenants/x"))
        })
        .mount(&server)
        .await;
    mount_default_sa_mocks(&server).await;

    let workspace_id = seed_workspace(&db, "gone").await;
    let prov = make_provisioner(db.clone(), &server);
    prov.provision(workspace_id, "gone".to_string())
        .await
        .unwrap();
    let tenant = prov
        .detached_tenant(workspace_id)
        .await
        .unwrap()
        .expect("the workspace has a tenant");
    let sa_id = tenant
        .service_account_id
        .clone()
        .expect("provision minted a service account");

    let txn = db.begin().await.unwrap();
    workspaces::Entity::delete_by_id(workspace_id)
        .exec(&txn)
        .await
        .unwrap();
    airhouse::workspace_delete::record(&txn, std::slice::from_ref(&tenant))
        .await
        .unwrap();
    txn.commit().await.unwrap();
    assert!(
        prov.detached_tenant(workspace_id).await.unwrap().is_none(),
        "the workspace delete cascaded the tenant row"
    );
    assert_eq!(
        pending_removals(&db).await,
        1,
        "the record survived the cascade"
    );

    Mock::given(method("DELETE"))
        .and(wiremock::matchers::path(format!(
            "/admin/v1/service-accounts/{sa_id}"
        )))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(wiremock::matchers::path(format!(
            "/admin/v1/tenants/{}",
            tenant.airhouse_tenant_id
        )))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    assert_eq!(airhouse::workspace_delete::sweep_with(&db, &prov).await, 1);
    server.verify().await;
    assert_eq!(
        pending_removals(&db).await,
        0,
        "a removed tenant's record goes"
    );
    assert_eq!(
        airhouse::workspace_delete::sweep_with(&db, &prov).await,
        0,
        "nothing is left to sweep"
    );
}

/// A removal Airhouse refuses stays recorded, with the error, and is not due
/// again until its backoff has passed.
#[tokio::test]
async fn a_failed_removal_stays_recorded_and_backs_off() {
    let db = test_db().await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/admin/v1/service-accounts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/admin/v1/tenants/oxy-ws-refused"))
        .respond_with(ResponseTemplate::new(500).set_body_string("down"))
        .expect(1)
        .mount(&server)
        .await;

    let tenant = airhouse::DetachedTenant {
        workspace_id: Uuid::new_v4(),
        airhouse_tenant_id: "oxy-ws-refused".to_string(),
        service_account_id: None,
    };
    airhouse::workspace_delete::record(&db, std::slice::from_ref(&tenant))
        .await
        .unwrap();
    let prov = make_provisioner(db.clone(), &server);

    assert_eq!(airhouse::workspace_delete::sweep_with(&db, &prov).await, 0);
    assert_eq!(
        airhouse::workspace_delete::sweep_with(&db, &prov).await,
        0,
        "not due again before its backoff"
    );
    server.verify().await;

    let row = db
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT attempts, last_error, next_attempt_at > now() AS later \
             FROM airhouse_tenant_deletions WHERE airhouse_tenant_id = 'oxy-ws-refused'",
        ))
        .await
        .unwrap()
        .expect("the failed removal is still recorded");
    assert_eq!(row.try_get::<i32>("", "attempts").unwrap(), 1);
    assert!(row.try_get::<bool>("", "later").unwrap());
    assert!(
        row.try_get::<Option<String>>("", "last_error")
            .unwrap()
            .is_some()
    );
}

async fn pending_removals(db: &DatabaseConnection) -> i64 {
    db.query_one_raw(sea_orm::Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT count(*) AS n FROM airhouse_tenant_deletions",
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "n")
    .unwrap()
}

/// A workspace deleted while its provision was still running: the tenant was
/// read before the service account existed. The account minted since is found
/// by its tenant in Airhouse and revoked; another tenant's is left alone.
#[tokio::test]
async fn a_service_account_minted_after_the_tenant_was_read_is_revoked() {
    let db = test_db().await;
    let server = MockServer::start().await;
    let workspace_id = seed_workspace(&db, "race").await;
    let tenant = airhouse::DetachedTenant {
        workspace_id,
        airhouse_tenant_id: "oxy-ws-race".to_string(),
        service_account_id: None,
    };

    Mock::given(method("GET"))
        .and(path("/admin/v1/service-accounts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            sa_record_body("sa_minted_late", "oxy-tenant-oxy-ws-race", "oxy-ws-race"),
            sa_record_body("sa_other", "oxy-tenant-oxy-ws-other", "oxy-ws-other"),
        ])))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/admin/v1/service-accounts/sa_minted_late"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/admin/v1/tenants/oxy-ws-race"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    make_provisioner(db.clone(), &server)
        .delete_detached(&tenant)
        .await
        .unwrap();
    // `sa_other` has no mock: wiremock answers an unmatched call 404, and the
    // expectations above fail if anything else was revoked or deleted.
    server.verify().await;
    let received = server.received_requests().await.unwrap();
    assert!(
        !received.iter().any(|r| r.url.path().ends_with("/sa_other")),
        "another tenant's service account was revoked"
    );
}

#[tokio::test]
async fn deprovision_is_noop_when_local_row_absent() {
    let db = test_db().await;
    let server = MockServer::start().await;
    let workspace_id = seed_workspace(&db, "noop").await;

    let prov = make_provisioner(db.clone(), &server);
    prov.deprovision(workspace_id).await.expect("noop");
    // No mocks registered — wiremock would reject any request, so this also
    // verifies we never called Airhouse.
}

#[tokio::test]
async fn invalid_tenant_name_is_rejected_before_airhouse_call() {
    let db = test_db().await;
    let server = MockServer::start().await;
    let workspace_id = seed_workspace(&db, "badname").await;

    let prov = make_provisioner(db.clone(), &server);
    let err = prov
        .provision(workspace_id, "1-starts-with-digit".to_string())
        .await
        .expect_err("invalid name must be rejected");
    assert!(
        matches!(err, airhouse::ProvisionerError::InvalidTenantName(_)),
        "expected InvalidTenantName, got {err:?}"
    );
    // No mocks registered — verifies Airhouse was never called.
}

// `path("/admin/v1/tenants/{id}")` doesn't accept patterns; match by prefix-trimmed regex.
fn path_regex_admin_get() -> wiremock::matchers::PathRegexMatcher {
    wiremock::matchers::path_regex(r"^/admin/v1/tenants/[^/]+$")
}

fn path_regex_admin_delete() -> wiremock::matchers::PathRegexMatcher {
    wiremock::matchers::path_regex(r"^/admin/v1/tenants/[^/]+$")
}

// ── service-account-specific tests ──────────────────────────────────────────

/// Re-provisioning a tenant whose remote SA still exists with the
/// deterministic name (e.g. previous run crashed between SA mint and DB
/// persist) revokes the orphan and mints a fresh one. The bearer of the
/// orphan is unrecoverable, so adoption-by-reuse isn't an option.
#[tokio::test]
async fn provision_revokes_orphan_sa_and_remints() {
    set_test_encryption_key();
    let db = test_db().await;
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let id = body["id"].as_str().unwrap().to_string();
            ResponseTemplate::new(201).set_body_json(tenant_body(&id, "tenants/x"))
        })
        .mount(&server)
        .await;

    // List returns an orphan SA whose name matches our deterministic format.
    let orphan_sa_id = "sa_orphan_from_previous_run".to_string();
    let orphan_sa_id_for_list = orphan_sa_id.clone();
    Mock::given(method("GET"))
        .and(path("/admin/v1/service-accounts"))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(json!([{
                "id": orphan_sa_id_for_list,
                "name": "oxy-tenant-orphan",
                "tenant_id": "orphan",
                "max_role": "admin",
                "max_ttl_secs": 86400,
                "created_at": "2026-05-01T10:00:00Z",
                "revoked_at": null,
                "last_used_at": null,
            }]))
        })
        .expect(1)
        .mount(&server)
        .await;

    // Revocation of the orphan is observed via expect(1).
    let orphan_sa_path = format!("/admin/v1/service-accounts/{orphan_sa_id}");
    Mock::given(method("DELETE"))
        .and(path(orphan_sa_path))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    // Fresh mint after revocation.
    let new_sa_id = "sa_freshly_minted".to_string();
    let new_sa_id_for_resp = new_sa_id.clone();
    Mock::given(method("POST"))
        .and(path("/admin/v1/service-accounts"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let mut record = sa_record_body(
                &new_sa_id_for_resp,
                body["name"].as_str().unwrap(),
                "orphan",
            );
            record
                .as_object_mut()
                .unwrap()
                .insert("bearer".into(), json!("ahsa_freshbearer"));
            ResponseTemplate::new(201).set_body_json(record)
        })
        .expect(1)
        .mount(&server)
        .await;

    let workspace_id = seed_workspace(&db, "orphan").await;
    let prov = make_provisioner(db.clone(), &server);
    prov.provision(workspace_id, "orphan".to_string())
        .await
        .expect("provision");

    let local = AirhouseTenants::find()
        .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        local.service_account_id.as_deref(),
        Some(new_sa_id.as_str()),
        "must persist the freshly minted SA, not the orphan"
    );
}

/// A second `provision` call on an already-fully-provisioned tenant must NOT
/// hit the SA endpoints at all — the local row already has SA fields, so
/// the short-circuit in `ensure_service_account_for_workspace` fires before
/// any HTTP call.
#[tokio::test]
async fn provision_skips_sa_path_when_local_has_sa_fields() {
    set_test_encryption_key();
    let db = test_db().await;
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let id = body["id"].as_str().unwrap().to_string();
            ResponseTemplate::new(201).set_body_json(tenant_body(&id, "tenants/x"))
        })
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex_admin_get())
        .respond_with(move |req: &wiremock::Request| {
            let id = req.url.path().rsplit('/').next().unwrap().to_string();
            ResponseTemplate::new(200).set_body_json(tenant_body(&id, "tenants/x"))
        })
        .mount(&server)
        .await;

    // First provision: list+create SA.
    Mock::given(method("GET"))
        .and(path("/admin/v1/service-accounts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/admin/v1/service-accounts"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let mut record = sa_record_body(
                "sa_first_mint",
                body["name"].as_str().unwrap(),
                body["tenant_id"].as_str().unwrap(),
            );
            record
                .as_object_mut()
                .unwrap()
                .insert("bearer".into(), json!("ahsa_x"));
            ResponseTemplate::new(201).set_body_json(record)
        })
        .expect(1) // exactly one mint; second provision must short-circuit.
        .mount(&server)
        .await;

    let workspace_id = seed_workspace(&db, "skip").await;
    let prov = make_provisioner(db.clone(), &server);
    prov.provision(workspace_id, "skip".to_string())
        .await
        .unwrap();
    prov.provision(workspace_id, "skip".to_string())
        .await
        .unwrap();
    // Drop here to trigger wiremock's expect(1) verification on each mock.
}

/// `rotate_service_account` revokes the old SA airhouse-side, mints a new
/// one under the same deterministic name, and atomically swaps the
/// `service_account_id` + `bearer_ciphertext` + `sa_rotated_at` columns
/// on the local row. Outstanding airhouse-side ephemerals are not touched.
#[tokio::test]
async fn rotate_service_account_swaps_id_and_bearer() {
    set_test_encryption_key();
    let db = test_db().await;
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let id = body["id"].as_str().unwrap().to_string();
            ResponseTemplate::new(201).set_body_json(tenant_body(&id, "tenants/rot"))
        })
        .mount(&server)
        .await;
    mount_default_sa_mocks(&server).await;

    let workspace_id = seed_workspace(&db, "rot").await;
    let prov = make_provisioner(db.clone(), &server);
    prov.provision(workspace_id, "rot".to_string())
        .await
        .expect("initial provision");

    let before = AirhouseTenants::find()
        .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let old_sa_id = before
        .service_account_id
        .clone()
        .expect("initial provision wrote SA id");
    let old_ciphertext = before
        .bearer_ciphertext
        .clone()
        .expect("initial provision wrote bearer");

    // Rotation: DELETE old, POST new. The default mocks above already
    // cover both methods on /admin/v1/service-accounts; no extra setup.
    Mock::given(method("DELETE"))
        .and(wiremock::matchers::path_regex(
            r"^/admin/v1/service-accounts/[^/]+$",
        ))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let rotated = prov
        .rotate_service_account(workspace_id)
        .await
        .expect("rotate");
    assert_eq!(rotated.workspace_id, workspace_id);
    assert_eq!(rotated.old_sa_id, old_sa_id);
    assert_ne!(rotated.new_sa_id, old_sa_id, "must mint a distinct SA id");

    let after = AirhouseTenants::find()
        .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.service_account_id.as_deref(),
        Some(rotated.new_sa_id.as_str())
    );
    assert!(
        after.bearer_ciphertext.is_some(),
        "bearer ciphertext must remain populated"
    );
    assert_ne!(
        after.bearer_ciphertext.as_deref(),
        Some(old_ciphertext.as_slice()),
        "bearer ciphertext must change on rotation"
    );
    assert!(
        after.sa_rotated_at.is_some(),
        "sa_rotated_at must be stamped"
    );
}

/// Rotating a tenant that has no SA fields (pre-Phase-2 row) returns a
/// typed error — the caller should provision first, not rotate something
/// that doesn't exist.
#[tokio::test]
async fn rotate_returns_error_when_tenant_has_no_sa() {
    set_test_encryption_key();
    let db = test_db().await;
    let server = MockServer::start().await;
    let workspace_id = seed_workspace(&db, "norotate").await;

    // Pre-seed a tenant row without SA fields.
    airhouse_tenants::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        workspace_id: ActiveValue::Set(workspace_id),
        airhouse_tenant_id: ActiveValue::Set("norotate".to_string()),
        bucket: ActiveValue::Set("test-bucket".into()),
        prefix: ActiveValue::Set(Some("tenants/norotate".into())),
        status: ActiveValue::Set(TenantStatus::Active),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();

    let prov = make_provisioner(db.clone(), &server);
    let err = prov
        .rotate_service_account(workspace_id)
        .await
        .expect_err("must reject rotation when SA fields are NULL");
    assert!(
        matches!(err, airhouse::ProvisionerError::TenantHasNoServiceAccount(w) if w == workspace_id),
        "got {err:?}"
    );
    // No SA mocks were registered — verifies Airhouse was never called.
}
