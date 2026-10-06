//! The workspace, database and host-adapter fixtures for
//! `context_documents_boundary`.

use std::path::{Path, PathBuf};

use entity::workspaces::WorkspaceStatus;
use entity::{context_document_definitions, organizations, workspaces};
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy_app::agentic_wiring::OxyProjectContext;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Statement,
};
use uuid::Uuid;

use crate::common::Schema;

pub(super) const GLOSSARY: &str = "# Glossary\n\n**GMV** is gross merchandise value.\n";
pub(super) const METRICS: &str = "# Metrics\n\nNet revenue excludes refunds.\n";
pub(super) const NOTES: &str = "# Notes kept beside the views\n";
pub(super) const CLOSE: &str = "# Month-end close\n";

pub(super) fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

/// The analyst's `context:`, as its `.agentic.yml` below declares it.
pub(super) fn analyst_patterns() -> Vec<String> {
    strings(&["./semantics/**/*", "./docs/*.md"])
}

/// What the analyst reads, in the order its patterns list them.
pub(super) fn analyst_documents() -> Vec<String> {
    strings(&[NOTES, GLOSSARY, METRICS])
}

pub(super) async fn setup_db(schema: Schema) -> DatabaseConnection {
    let (db, test_url) = crate::common::fresh_db(schema).await;
    // SAFETY: single-threaded test setup before any other env access. nextest
    // isolates each test in its own process, so pointing the process-wide
    // connection at the per-test database here is safe.
    unsafe {
        std::env::set_var("OXY_DATABASE_URL", &test_url);
        std::env::remove_var("OXY_DATABASE_AUTH_MODE");
    }
    db
}

pub(super) fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// Two agents with disjoint documents, a README nobody references, and one
/// markdown file that only a broad semantic glob reaches.
pub(super) fn working_copy() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("workspace dir");
    let root = dir.path();
    write(root, "config.yml", "models: []\ndatabases: []\n");
    write(
        root,
        "analyst.agentic.yml",
        "name: analyst\ncontext:\n  - ./semantics/**/*\n  - ./docs/*.md\n",
    );
    write(
        root,
        "finance/controller.agentic.yml",
        "name: controller\ncontext:\n  - ./finance/**/*.md\n",
    );
    write(root, "docs/glossary.md", GLOSSARY);
    write(root, "docs/metrics.md", METRICS);
    write(root, "semantics/notes.md", NOTES);
    write(root, "finance/close.md", CLOSE);
    write(root, "README.md", "# No agent's context reaches this\n");
    dir
}

pub(super) async fn seed_workspace_at(db: &DatabaseConnection, root: &Path) -> Uuid {
    let now = chrono::Utc::now().fixed_offset();
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set("docs-org".into()),
        slug: ActiveValue::Set(format!("docs-{}", org_id.simple())),
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
        name: ActiveValue::Set("docs-ws".into()),
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

/// Run the real compiler over `root` and promote the result.
pub(super) async fn compile(db: &DatabaseConnection, ws_id: Uuid, root: &Path) -> Uuid {
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
        config_gate: Some(oxy_app::server::compile_config_gate::runtime_config_gate()),
    })
    .await
    .expect("compile the workspace");
    assert!(
        outcome.failures.is_empty(),
        "fixture must compile clean: {:?}",
        outcome.failures
    );
    assert_eq!(outcome.promotion, oxy_compile::Promotion::Promoted);
    outcome.revision_id
}

/// The host adapter over a workspace root, pinned to `revision` the way the
/// request middleware pins it. `None` is `Origin::Disk`.
pub(super) async fn context(ws_id: Uuid, root: &Path, revision: Option<Uuid>) -> OxyProjectContext {
    let manager = WorkspaceBuilder::new(ws_id)
        .with_working_copy(root, revision, oxy::config::OnMissing::Empty)
        .await
        .expect("workspace builder")
        .build()
        .await
        .expect("workspace manager");
    OxyProjectContext::new(manager)
}

/// A root that is not on this node: what a serve or worker pod holds.
pub(super) fn nowhere() -> PathBuf {
    let absent = PathBuf::from(format!("/nonexistent-oxy-workspace/{}", Uuid::new_v4()));
    assert!(!absent.exists(), "precondition: no working copy on disk");
    absent
}

pub(super) async fn compiled_document_paths(
    db: &DatabaseConnection,
    revision_id: Uuid,
) -> Vec<String> {
    context_document_definitions::Entity::find()
        .filter(context_document_definitions::Column::RevisionId.eq(revision_id))
        .order_by_asc(context_document_definitions::Column::FilePath)
        .all(db)
        .await
        .expect("read compiled documents")
        .into_iter()
        .map(|row| row.file_path)
        .collect()
}

/// Rewrite a revision into what the previous release's compiler produced: the
/// older schema version and no document rows, compiled long enough ago that a
/// self-heal is not still cooling down.
pub(super) async fn make_revision_predate_documents(db: &DatabaseConnection, revision_id: Uuid) {
    context_document_definitions::Entity::delete_many()
        .filter(context_document_definitions::Column::RevisionId.eq(revision_id))
        .exec(db)
        .await
        .expect("drop the document rows");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE revisions SET schema_version = 1, \
         started_at = now() - interval '2 hours', \
         finished_at = now() - interval '2 hours' WHERE revision_id = $1",
        [revision_id.into()],
    ))
    .await
    .expect("age the revision");
}

/// How many compiles are in the queue. Each test owns its database, so every
/// one counted is this workspace's.
pub(super) async fn queued_compiles(db: &DatabaseConnection) -> i64 {
    db.query_one_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT count(*)::bigint AS n FROM agentic_runs WHERE source_type = 'compile'",
    ))
    .await
    .expect("count compile runs")
    .expect("a count row")
    .try_get::<i64>("", "n")
    .expect("n")
}
