//! A manager pinned to a promoted revision never answers an automation or
//! airway pipeline read from the working copy.
//!
//! The pod in these tests HAS the files: the directory exists and carries a
//! `.airway.yml` and an `.automation.yml` that the revision does not. Before
//! `internal-docs/factory-retirement.md` Phase 4 deleted the fallback, a miss on
//! the revision fell through to those files, so one request returned different
//! YAML depending on which pod served it. `airway_compile_boundary` covers the
//! pod with no files; this is the pod with them.
//!
//! The counterpart is pinned too: a manager built over a directory with no
//! revision (the CLI, single-process mode, a draft branch in the IDE) still
//! reads it, because that is the one thing `Origin::Disk` means.
//!
//! Database-backed through [`common::fresh_db`], so it belongs to `db-per-test`.

use std::path::Path;

use agentic_automation::{WorkspaceContext, WorkspaceReadError};
use agentic_pipeline::pipeline_ref::{PipelineRefError, load_pipeline_yaml};
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy_app::agentic_wiring::OxyProjectContext;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use serde_json::json;
use uuid::Uuid;

use super::airway_compile_boundary::{seed_pipeline, seed_promoted_workspace};

const COMPILED_PIPELINE: &str = "pipelines/compiled.airway.yml";
const DISK_ONLY_PIPELINE: &str = "pipelines/disk_only.airway.yml";
const COMPILED_AUTOMATION: &str = "automations/compiled.automation.yml";
const DISK_ONLY_AUTOMATION: &str = "automations/disk_only.automation.yml";

const PIPELINE_YAML: &str = "name: on_disk\n\
source:\n  kind: filesystem\n  config:\n    base_path: /tmp/x\n    pattern: '*.jsonl'\n    format: jsonl\n    table_name: orders\n\
destination:\n  database: warehouse\n  dataset_name: raw\n\
resources:\n  - orders\n";

const AUTOMATION_YAML: &str = "name: on_disk\ntasks: []\n";

async fn setup_db() -> DatabaseConnection {
    let (db, test_url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    // SAFETY: single-threaded test setup before any other env access; nextest
    // gives each test its own process.
    unsafe {
        std::env::set_var("OXY_DATABASE_URL", &test_url);
        std::env::remove_var("OXY_DATABASE_AUTH_MODE");
    }
    db
}

/// A working copy holding both the compiled files and one of each kind that no
/// revision has.
fn working_copy() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(root.join("config.yml"), "models: []\ndatabases: []\n").expect("config");
    for (path, body) in [
        (COMPILED_PIPELINE, PIPELINE_YAML),
        (DISK_ONLY_PIPELINE, PIPELINE_YAML),
        (COMPILED_AUTOMATION, AUTOMATION_YAML),
        (DISK_ONLY_AUTOMATION, AUTOMATION_YAML),
    ] {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().expect("parent")).expect("mkdir");
        std::fs::write(full, body).expect("write");
    }
    dir
}

async fn seed_automation(db: &DatabaseConnection, revision_id: Uuid, file_path: &str) {
    entity::automation_definitions::ActiveModel {
        revision_id: ActiveValue::Set(revision_id),
        name: ActiveValue::Set("compiled".into()),
        file_path: ActiveValue::Set(file_path.into()),
        extension: ActiveValue::Set("automation.yml".into()),
        definition: ActiveValue::Set(json!({ "name": "compiled", "tasks": [] })),
    }
    .insert(db)
    .await
    .expect("seed automation");
}

async fn context_over(
    root: &Path,
    workspace_id: Uuid,
    revision_id: Option<Uuid>,
) -> OxyProjectContext {
    let manager = WorkspaceBuilder::new(workspace_id)
        .with_working_copy(root, revision_id, oxy::config::OnMissing::Empty)
        .await
        .expect("workspace builder")
        .build()
        .await
        .expect("workspace manager");
    OxyProjectContext::new(manager)
}

#[tokio::test]
async fn a_pod_with_the_files_answers_a_revision_miss_from_the_revision() {
    let db = setup_db().await;
    let (workspace_id, revision_id) = seed_promoted_workspace(&db).await;
    seed_pipeline(&db, revision_id, "compiled", COMPILED_PIPELINE).await;
    seed_automation(&db, revision_id, COMPILED_AUTOMATION).await;
    let dir = working_copy();

    let ctx = context_over(dir.path(), workspace_id, Some(revision_id)).await;
    let manager = &ctx.workspace_manager().config_manager;
    assert_eq!(
        manager.revision_id(),
        Some(revision_id),
        "precondition: pinned to the promoted revision, not downgraded to the directory"
    );

    let pipelines: Vec<String> = manager
        .list_pipelines()
        .await
        .expect("list pipelines")
        .into_iter()
        .map(|p| p.file_path)
        .collect();
    assert_eq!(
        pipelines,
        vec![COMPILED_PIPELINE],
        "the pipeline that only exists on disk is not listed"
    );
    let automations: Vec<String> = manager
        .list_automations()
        .await
        .expect("list automations")
        .into_iter()
        .map(|a| a.file_path)
        .collect();
    assert_eq!(
        automations,
        vec![COMPILED_AUTOMATION],
        "the automation that only exists on disk is not listed"
    );

    assert!(
        manager
            .pipeline_definition(DISK_ONLY_PIPELINE)
            .await
            .expect("lookup")
            .is_none()
    );
    assert!(
        manager
            .automation_definition(DISK_ONLY_AUTOMATION)
            .await
            .expect("lookup")
            .is_none()
    );

    let err = load_pipeline_yaml(&ctx, DISK_ONLY_PIPELINE)
        .await
        .expect_err("a pipeline the revision lacks is not read from the directory");
    assert!(
        matches!(err, PipelineRefError::NotInRevision(_)),
        "got {err:?}"
    );

    let err = ctx
        .resolve_automation_yaml(DISK_ONLY_AUTOMATION)
        .await
        .expect_err("an automation the revision lacks is not read from the directory");
    assert!(matches!(err, WorkspaceReadError::Missing(_)), "got {err:?}");

    // What the revision does carry is served from it.
    load_pipeline_yaml(&ctx, COMPILED_PIPELINE)
        .await
        .expect("a compiled pipeline resolves");
    ctx.resolve_automation_yaml(COMPILED_AUTOMATION)
        .await
        .expect("a compiled automation resolves");
}

#[tokio::test]
async fn a_manager_over_a_directory_with_no_revision_still_reads_it() {
    let db = setup_db().await;
    let (workspace_id, _revision_id) = seed_promoted_workspace(&db).await;
    let dir = working_copy();

    let ctx = context_over(dir.path(), workspace_id, None).await;
    assert_eq!(ctx.workspace_manager().config_manager.revision_id(), None);

    let yaml = load_pipeline_yaml(&ctx, DISK_ONLY_PIPELINE)
        .await
        .expect("single-process mode reads its directory");
    assert!(yaml.contains("name: on_disk"), "{yaml}");
    ctx.resolve_automation_yaml(DISK_ONLY_AUTOMATION)
        .await
        .expect("and so does an automation");
}
