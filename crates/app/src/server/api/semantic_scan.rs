//! Moved to `oxy::config::scan`.
//!
//! These readers ARE the compiled-vs-disk decision for the semantic scan, an
//! agent's `context:` root and `.monitor.yml`. That decision belongs to `ConfigManager` — living here made
//! it a free function every caller could reach around, and the four
//! `compiled_*` row readers it uses had to be public for it, which is what let
//! three other call sites write their own half of the choice.
//!
//! They are `pub(super)` in core now. This module re-exports the entry points
//! so the call sites read the same; there is no second door left.

pub use oxy::config::scan::{
    MaterialisedContext, MaterialisedMonitorConfig, ScanDir, SemanticEntity, SemanticTarget,
    materialise_agent_context, materialise_monitor_config, read_semantic_entity, scan_dir,
};

// ── `GET /semantic/objects` ────────────────────────────────────────────────

use axum::Json;
use axum::http::StatusCode;
use serde::Serialize;
use std::path::Path;

use crate::server::api::middlewares::workspace_context::WorkspaceManagerReadOnly;

/// One view or topic: its name (the file stem) and workspace-relative path,
/// which is what `/semantic/view/{b64}` and `/semantic/topic/{b64}` take.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SemanticObject {
    pub name: String,
    pub path: String,
}

/// The views and topics a workspace serves.
#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct SemanticObjects {
    pub views: Vec<SemanticObject>,
    pub topics: Vec<SemanticObject>,
}

/// `GET /{workspace_id}/semantic/objects` — FleetOk. The views and topics of the
/// served revision (or, in single-process mode, of the workspace directory):
/// the list the semantic explorer and the view and topic pickers read. It
/// replaced walking the working copy's file tree through `GET /files`.
pub async fn list_semantic_objects(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
) -> Result<Json<SemanticObjects>, (StatusCode, String)> {
    let config_manager = &workspace_manager.config_manager;
    let scan = scan_dir(config_manager).await.map_err(|e| {
        let status = if e.retryable() {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        (status, format!("could not read the semantic model: {e}"))
    })?;
    Ok(Json(match &scan {
        ScanDir::Compiled(files) => SemanticObjects {
            views: objects(files.views().iter().map(|f| f.file_path.as_str()), VIEW),
            topics: objects(files.topics().iter().map(|f| f.file_path.as_str()), TOPIC),
        },
        ScanDir::Real(dir) => {
            let root = config_manager
                .working_copy()
                .map(|w| w.root().to_path_buf())
                .unwrap_or_else(|| dir.clone());
            let mut paths = Vec::new();
            walk(dir, &root, &mut paths);
            paths.sort();
            SemanticObjects {
                views: objects(paths.iter().map(String::as_str), VIEW),
                topics: objects(paths.iter().map(String::as_str), TOPIC),
            }
        }
    }))
}

const VIEW: [&str; 2] = [".view.yml", ".view.yaml"];
const TOPIC: [&str; 2] = [".topic.yml", ".topic.yaml"];

/// The paths that end in one of `suffixes`, named by their stem.
fn objects<'a>(paths: impl Iterator<Item = &'a str>, suffixes: [&str; 2]) -> Vec<SemanticObject> {
    paths
        .filter_map(|path| {
            let file = path.rsplit('/').next().unwrap_or(path);
            let name = suffixes.iter().find_map(|s| file.strip_suffix(s))?;
            Some(SemanticObject {
                name: name.to_string(),
                path: path.to_string(),
            })
        })
        .collect()
}

/// How deep the directory walk goes; a semantic model is a few levels deep.
const MAX_DEPTH: usize = 16;

/// Every file under `dir`, as a `/`-separated path relative to `root`, skipping
/// what the compiler skips (`oxy_compile::walker::is_skipped`: dot-prefixed and
/// build-output directories at any depth) and files named `*.test.*`, so the
/// list names only what a compile would serve. Symlinks are never
/// followed (`DirEntry::file_type` does not follow them), so a link loop or a
/// link out of the tree is skipped, and the walk stops at [`MAX_DEPTH`].
fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
    walk_at(dir, root, out, 0);
}

fn walk_at(dir: &Path, root: &Path, out: &mut Vec<String>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if oxy_compile::walker::is_skipped(&relative) {
            continue;
        }
        if kind.is_dir() {
            walk_at(&path, root, out, depth + 1);
        } else if kind.is_file() && !entry.file_name().to_string_lossy().contains(".test.") {
            out.push(relative);
        }
    }
}

#[cfg(test)]
mod objects_tests {
    use super::*;

    #[test]
    fn views_and_topics_are_named_by_their_stem() {
        let paths = [
            "semantics/views/orders.view.yml",
            "semantics/topics/sales.topic.yaml",
            "agents/a.agentic.yml",
        ];
        assert_eq!(
            objects(paths.iter().copied(), VIEW),
            vec![SemanticObject {
                name: "orders".into(),
                path: "semantics/views/orders.view.yml".into()
            }]
        );
        assert_eq!(
            objects(paths.iter().copied(), TOPIC),
            vec![SemanticObject {
                name: "sales".into(),
                path: "semantics/topics/sales.topic.yaml".into()
            }]
        );
    }

    #[test]
    fn a_directory_walk_is_relative_to_the_root_and_skips_dot_entries() {
        let dir = tempfile::tempdir().unwrap();
        let views = dir.path().join("semantics/views");
        std::fs::create_dir_all(&views).unwrap();
        std::fs::write(views.join("a.view.yml"), "name: a").unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/b.view.yml"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("build/semantics")).unwrap();
        std::fs::write(dir.path().join("build/semantics/c.view.yml"), "").unwrap();
        std::fs::write(views.join("d.test.view.yml"), "").unwrap();
        let mut out = Vec::new();
        walk(dir.path(), dir.path(), &mut out);
        assert_eq!(out, vec!["semantics/views/a.view.yml".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_walk_follows_no_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let views = dir.path().join("semantics/views");
        std::fs::create_dir_all(&views).unwrap();
        std::fs::write(views.join("a.view.yml"), "name: a").unwrap();
        // A loop back to the root, and a link out of the tree.
        std::os::unix::fs::symlink(dir.path(), views.join("loop")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("b.view.yml"), "").unwrap();
        std::os::unix::fs::symlink(outside.path(), views.join("out")).unwrap();
        let mut out = Vec::new();
        walk(dir.path(), dir.path(), &mut out);
        assert_eq!(out, vec!["semantics/views/a.view.yml".to_string()]);
    }
}

// ── `GET /context-graph/sources` ───────────────────────────────────────────
//
// What the Context Graph page draws, read from the served revision. The page
// built its graph from the working copy through `GET /files` and one
// `GET /files/{path}` per artifact; those routes went with the IDE
// (factory-retirement.md, phase 5). This reads the compiled rows instead, so
// any replica answers. The edges are still worked out in the browser, from
// the definitions this returns. It lives beside `/semantic/objects` because
// both are compiled-revision reads for a Factory page (S8: no new api module).

use oxy::config::ContextGraphSources;

/// FleetOk. `503` (retry) when nothing is compiled yet or the boundary could
/// not be read; never an empty graph for a workspace that was not looked at.
pub async fn get_context_graph_sources(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
) -> Result<Json<ContextGraphSources>, (StatusCode, String)> {
    workspace_manager
        .config_manager
        .context_graph_sources()
        .await
        .map(Json)
        .map_err(|e| {
            tracing::warn!(
                workspace_id = %workspace_manager.workspace_id,
                error = %e,
                "context graph sources could not be read"
            );
            let status = if e.retryable() {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (
                status,
                format!("could not read the compiled workspace: {e}"),
            )
        })
}

// ── `GET /revision/files` and `GET /revision/files/content` ────────────────
//
// The Factory's read-only file browser: the served revision's files and one
// file's content. The IDE's tree read the working copy through `GET /files`
// and went with it (factory-retirement.md, phase 5); this reads the compiled
// rows, so any replica answers and a pinned preview shows its own revision.
// A YAML file's content is its stored definition re-serialised.

use axum::extract::Query;
use oxy::config::{ArtifactError, RevisionFile, RevisionFileEntry};

/// FleetOk. The served revision's files, sorted by path. `503` (retry) when
/// nothing is compiled yet; never an empty tree for a workspace not looked at.
pub async fn get_revision_files(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
) -> Result<Json<Vec<RevisionFileEntry>>, (StatusCode, String)> {
    workspace_manager
        .config_manager
        .revision_files()
        .await
        .map(Json)
        .map_err(|e| compiled_read_error(workspace_manager.workspace_id, e))
}

#[derive(serde::Deserialize)]
pub struct RevisionFileQuery {
    path: String,
}

/// FleetOk. One file of the served revision; `404` when it has none at `path`.
pub async fn get_revision_file(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Query(RevisionFileQuery { path }): Query<RevisionFileQuery>,
) -> Result<Json<RevisionFile>, (StatusCode, String)> {
    match workspace_manager.config_manager.revision_file(&path).await {
        Ok(Some(file)) => Ok(Json(file)),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            format!("the served revision has no file at {path}"),
        )),
        Err(e) => Err(compiled_read_error(workspace_manager.workspace_id, e)),
    }
}

/// FleetOk. The revision this request reads (a pinned preview's, when one is
/// pinned) and the commit it was compiled from, for the Factory's top bar.
/// `503` (retry) when nothing is compiled yet.
pub async fn get_served_revision(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
) -> Result<Json<oxy::config::ServedRevision>, (StatusCode, String)> {
    workspace_manager
        .config_manager
        .served_revision()
        .await
        .map(Json)
        .map_err(|e| compiled_read_error(workspace_manager.workspace_id, e))
}

/// `503` for a retryable miss (nothing compiled, the boundary unreachable),
/// `500` otherwise.
fn compiled_read_error(workspace_id: uuid::Uuid, e: ArtifactError) -> (StatusCode, String) {
    tracing::warn!(%workspace_id, error = %e, "the compiled workspace could not be read");
    let status = if e.retryable() {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (
        status,
        format!("could not read the compiled workspace: {e}"),
    )
}
