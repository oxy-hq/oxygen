//! A pod with no working copy reads the compiled semantic model without
//! writing it anywhere.
//!
//! The compiled `.view.yml` / `.topic.yml` rows used to be written into a
//! temporary directory laid out the way airlayer's directory loader expects,
//! and the loader walked it (`internal-docs/factory-retirement.md` Phase 4,
//! "Semantic materialiser temp directory"). They are now held as YAML text and
//! parsed from memory by the same parser.
//!
//! The process's temporary directory is pointed at an empty directory first, so
//! "nothing was written" is a statement about the filesystem, not about a code
//! path. Database-backed through [`common::fresh_db`], so it belongs to
//! `db-per-test`; nextest gives each test its own process, which is what makes
//! changing `TMPDIR` here safe.

use entity::{semantic_topics, semantic_views};
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::config::scan::{SemanticEntity, SemanticTarget, read_semantic_entity, scan_dir};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use serde_json::json;
use uuid::Uuid;

use super::airway_compile_boundary::seed_promoted_workspace;

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

async fn seed_model(db: &DatabaseConnection, revision_id: Uuid) {
    for (name, file_path) in [
        ("orders", "semantics/views/orders.view.yml"),
        ("customers", "semantics/views/crm/customers.view.yml"),
    ] {
        semantic_views::ActiveModel {
            revision_id: ActiveValue::Set(revision_id),
            name: ActiveValue::Set(name.into()),
            file_path: ActiveValue::Set(file_path.into()),
            definition: ActiveValue::Set(json!({
                "name": name,
                "table": name,
                "entities": [],
                "dimensions": [{ "name": "id", "type": "number", "expr": "id" }],
            })),
            compiled_sql_blob_key: ActiveValue::Set(None),
        }
        .insert(db)
        .await
        .expect("seed view");
    }
    semantic_topics::ActiveModel {
        revision_id: ActiveValue::Set(revision_id),
        name: ActiveValue::Set("sales".into()),
        file_path: ActiveValue::Set("semantics/topics/sales.topic.yml".into()),
        definition: ActiveValue::Set(json!({
            "name": "sales",
            "views": ["orders"],
            "base_view": "orders",
        })),
        compiled_sql_blob_key: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed topic");
}

fn entries(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .expect("read tmp")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect()
}

#[tokio::test]
async fn a_compiled_model_is_read_and_parsed_without_writing_a_directory() {
    let db = setup_db().await;
    let (workspace_id, revision_id) = seed_promoted_workspace(&db).await;
    seed_model(&db, revision_id).await;

    // The pod: pinned to the revision, the working copy's directory absent.
    let absent = std::path::PathBuf::from(format!("/nonexistent-oxy-workspace/{}", Uuid::new_v4()));
    let manager = WorkspaceBuilder::new(workspace_id)
        .with_working_copy(&absent, Some(revision_id), oxy::config::OnMissing::Empty)
        .await
        .expect("workspace builder")
        .build()
        .await
        .expect("workspace manager");
    let config_manager = manager.config_manager.into_read_only();
    assert_eq!(config_manager.revision_id(), Some(revision_id));

    // Everything below happens with the temporary directory empty and
    // watched.
    let tmp = tempfile::tempdir().expect("tempdir");
    // SAFETY: see `setup_db`.
    unsafe { std::env::set_var("TMPDIR", tmp.path()) };
    assert_eq!(
        std::env::temp_dir(),
        tmp.path(),
        "precondition: TMPDIR took"
    );

    let scan = scan_dir(&config_manager).await.expect("the revision scans");
    assert!(scan.is_compiled());
    assert!(
        scan.directory().is_none(),
        "a compiled scan has no directory"
    );

    let layer = scan.load_layer().expect("the model loads from memory");
    let views: Vec<&str> = layer.views.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(
        views,
        ["customers", "orders"],
        "ordered by path below the kind, as a directory walk would"
    );
    assert_eq!(layer.topics.as_ref().map(Vec::len), Some(1));

    // The IDE's single-entity reads: the row's own text, not a path.
    let (_, target) = read_semantic_entity(
        &config_manager,
        SemanticEntity::View,
        "semantics/views/crm/customers.view.yml",
    )
    .await
    .expect("lookup")
    .expect("a compiled view is found by its workspace-relative path");
    let SemanticTarget::Yaml { file_path, yaml } = target else {
        panic!("a compiled view has no file");
    };
    assert_eq!(file_path, "semantics/views/crm/customers.view.yml");
    assert!(yaml.contains("customers"), "{yaml}");
    assert!(
        read_semantic_entity(
            &config_manager,
            SemanticEntity::View,
            "crm/customers.view.yml"
        )
        .await
        .expect("lookup")
        .is_none(),
        "the path below the kind is not the row's path"
    );

    let parsed = scan
        .parse_semantic_model()
        .expect("the validating parser reads the same text");
    assert_eq!(parsed.semantic_layer.views.len(), 2);

    assert!(
        entries(tmp.path()).is_empty(),
        "nothing was written to the temporary directory: {:?}",
        entries(tmp.path())
    );
}
