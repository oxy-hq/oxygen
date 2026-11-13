//! The served revision's files, read-only, for the Factory's file browser.
//!
//! The IDE's file tree read the working copy (`GET /files`), and went with it
//! (factory-retirement.md, phase 5). The browser that replaces it shows what
//! the workspace *serves*: every artifact the compile recorded at the
//! revision, by its workspace-relative path. A YAML artifact's content is its
//! stored definition re-serialised to YAML, so comments and key order are not
//! the file's; SQL and context documents are stored as text and shown as
//! such. Files the compiler does not read (a README, scripts) are not here.
//!
//! Compiled only. A manager with no revision answers
//! [`ArtifactError::NoSource`], which is retryable: compile, then reload.

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use serde::Serialize;
use uuid::Uuid;

use super::artifacts::ArtifactError;
use super::compiled;
use super::manager::{ConfigManager, DiskSlot, Origin};

/// What a compiled file is. Snake case on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionFileKind {
    Config,
    Agent,
    View,
    Topic,
    Automation,
    Query,
    Migration,
    Pipeline,
    Simulation,
    ContextDocument,
    Monitor,
    Reconcile,
    WorldModel,
}

/// One file in the revision's listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RevisionFileEntry {
    pub path: String,
    pub kind: RevisionFileKind,
}

/// One file with its content as the browser shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RevisionFile {
    pub path: String,
    pub kind: RevisionFileKind,
    /// `yaml`, `sql` or `markdown`: what the viewer highlights it as.
    pub language: &'static str,
    pub content: String,
}

/// The root singletons and the one fixed path each compiles from
/// (`oxy_compile::walker`). `config.yml` is reassembled from its columns.
const CONFIG: &str = "config.yml";
const MONITOR: &str = ".monitor.yml";
const RECONCILE: &str = "reconcile.yml";
const WORLD_MODEL: &str = ".world-model.yml";

impl<S: DiskSlot> ConfigManager<S> {
    /// Every file the served revision records, sorted by path.
    pub async fn revision_files(&self) -> Result<Vec<RevisionFileEntry>, ArtifactError> {
        let revision_id = compiled_revision(self.origin())?;
        let mut out = list_paths_at(revision_id).await?;
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out.dedup_by(|a, b| a.path == b.path);
        Ok(out)
    }

    /// One file of the served revision. `Ok(None)` when the revision has no
    /// file at `path`.
    pub async fn revision_file(&self, path: &str) -> Result<Option<RevisionFile>, ArtifactError> {
        let revision_id = compiled_revision(self.origin())?;
        let path = path.trim_start_matches("./");
        if let Some((kind, body)) = singleton_at(revision_id, path).await? {
            return yaml_file(path, kind, &body).map(Some);
        }
        file_at(revision_id, path).await
    }
}

fn compiled_revision(origin: Origin) -> Result<Uuid, ArtifactError> {
    match origin {
        Origin::Compiled { revision_id, .. } => Ok(revision_id),
        _ => Err(ArtifactError::NoSource),
    }
}

fn backend(e: sea_orm::DbErr) -> ArtifactError {
    ArtifactError::Backend(e.to_string())
}

/// What a file's path says it should be highlighted as.
pub fn language_of(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".sql") {
        "sql"
    } else if lower.ends_with(".md") || lower.ends_with(".markdown") {
        "markdown"
    } else {
        "yaml"
    }
}

fn yaml_file(
    path: &str,
    kind: RevisionFileKind,
    body: &serde_json::Value,
) -> Result<RevisionFile, ArtifactError> {
    let content =
        serde_yaml::to_string(body).map_err(|e| ArtifactError::Backend(format!("yaml: {e}")))?;
    Ok(RevisionFile {
        path: path.to_string(),
        kind,
        language: "yaml",
        content,
    })
}

fn text_file(path: &str, kind: RevisionFileKind, content: String) -> RevisionFile {
    RevisionFile {
        path: path.to_string(),
        kind,
        language: language_of(path),
        content,
    }
}

/// The paths of one table's rows at a revision, without their bodies.
macro_rules! paths {
    ($db:expr, $rev:expr, $table:ident, $kind:expr) => {
        async {
            entity::$table::Entity::find()
                .select_only()
                .column(entity::$table::Column::FilePath)
                .filter(entity::$table::Column::RevisionId.eq($rev))
                .into_tuple::<String>()
                .all($db)
                .await
                .map_err(backend)
                .map(|paths| {
                    paths
                        .into_iter()
                        .map(|path| RevisionFileEntry { path, kind: $kind })
                        .collect::<Vec<_>>()
                })
        }
    };
}

/// Whether a one-row-per-revision table has a row at the revision.
macro_rules! present {
    ($db:expr, $rev:expr, $table:ident) => {
        async {
            entity::$table::Entity::find_by_id($rev)
                .select_only()
                .column(entity::$table::Column::RevisionId)
                .into_tuple::<Uuid>()
                .one($db)
                .await
                .map_err(backend)
                .map(|row| row.is_some())
        }
    };
}

async fn list_paths_at(rev: Uuid) -> Result<Vec<RevisionFileEntry>, ArtifactError> {
    use RevisionFileKind as K;
    let db = &compiled::conn().await?;
    let tables = futures::try_join!(
        paths!(db, rev, agent_definitions, K::Agent),
        paths!(db, rev, semantic_views, K::View),
        paths!(db, rev, semantic_topics, K::Topic),
        paths!(db, rev, automation_definitions, K::Automation),
        paths!(db, rev, verified_queries, K::Query),
        paths!(db, rev, schema_migration_definitions, K::Migration),
        paths!(db, rev, airway_pipelines, K::Pipeline),
        paths!(db, rev, simulation_definitions, K::Simulation),
        paths!(db, rev, context_document_definitions, K::ContextDocument),
    )?;
    let singletons = futures::try_join!(
        present!(db, rev, workspace_compiled_configs),
        present!(db, rev, monitor_configs),
        present!(db, rev, reconcile_configs),
        present!(db, rev, world_model_configs),
    )?;
    let mut out: Vec<RevisionFileEntry> = [
        tables.0, tables.1, tables.2, tables.3, tables.4, tables.5, tables.6, tables.7, tables.8,
    ]
    .into_iter()
    .flatten()
    .collect();
    for (present, path, kind) in [
        (singletons.0, CONFIG, K::Config),
        (singletons.1, MONITOR, K::Monitor),
        (singletons.2, RECONCILE, K::Reconcile),
        (singletons.3, WORLD_MODEL, K::WorldModel),
    ] {
        if present {
            out.push(RevisionFileEntry {
                path: path.to_string(),
                kind,
            });
        }
    }
    Ok(out)
}

/// A root singleton's body, when `path` names one and the revision has it.
async fn singleton_at(
    rev: Uuid,
    path: &str,
) -> Result<Option<(RevisionFileKind, serde_json::Value)>, ArtifactError> {
    use RevisionFileKind as K;
    let db = &compiled::conn().await?;
    let found = match path {
        CONFIG => return Ok(compiled::load_config_at(rev).await?.map(|v| (K::Config, v))),
        MONITOR => entity::monitor_configs::Entity::find_by_id(rev)
            .one(db)
            .await
            .map_err(backend)?
            .map(|m| (K::Monitor, m.definition)),
        RECONCILE => entity::reconcile_configs::Entity::find_by_id(rev)
            .one(db)
            .await
            .map_err(backend)?
            .map(|m| (K::Reconcile, m.definition)),
        WORLD_MODEL => entity::world_model_configs::Entity::find_by_id(rev)
            .one(db)
            .await
            .map_err(backend)?
            .map(|m| (K::WorldModel, m.definition)),
        _ => None,
    };
    Ok(found)
}

/// The row of one table at `(revision, path)`.
macro_rules! row {
    ($db:expr, $rev:expr, $path:expr, $table:ident) => {
        async {
            entity::$table::Entity::find()
                .filter(entity::$table::Column::RevisionId.eq($rev))
                .filter(entity::$table::Column::FilePath.eq($path))
                .one($db)
                .await
                .map_err(backend)
        }
    };
}

/// A path-keyed artifact's content. The tables are asked together; a path
/// lives in one of them.
async fn file_at(rev: Uuid, path: &str) -> Result<Option<RevisionFile>, ArtifactError> {
    use RevisionFileKind as K;
    let db = &compiled::conn().await?;
    let (agent, view, topic, automation, pipeline, simulation) = futures::try_join!(
        row!(db, rev, path, agent_definitions),
        row!(db, rev, path, semantic_views),
        row!(db, rev, path, semantic_topics),
        row!(db, rev, path, automation_definitions),
        row!(db, rev, path, airway_pipelines),
        row!(db, rev, path, simulation_definitions),
    )?;
    let yaml = [
        agent.map(|m| (K::Agent, m.definition)),
        view.map(|m| (K::View, m.definition)),
        topic.map(|m| (K::Topic, m.definition)),
        automation.map(|m| (K::Automation, m.definition)),
        pipeline.map(|m| (K::Pipeline, m.definition)),
        simulation.map(|m| (K::Simulation, m.definition)),
    ];
    if let Some((kind, body)) = yaml.into_iter().flatten().next() {
        return yaml_file(path, kind, &body).map(Some);
    }
    let (query, migration, document) = futures::try_join!(
        row!(db, rev, path, verified_queries),
        row!(db, rev, path, schema_migration_definitions),
        row!(db, rev, path, context_document_definitions),
    )?;
    let text = [
        query.map(|m| (K::Query, m.content)),
        migration.map(|m| (K::Migration, m.content)),
        document.map(|m| (K::ContextDocument, m.content)),
    ];
    Ok(text
        .into_iter()
        .flatten()
        .next()
        .map(|(kind, content)| text_file(path, kind, content)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_names_its_language() {
        assert_eq!(language_of("queries/top.sql"), "sql");
        assert_eq!(language_of("docs/README.MD"), "markdown");
        assert_eq!(language_of("semantics/views/a.view.yml"), "yaml");
        assert_eq!(language_of("config.yml"), "yaml");
    }

    #[test]
    fn a_definition_reads_back_as_yaml() {
        let body = serde_json::json!({ "name": "orders", "table": "orders" });
        let file = yaml_file(
            "semantics/views/orders.view.yml",
            RevisionFileKind::View,
            &body,
        )
        .expect("yaml");
        assert_eq!(file.language, "yaml");
        let back: serde_json::Value = serde_yaml::from_str(&file.content).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn kinds_are_snake_case_on_the_wire() {
        let entry = RevisionFileEntry {
            path: ".world-model.yml".into(),
            kind: RevisionFileKind::WorldModel,
        };
        assert_eq!(
            serde_json::to_value(entry).unwrap(),
            serde_json::json!({ "path": ".world-model.yml", "kind": "world_model" })
        );
    }

    /// Nothing compiled: no source, and retryable rather than an empty tree,
    /// which the page would draw as "this workspace has no files".
    #[tokio::test]
    async fn a_manager_with_no_revision_has_no_source() {
        use crate::config::{ConfigBuilder, OnMissing};
        let dir = tempfile::tempdir().unwrap();
        let manager = ConfigBuilder::new()
            .with_workspace_path(dir.path().join("never-cloned-here"))
            .unwrap()
            .build_without_working_copy(Origin::Disk, OnMissing::Empty)
            .await
            .unwrap();
        let err = manager.revision_files().await.expect_err("no revision");
        assert!(matches!(err, ArtifactError::NoSource), "{err:?}");
        assert!(err.retryable());
        let err = manager
            .revision_file("config.yml")
            .await
            .expect_err("no revision");
        assert!(matches!(err, ArtifactError::NoSource), "{err:?}");
    }
}
