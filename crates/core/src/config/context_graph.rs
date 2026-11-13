//! The compiled artifacts the Context Graph page draws.
//!
//! The page used to build its graph in the browser from the working copy:
//! `GET /files` for the tree, then one `GET /files/{path}` per view, topic,
//! agent and automation. Those routes went with the IDE (factory-retirement.md,
//! phase 5), so the page reads the served revision instead — the same
//! `*_definitions` rows every other surface reads, keyed by `revision_id`.
//!
//! Compiled only. A manager with no revision (`Origin::Disk`: the CLI, or
//! single-process mode on a workspace nothing has compiled) answers
//! [`ArtifactError::NoSource`], which is retryable: compile, then reload.

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

use super::artifacts::{ArtifactError, CompiledArtifact};
use super::compiled;
use super::manager::{ConfigManager, DiskSlot, Origin};

/// One compiled artifact as the graph reads it: its name, its
/// workspace-relative path, and the parsed YAML body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GraphArtifact {
    pub name: String,
    pub path: String,
    pub definition: serde_json::Value,
}

/// Everything the Context Graph builds nodes from, read from one revision.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct ContextGraphSources {
    pub views: Vec<GraphArtifact>,
    pub topics: Vec<GraphArtifact>,
    pub automations: Vec<GraphArtifact>,
    /// Analytics agents (`.agentic.yml`), the only agent kind the compile
    /// walker records.
    pub agents: Vec<GraphArtifact>,
    /// Workspace-relative paths of the verified queries (`.sql`). The graph
    /// draws one only where an automation or an agent references it, so the
    /// SQL text is not sent.
    pub sql_queries: Vec<String>,
}

impl<S: DiskSlot> ConfigManager<S> {
    /// The served revision's views, topics, automations, analytics agents and
    /// verified-query paths, each list sorted by path.
    pub async fn context_graph_sources(&self) -> Result<ContextGraphSources, ArtifactError> {
        let Origin::Compiled { revision_id, .. } = self.origin() else {
            return Err(ArtifactError::NoSource);
        };
        let (views, topics, automations, agents, sql_queries) = futures::try_join!(
            compiled::list_semantic_views_at(revision_id),
            compiled::list_semantic_topics_at(revision_id),
            compiled::list_automation_artifacts_at(revision_id),
            list_agent_artifacts_at(revision_id),
            // Paths only, already sorted: the SQL bodies are never sent.
            compiled::list_verified_query_paths_at(revision_id),
        )?;
        Ok(ContextGraphSources {
            views: to_graph(views),
            topics: to_graph(topics),
            automations: to_graph(automations),
            agents: to_graph(agents),
            sql_queries,
        })
    }
}

/// Every analytics agent's row at a revision, body included. The listing
/// reader (`list_agents_at`) keeps only what a list needs.
async fn list_agent_artifacts_at(
    revision_id: Uuid,
) -> Result<Vec<CompiledArtifact>, ArtifactError> {
    Ok(entity::agent_definitions::Entity::find()
        .filter(entity::agent_definitions::Column::RevisionId.eq(revision_id))
        .all(&compiled::conn().await?)
        .await
        .map_err(|e| ArtifactError::Backend(e.to_string()))?
        .into_iter()
        .map(|m| CompiledArtifact {
            name: m.name,
            file_path: m.file_path,
            definition: m.definition,
            blob_key: None,
        })
        .collect())
}

/// Rows as graph artifacts, sorted by path so the page lays out the same graph
/// for the same revision.
fn to_graph(rows: Vec<CompiledArtifact>) -> Vec<GraphArtifact> {
    let mut out: Vec<GraphArtifact> = rows
        .into_iter()
        .map(|row| GraphArtifact {
            name: row.name,
            path: row.file_path,
            definition: row.definition,
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, path: &str) -> CompiledArtifact {
        CompiledArtifact {
            name: name.into(),
            file_path: path.into(),
            definition: serde_json::json!({ "name": name }),
            blob_key: Some("ignored".into()),
        }
    }

    #[test]
    fn rows_become_artifacts_sorted_by_path() {
        let out = to_graph(vec![row("b", "z/b.view.yml"), row("a", "a/a.view.yml")]);
        assert_eq!(
            out.iter().map(|a| a.path.as_str()).collect::<Vec<_>>(),
            vec!["a/a.view.yml", "z/b.view.yml"]
        );
        assert_eq!(out[0].definition, serde_json::json!({ "name": "a" }));
    }

    /// Nothing compiled: no source, and retryable rather than an empty graph,
    /// which the page would draw as "this workspace has nothing".
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
        let err = manager
            .context_graph_sources()
            .await
            .expect_err("no revision must not answer an empty graph");
        assert!(matches!(err, ArtifactError::NoSource), "{err:?}");
        assert!(err.retryable());
    }
}
