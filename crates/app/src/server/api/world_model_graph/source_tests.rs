//! The world-model module's source rule, asserted where it is declared.

/// `src` with `//` comments removed. The rule below is about code, and this
/// module's own docs name the very calls it forbids.
fn code(src: &str) -> String {
    src.lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every handler-side file of the module: comments stripped, test modules cut.
fn module_code() -> Vec<(&'static str, String)> {
    [
        ("handlers.rs", include_str!("handlers.rs")),
        ("query.rs", include_str!("query.rs")),
        ("source.rs", include_str!("source.rs")),
    ]
    .into_iter()
    .map(|(name, src)| {
        let src = src.split("\n#[cfg(test)]").next().unwrap_or_default();
        (name, code(src))
    })
    .collect()
}

/// These routes are `FleetOk`. A handler that scans the working copy compiles
/// fine and fails only on a replica — and with an empty model rather than an
/// error — so the shape is guarded here, as `metric_tree` guards its own
/// (`handlers_never_scan_the_working_copy`).
#[test]
fn nothing_here_reads_the_working_copy_directly() {
    for (file, code) in module_code() {
        assert!(!code.is_empty(), "{file} read back empty");
        for forbidden in [
            "semantics_scan_path",
            "working_copy_key",
            "WorkspaceManagerWorkingCopy",
        ] {
            assert!(
                !code.contains(forbidden),
                "world_model_graph/{file} uses `{forbidden}`: resolve the scan through \
                 `source::ModelSource` (compile boundary first) and key caches by what it read"
            );
        }
    }
}

/// The stripper: a forbidden call must be seen, a comment naming it must not.
#[test]
fn the_scan_sees_code_and_not_comments() {
    assert!(code("let p = cm.semantics_scan_path();").contains("semantics_scan_path"));
    assert!(!code("// never semantics_scan_path() here").contains("semantics_scan_path"));
    assert!(!code("let x = 1; // semantics_scan_path").contains("semantics_scan_path"));
}

// ── the display config: a read that failed is not "no config" ───────────────

use axum::extract::{Json, Path};
use axum::http::StatusCode;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use uuid::Uuid;

use crate::server::api::middlewares::workspace_context::{
    SemanticLayerCacheCtx, WorkspaceManagerReadOnly,
};
use crate::server::api::semantic::WorkspacePath;
// Renamed because `sentry_surface`'s gate-entry check matches calls by name,
// and the custom-app handler `projects::world_model::get_world_model` enters
// the gate; a bare call here reads to it as a cross-file call of that one.
use crate::server::api::world_model_graph::get_world_model as ide_world_model_handler;
use crate::server::router::workspace_cache::{new_semantic_engine_cache, new_semantic_layer_cache};

/// A working copy with one entity (`order`), plus whatever `at_root` adds.
fn workspace(at_root: impl FnOnce(&std::path::Path)) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n").expect("config");
    let semantics = dir.path().join("semantics");
    std::fs::create_dir_all(&semantics).expect("semantics dir");
    std::fs::write(
        semantics.join("orders.view.yml"),
        "name: orders\ntable: orders\nentities:\n- name: order\n  type: primary\n  key: id\n\
         dimensions:\n- name: id\n  type: number\n  expr: id\nmeasures:\n- name: n\n  type: count\n",
    )
    .expect("view");
    at_root(dir.path());
    dir
}

/// `GET /semantic/world-model` over `root`, on a node that owns the files.
async fn graph(root: &std::path::Path) -> Result<serde_json::Value, (StatusCode, String)> {
    let id = Uuid::new_v4();
    let manager = WorkspaceBuilder::new(id)
        .with_working_copy(root, None, oxy::config::OnMissing::Empty)
        .await
        .expect("builder")
        .build()
        .await
        .expect("manager")
        .into_read_only();
    let layer_cache = SemanticLayerCacheCtx {
        cache: new_semantic_layer_cache(),
        workspace_id: id,
        engine_cache: new_semantic_engine_cache(),
    };
    ide_world_model_handler(
        WorkspaceManagerReadOnly(manager),
        layer_cache,
        Path(WorkspacePath { workspace_id: id }),
    )
    .await
    .map(|Json(graph)| serde_json::to_value(graph).expect("graph serialises"))
    .map_err(|(status, Json(body))| (status, body.message))
}

/// `.world-model.yml` hides entities and renames them. A read of it that
/// FAILED must not be drawn as "the workspace configured nothing": the graph
/// would show every entity, the hidden ones included.
#[tokio::test]
async fn a_config_that_cannot_be_read_is_a_retryable_503_not_an_unfiltered_graph() {
    // A directory where the file should be: present, and unreadable as a file
    // whoever runs the test.
    let ws = workspace(|root| std::fs::create_dir(root.join(".world-model.yml")).expect("mkdir"));
    let (status, message) = graph(ws.path())
        .await
        .expect_err("an unreadable config is not an empty one");
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{message}");
}

/// The file being absent is a complete answer, and the common one.
#[tokio::test]
async fn a_workspace_with_no_config_file_draws_every_entity() {
    let ws = workspace(|_| {});
    let graph = graph(ws.path())
        .await
        .expect("no config is a complete answer");
    assert_eq!(graph["entities"][0]["id"], "order", "{graph}");
}

/// What is at stake in the case above: the config is an allowlist.
#[tokio::test]
async fn a_config_that_lists_no_entity_draws_none() {
    let ws = workspace(|root| {
        std::fs::write(root.join(".world-model.yml"), "entities: []\n").expect("config");
    });
    let graph = graph(ws.path()).await.expect("served");
    assert_eq!(
        graph["entities"].as_array().map(Vec::len),
        Some(0),
        "{graph}"
    );
}

/// A config that was read and is wrong stays the error it was.
#[tokio::test]
async fn a_config_that_does_not_parse_is_still_a_500() {
    let ws = workspace(|root| {
        std::fs::write(root.join(".world-model.yml"), "entities: 7\n").expect("config");
    });
    let (status, message) = graph(ws.path())
        .await
        .expect_err("a broken config is an error");
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{message}");
}
