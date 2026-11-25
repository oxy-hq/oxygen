//! Eval suites (`*.test.yml`) across the compile boundary, and the
//! older-revision rule `oxyc evals run`'s route depends on.
//!
//! A compile writes `eval_test_definitions` rows and stamps the revision with
//! `CURRENT_SCHEMA_VERSION` (3 and up). A revision compiled before test files
//! were a kind has no rows because nobody looked, so `ConfigManager` answers
//! `NotCompiled` for it on a node with no files: the route turns that into a
//! retryable `503 not_compiled` and a queued compile, never a 404 for a test
//! file that is sitting in the repository.
//!
//! Database-backed through [`common::fresh_db`]: own database per test, so it
//! runs under `db-per-test` with the rest of this binary.

use std::path::{Path, PathBuf};

use entity::workspaces::WorkspaceStatus;
use entity::{eval_test_definitions, organizations, workspaces};
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::config::{EvalTest, EvalTests, ReadOnly};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, QueryFilter, Statement,
};
use uuid::Uuid;

use crate::common::Schema;

const SUITE: &str = "tests/sales.agentic.test.yml";

async fn setup_db() -> DatabaseConnection {
    let (db, test_url) = crate::common::fresh_db(Schema::Central).await;
    // SAFETY: single-threaded test setup; nextest runs each test in its own
    // process, so pointing the process-wide connection here is safe.
    unsafe {
        std::env::set_var("OXY_DATABASE_URL", &test_url);
        std::env::remove_var("OXY_DATABASE_AUTH_MODE");
    }
    db
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn working_copy() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("workspace dir");
    let root = dir.path();
    write(root, "config.yml", "models: []\ndatabases: []\n");
    write(root, "tests/sales.agentic.yml", "name: sales\n");
    write(
        root,
        SUITE,
        "cases:\n  - prompt: What was revenue in 2025?\n    expected: About $6.7B.\n",
    );
    dir
}

async fn seed_workspace_at(db: &DatabaseConnection, root: &Path) -> Uuid {
    let now = chrono::Utc::now().fixed_offset();
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set("evals-org".into()),
        slug: ActiveValue::Set(format!("evals-{}", org_id.simple())),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed org");
    let ws_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(ws_id),
        name: ActiveValue::Set("evals-ws".into()),
        org_id: ActiveValue::Set(Some(org_id)),
        path: ActiveValue::Set(Some(root.to_string_lossy().into_owned())),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    ws_id
}

async fn compile(db: &DatabaseConnection, ws_id: Uuid, root: &Path) -> Uuid {
    let outcome = oxy_compile::compile_workspace(oxy_compile::CompileRequest {
        db,
        workspace_id: ws_id,
        workspace_path: root,
        git_sha: None,
        branch: Some("main".to_string()),
        compiler_version: oxy_compile::compiler_version(),
        promote: true,
        kind: oxy_compile::RevisionKind::Main,
        owner_user_id: None,
        config_gate: Some(oxy_compile_runtime::config_gate::runtime_config_gate()),
    })
    .await
    .expect("compile the workspace");
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    outcome.revision_id
}

/// The config manager a request pinned to `revision` gets, over `root`.
async fn manager(ws_id: Uuid, root: &Path, revision: Uuid) -> oxy::config::ConfigManager<ReadOnly> {
    WorkspaceBuilder::new(ws_id)
        .with_working_copy(root, Some(revision), oxy::config::OnMissing::Empty)
        .await
        .expect("workspace builder")
        .build()
        .await
        .expect("workspace manager")
        .config_manager
        .into_read_only()
}

fn nowhere() -> PathBuf {
    let absent = PathBuf::from(format!("/nonexistent-oxy-workspace/{}", Uuid::new_v4()));
    assert!(!absent.exists());
    absent
}

/// What the previous release's compiler wrote: schema version 2, no rows.
async fn make_revision_predate_eval_tests(db: &DatabaseConnection, revision_id: Uuid) {
    eval_test_definitions::Entity::delete_many()
        .filter(eval_test_definitions::Column::RevisionId.eq(revision_id))
        .exec(db)
        .await
        .expect("drop the suite rows");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE revisions SET schema_version = 2 WHERE revision_id = $1",
        [revision_id.into()],
    ))
    .await
    .expect("downgrade the revision");
}

/// A compile carries the suite, and a node with no files reads it back.
#[tokio::test]
async fn a_node_with_no_files_reads_the_suite_from_the_revision() {
    let db = setup_db().await;
    let files = working_copy();
    let ws_id = seed_workspace_at(&db, files.path()).await;
    let revision_id = compile(&db, ws_id, files.path()).await;

    let pod = manager(ws_id, &nowhere(), revision_id).await;
    match pod.list_eval_tests().await.expect("the boundary answers") {
        EvalTests::Read(tests) => {
            let paths: Vec<_> = tests.iter().map(|t| t.file_path.as_str()).collect();
            assert_eq!(paths, [SUITE]);
        }
        EvalTests::NotCompiled => panic!("a current revision is compiled"),
    }
    assert!(matches!(
        pod.eval_test(SUITE).await.unwrap(),
        EvalTest::Found(_)
    ));
    assert!(matches!(
        pod.eval_test("tests/other.test.yml").await.unwrap(),
        EvalTest::Missing
    ));
}

/// THE rule: an older revision on a node with no files is not compiled, not
/// "no such test file".
#[tokio::test]
async fn an_older_revision_on_a_node_with_no_files_is_not_compiled() {
    let db = setup_db().await;
    let files = working_copy();
    let ws_id = seed_workspace_at(&db, files.path()).await;
    let revision_id = compile(&db, ws_id, files.path()).await;
    make_revision_predate_eval_tests(&db, revision_id).await;

    let pod = manager(ws_id, &nowhere(), revision_id).await;
    assert_eq!(
        pod.list_eval_tests().await.expect("the boundary answers"),
        EvalTests::NotCompiled
    );
    assert_eq!(pod.eval_test(SUITE).await.unwrap(), EvalTest::NotCompiled);
}

/// The same older revision on the node that holds the files reads the files.
#[tokio::test]
async fn an_older_revision_on_a_node_with_the_files_reads_the_files() {
    let db = setup_db().await;
    let files = working_copy();
    let ws_id = seed_workspace_at(&db, files.path()).await;
    let revision_id = compile(&db, ws_id, files.path()).await;
    make_revision_predate_eval_tests(&db, revision_id).await;

    let owns_files = manager(ws_id, files.path(), revision_id).await;
    assert!(matches!(
        owns_files.eval_test(SUITE).await.unwrap(),
        EvalTest::Found(_)
    ));
}
