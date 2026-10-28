//! A compile for the pod fleet refuses a database credential that is a file in
//! the repository, and the previous revision keeps serving.
//!
//! `internal-docs/factory-retirement.md` Phase 3a: BigQuery `key_path` and
//! Snowflake `private_key_path` name a file that a serving pod does not have
//! (and that the compiler blanks from the compiled config). The process here
//! has the worker role, the one a compile worker runs as, with the refusal
//! switched on (`OXY_COMPILE_REFUSE_FILE_CREDENTIALS=1`); the single-process
//! role is covered by the unit tests in `compile_config_gate`.
//!
//! Database-backed through [`common::fresh_db`], so it belongs to `db-per-test`;
//! nextest gives each test its own process, which makes setting `OXY_ROLE` safe.

use entity::workspaces;
use oxy_app::server::compile_config_gate::runtime_config_gate;
use oxy_compile::{CompileRequest, Promotion, RevisionKind, compile_workspace, compiler_version};
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use super::airway_compile_boundary::seed_workspace_at;

async fn setup_db() -> DatabaseConnection {
    let (db, test_url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    // SAFETY: single-threaded test setup before any other env access; nextest
    // gives each test its own process.
    unsafe {
        std::env::set_var("OXY_DATABASE_URL", &test_url);
        std::env::remove_var("OXY_DATABASE_AUTH_MODE");
        std::env::set_var("OXY_ROLE", "worker");
        std::env::set_var("OXY_COMPILE_REFUSE_FILE_CREDENTIALS", "1");
    }
    oxy_app::server::role_manifest::init_process_role_from_env();
    db
}

async fn compile(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    root: &std::path::Path,
) -> oxy_compile::CompileOutcome {
    compile_workspace(CompileRequest {
        db,
        workspace_id,
        workspace_path: root,
        git_sha: None,
        branch: Some("main".to_string()),
        compiler_version: compiler_version(),
        promote: true,
        kind: RevisionKind::Main,
        owner_user_id: None,
        config_gate: Some(runtime_config_gate()),
    })
    .await
    .expect("the compile runs")
}

async fn promoted(db: &DatabaseConnection, workspace_id: Uuid) -> Option<Uuid> {
    workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await
        .expect("load workspace")
        .and_then(|w| w.current_revision_id)
}

#[tokio::test]
async fn a_key_file_fails_the_compile_and_the_previous_revision_keeps_serving() {
    let db = setup_db().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("config.yml");
    let workspace_id = seed_workspace_at(&db, dir.path()).await;

    // A secret-backed credential compiles and is promoted.
    std::fs::write(
        &config,
        "models: []\ndatabases:\n  - name: warehouse\n    type: bigquery\n    key_path_var: BQ_KEY\n    dataset: sales\n",
    )
    .expect("config");
    let good = compile(&db, workspace_id, dir.path()).await;
    assert_eq!(good.promotion, Promotion::Promoted, "{:?}", good.failures);
    assert_eq!(promoted(&db, workspace_id).await, Some(good.revision_id));

    // The same database authored with a key file in the repository does not.
    std::fs::write(
        &config,
        "models: []\ndatabases:\n  - name: warehouse\n    type: bigquery\n    key_path: service-account.json\n    dataset: sales\n",
    )
    .expect("config");
    let bad = compile(&db, workspace_id, dir.path()).await;
    assert_ne!(bad.promotion, Promotion::Promoted);
    let failure = bad
        .failures
        .iter()
        .find(|f| f.path.ends_with("config.yml"))
        .unwrap_or_else(|| panic!("config.yml is named: {:?}", bad.failures));
    assert!(
        failure.message.contains("`warehouse`") && failure.message.contains("cannot be served"),
        "the message names the database and says it cannot be served yet: {}",
        failure.message
    );
    assert_eq!(
        promoted(&db, workspace_id).await,
        Some(good.revision_id),
        "the revision that was serving still is"
    );

    // Snowflake's private key file is refused the same way.
    std::fs::write(
        &config,
        "models: []\ndatabases:\n  - name: wh\n    type: snowflake\n    account: acme\n    username: svc\n    warehouse: compute\n    database: analytics\n    private_key_path: keys/svc.p8\n",
    )
    .expect("config");
    let snowflake = compile(&db, workspace_id, dir.path()).await;
    assert_ne!(snowflake.promotion, Promotion::Promoted);
    assert!(
        snowflake
            .failures
            .iter()
            .any(|f| f.message.contains("private_key_path") && f.message.contains("password_var")),
        "{:?}",
        snowflake.failures
    );
    assert_eq!(promoted(&db, workspace_id).await, Some(good.revision_id));
}
