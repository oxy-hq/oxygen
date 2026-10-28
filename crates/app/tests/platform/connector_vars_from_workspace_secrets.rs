//! A warehouse's `*_var` connection fields resolve from the workspace's secrets
//! in oxydb on a process that holds no working copy — the serve and worker
//! fleet that has answered chat since chat routes stopped being ide-only — and
//! a `*_var` that names no secret is an error naming it, never a connection to
//! `localhost`.
//!
//! What this guards: a Poke House chat on a serve pod failed every semantic
//! query with "error sending request for url (http://localhost:8123/…)" until
//! `max iterations exceeded`. The workspace's ClickHouse database declares
//! `host_var: CLICKHOUSE_HOST` and that workspace had no secret of that name;
//! the translation fell back to `localhost` and `database = ''`, and the
//! missing secret was named nowhere a person would look. The first test pins
//! the half that already worked — the fleet does read the workspace's secrets —
//! so a change that loses it fails here rather than in a customer's chat.
//!
//! The manager here is built the way the request middleware and the queued-run
//! builders build it: pinned to a promoted revision whose compiled config
//! carries the database, over a directory that does not exist, with
//! `SecretsManager::from_database_with_env_fallback` for the workspace.
//!
//! Database-backed through [`common::fresh_db`], so it belongs to `db-per-test`.

use agentic_connector::ConnectorConfig;
use entity::{users, workspace_compiled_configs};
use oxy::adapters::secrets::SecretsManager;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::ReadOnly;
use oxy::service::secret_manager::{CreateSecretParams, SecretManagerService};
use oxy_app::agentic_wiring::project_ctx::try_database_to_connector_config;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait};
use serde_json::json;
use uuid::Uuid;

use super::airway_compile_boundary::seed_promoted_workspace;

/// Unique per test so a stray variable in the test process environment can
/// never be what resolves.
fn var(stem: &str) -> String {
    format!("S10_{stem}_{}", Uuid::new_v4().simple()).to_uppercase()
}

async fn setup_db() -> DatabaseConnection {
    let (db, test_url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    // SAFETY: single-threaded test setup before any other env access; nextest
    // gives each test its own process.
    unsafe {
        std::env::set_var("OXY_DATABASE_URL", &test_url);
        std::env::remove_var("OXY_DATABASE_AUTH_MODE");
        // A fixed key, so the secret written below decrypts on the way back
        // and nothing writes a key file into the test's home directory.
        std::env::set_var(
            "OXY_ENCRYPTION_KEY",
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=",
        );
    }
    db
}

async fn seed_user(db: &DatabaseConnection) -> Uuid {
    let user = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(user),
        email: ActiveValue::Set(Some(format!("owner-{user}@example.com"))),
        name: ActiveValue::Set("Owner".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        status: ActiveValue::Set(users::UserStatus::Active),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    user
}

/// Give the promoted revision a compiled config carrying `databases`, shaped
/// as `oxy-compile` writes it: `*_var` references kept, no literals.
async fn set_compiled_databases(
    db: &DatabaseConnection,
    revision_id: Uuid,
    databases: serde_json::Value,
) {
    let row = workspace_compiled_configs::Entity::find_by_id(revision_id)
        .one(db)
        .await
        .expect("load compiled config")
        .expect("seeded compiled config");
    let mut row: workspace_compiled_configs::ActiveModel = row.into();
    row.databases = ActiveValue::Set(databases);
    row.update(db).await.expect("update compiled config");
}

async fn put_secret(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    user: Uuid,
    name: &str,
    value: &str,
) {
    SecretManagerService::new(workspace_id)
        .create_secret(
            db,
            CreateSecretParams {
                name: name.to_string(),
                value: value.to_string(),
                description: None,
                created_by: user,
            },
        )
        .await
        .expect("create workspace secret");
}

/// The manager a serve or worker process builds: compiled revision, no files,
/// workspace secrets first and the environment second.
async fn fleet_manager(workspace_id: Uuid, revision_id: Uuid) -> WorkspaceManager<ReadOnly> {
    let missing = std::env::temp_dir().join(format!("no-working-copy-{}", Uuid::new_v4()));
    assert!(
        !missing.exists(),
        "precondition: this process holds no working copy"
    );
    WorkspaceBuilder::new(workspace_id)
        .without_working_copy(&missing, Some(revision_id), oxy::config::OnMissing::Empty)
        .await
        .expect("workspace builder")
        .with_secrets_manager(
            SecretsManager::from_database_with_env_fallback(SecretManagerService::new(
                workspace_id,
            ))
            .expect("secrets manager"),
        )
        .build()
        .await
        .expect("workspace manager")
}

fn clickhouse_db(host: &str, user: &str, password: &str, database: &str) -> serde_json::Value {
    json!([{
        "name": "clickhouse",
        "type": "clickhouse",
        "host_var": host,
        "user_var": user,
        "password_var": password,
        "database_var": database,
    }])
}

#[tokio::test]
async fn a_compiled_clickhouse_host_var_resolves_from_the_workspace_secret_on_the_fleet() {
    let db = setup_db().await;
    let (workspace_id, revision_id) = seed_promoted_workspace(&db).await;
    let user = seed_user(&db).await;
    let (host, ch_user, password, database) = (
        var("CH_HOST"),
        var("CH_USER"),
        var("CH_PASSWORD"),
        var("CH_DATABASE"),
    );
    set_compiled_databases(
        &db,
        revision_id,
        clickhouse_db(&host, &ch_user, &password, &database),
    )
    .await;
    put_secret(
        &db,
        workspace_id,
        user,
        &host,
        "https://ch.example.test:8443",
    )
    .await;
    put_secret(&db, workspace_id, user, &ch_user, "oxy").await;
    put_secret(&db, workspace_id, user, &password, "not-a-real-password").await;
    put_secret(&db, workspace_id, user, &database, "restaurant_analytics").await;

    let manager = fleet_manager(workspace_id, revision_id).await;
    assert_eq!(
        manager.config_manager.revision_id(),
        Some(revision_id),
        "precondition: the database comes from the compiled revision, not a directory"
    );
    let database_config = manager
        .config_manager
        .resolve_database("clickhouse")
        .expect("the compiled config carries the database");

    let translated = try_database_to_connector_config(&database_config, &manager)
        .await
        .expect("every var names a workspace secret")
        .expect("clickhouse is translated, not deferred");
    let ConnectorConfig::ClickHouse(ch) = translated else {
        panic!("expected a ClickHouse connector config");
    };
    assert_eq!(
        ch.url, "https://ch.example.test:8443",
        "host from the workspace secret"
    );
    assert_eq!(ch.user, "oxy");
    assert_eq!(ch.database, "restaurant_analytics");
    assert!(
        !ch.url.contains("localhost"),
        "the fleet must never dial a host nobody configured"
    );
}

#[tokio::test]
async fn a_compiled_clickhouse_host_var_with_no_secret_is_an_error_naming_it() {
    let db = setup_db().await;
    let (workspace_id, revision_id) = seed_promoted_workspace(&db).await;
    let user = seed_user(&db).await;
    let (host, ch_user, password, database) = (
        var("CH_HOST"),
        var("CH_USER"),
        var("CH_PASSWORD"),
        var("CH_DATABASE"),
    );
    set_compiled_databases(
        &db,
        revision_id,
        clickhouse_db(&host, &ch_user, &password, &database),
    )
    .await;
    // Every secret but the host: the one the Poke House workspace was missing.
    put_secret(&db, workspace_id, user, &ch_user, "oxy").await;
    put_secret(&db, workspace_id, user, &password, "not-a-real-password").await;
    put_secret(&db, workspace_id, user, &database, "restaurant_analytics").await;

    let manager = fleet_manager(workspace_id, revision_id).await;
    let database_config = manager
        .config_manager
        .resolve_database("clickhouse")
        .expect("the compiled config carries the database");

    let err = try_database_to_connector_config(&database_config, &manager)
        .await
        .expect_err("a host_var naming no secret must not become a connection");
    let message = err.to_string();
    assert!(
        message.contains(&host) && message.contains("host_var") && message.contains("clickhouse"),
        "the error names the database, the field and the secret: {message}"
    );
    assert!(
        !message.contains("localhost"),
        "no fallback host is mentioned, let alone dialled: {message}"
    );
}
