//! Markdown context documents, from a revision or from the working copy.
//!
//! [`ConfigManager::context_documents`](super::ConfigManager::context_documents)
//! owns the choice of source. This module holds the two things it chooses
//! between and the selection both share.
//!
//! Which files are documents, and which of them a set of `context:` patterns
//! reaches, is defined once in `oxy_compile::context_documents`. The compile
//! walker uses that definition to decide what a revision carries; the reads
//! here use the same one, so a document cannot resolve on one arm and be
//! missing on the other.

use oxy_compile::context_documents::{
    ContextPatterns, DocumentFile, SINCE_SCHEMA_VERSION, document_text,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::artifacts::{ArtifactError, ContextDocument};

fn backend(e: sea_orm::DbErr) -> ArtifactError {
    ArtifactError::Backend(e.to_string())
}

/// Every document a revision carries, or `None` when the revision was compiled
/// before documents were a compiled kind.
///
/// `None` is not "no documents", and that is the reason this returns an
/// `Option` rather than an empty list. A revision written by an older compiler
/// has no rows because nobody looked, so its agents' documents are unknown.
/// What a caller does about that depends on whether it has files to read
/// instead; this only refuses to call it an answer.
///
/// `None` means exactly that and nothing else. A revision that is no longer
/// there (retention took it mid-request) is an error, not an older revision:
/// from version 2 on, a read that cannot be made is never an empty answer.
pub(super) async fn documents_at(
    revision_id: Uuid,
) -> Result<Option<Vec<ContextDocument>>, ArtifactError> {
    let db = super::compiled::conn().await?;
    let revision = entity::revisions::Entity::find_by_id(revision_id)
        .one(&db)
        .await
        .map_err(backend)?
        .ok_or_else(|| {
            ArtifactError::Backend(format!(
                "revision {revision_id} is no longer in the compile boundary"
            ))
        })?;
    if revision.schema_version < SINCE_SCHEMA_VERSION {
        return Ok(None);
    }
    let rows = entity::context_document_definitions::Entity::find()
        .filter(entity::context_document_definitions::Column::RevisionId.eq(revision_id))
        .all(&db)
        .await
        .map_err(backend)?;
    Ok(Some(
        rows.into_iter()
            .map(|row| ContextDocument {
                file_path: row.file_path,
                content: row.content,
            })
            .collect(),
    ))
}

/// The documents `patterns` reach among a revision's, in the agent's order:
/// by the first pattern that matches, then by path.
///
/// A revision carries the union of what every agent reaches, so this is what
/// keeps one agent from being handed another's documents.
pub(super) fn select(
    patterns: &ContextPatterns,
    documents: Vec<ContextDocument>,
) -> Vec<ContextDocument> {
    let mut reached: Vec<(usize, ContextDocument)> = documents
        .into_iter()
        .filter_map(|document| {
            patterns
                .position(&document.file_path)
                .map(|position| (position, document))
        })
        .collect();
    reached.sort_by(|a, b| (a.0, &a.1.file_path).cmp(&(b.0, &b.1.file_path)));
    reached.into_iter().map(|(_, document)| document).collect()
}

/// The bodies of the files the working-copy walk found, in its order.
///
/// A file that was listed and then cannot be read is a fault on this node, not
/// a document the agent does not have, so it is an error and a retryable one.
/// Decoded by the function the compiler stores it through, so the two arms
/// hand a run the same text for the same bytes.
pub(super) async fn read(files: Vec<DocumentFile>) -> Result<Vec<ContextDocument>, ArtifactError> {
    let mut documents = Vec::with_capacity(files.len());
    for file in files {
        let bytes = tokio::fs::read(&file.abs_path).await.map_err(|e| {
            ArtifactError::WorkspaceUnavailable(format!(
                "`{}` exists but could not be read: {e}",
                file.rel_path
            ))
        })?;
        documents.push(ContextDocument {
            file_path: file.rel_path,
            content: document_text(&bytes),
        });
    }
    Ok(documents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigBuilder, ContextDocuments, OnMissing, Origin};

    fn write(root: &std::path::Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn patterns(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|p| p.to_string()).collect()
    }

    /// The working-copy arm: a node that reads files, on a workspace with no
    /// revision pinned (a draft branch, or not compiled yet).
    #[tokio::test]
    async fn a_disk_origin_reads_the_documents_the_patterns_reach() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "config.yml", "models: []\ndatabases: []\n");
        write(root, "docs/glossary.md", "# Glossary\n");
        write(root, "docs/draft.test.md", "# a fixture\n");
        write(root, "README.md", "# not referenced\n");
        write(root, "node_modules/pkg/README.md", "# vendored\n");

        let manager = ConfigBuilder::new()
            .with_workspace_path(root)
            .unwrap()
            .build_with_working_copy(Origin::Disk, OnMissing::Empty)
            .await
            .unwrap();

        let documents = manager
            .context_documents(&patterns(&["./docs/*.md"]))
            .await
            .unwrap()
            .read()
            .expect("the working copy answers");
        assert_eq!(
            documents,
            vec![ContextDocument {
                file_path: "docs/glossary.md".to_string(),
                content: "# Glossary\n".to_string(),
            }]
        );
        assert_eq!(
            manager
                .context_documents(&patterns(&["./**/*.md"]))
                .await
                .unwrap()
                .read()
                .expect("the working copy answers")
                .len(),
            2,
            "a broad glob adds the README and still prunes node_modules and the fixture"
        );
    }

    /// A file a revision could not carry verbatim (Postgres `TEXT` holds no
    /// NUL) reads here as exactly what the compiler would have stored, so an
    /// agent is not given different text depending on which node ran it.
    #[tokio::test]
    async fn the_disk_arm_decodes_a_document_the_way_the_compiler_stores_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let raw: &[u8] = b"# Odd\0 bytes \xff here\n";
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/odd.md"), raw).unwrap();

        let manager = ConfigBuilder::new()
            .with_workspace_path(root)
            .unwrap()
            .build_with_working_copy(Origin::Disk, OnMissing::Empty)
            .await
            .unwrap();
        let documents = manager
            .context_documents(&patterns(&["./docs/*.md"]))
            .await
            .unwrap()
            .read()
            .expect("the working copy answers");

        assert_eq!(documents[0].content, document_text(raw));
        assert_eq!(documents[0].content, "# Odd bytes \u{fffd} here\n");
    }

    /// The incident shape, for this kind: a workspace root that is not on this
    /// node must not read as an agent that has no documents.
    #[tokio::test]
    async fn an_absent_root_is_a_retryable_error_not_an_agent_without_documents() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("never-cloned-here");
        let manager = ConfigBuilder::new()
            .with_workspace_path(&absent)
            .unwrap()
            .build_with_working_copy(Origin::Disk, OnMissing::Empty)
            .await
            .unwrap();

        let err = manager
            .context_documents(&patterns(&["./docs/*.md"]))
            .await
            .expect_err("an absent root must not answer `[]`");
        assert!(
            matches!(err, ArtifactError::WorkspaceUnavailable(_)),
            "{err:?}"
        );
        assert!(err.retryable());
        assert!(!absent.exists(), "and asking must not create the root");
    }

    /// Nothing compiled and no files: there is no source, and that is
    /// retryable rather than an empty list.
    #[tokio::test]
    async fn nothing_compiled_and_no_working_copy_is_no_source() {
        let dir = tempfile::tempdir().unwrap();
        let manager = ConfigBuilder::new()
            .with_workspace_path(dir.path().join("never-cloned-here"))
            .unwrap()
            .build_without_working_copy(Origin::Disk, OnMissing::Empty)
            .await
            .unwrap();

        let err = manager
            .context_documents(&patterns(&["./docs/*.md"]))
            .await
            .expect_err("no source must not answer `[]`");
        assert!(matches!(err, ArtifactError::NoSource), "{err:?}");
        assert!(err.retryable());
    }

    /// The other half of the two tests above, so they cannot be satisfied by
    /// refusing everything: an agent whose patterns cannot name a `.md` has no
    /// documents on any node, and is not made to wait for a source to say so.
    #[tokio::test]
    async fn patterns_that_cannot_name_markdown_are_answered_without_a_source() {
        let dir = tempfile::tempdir().unwrap();
        let manager = ConfigBuilder::new()
            .with_workspace_path(dir.path().join("never-cloned-here"))
            .unwrap()
            .build_without_working_copy(Origin::Disk, OnMissing::Empty)
            .await
            .unwrap();

        for raw in [
            &[][..],
            &["./semantics/*.view.yml", "./example_sql/*.sql"][..],
        ] {
            assert_eq!(
                manager.context_documents(&patterns(raw)).await.unwrap(),
                ContextDocuments::Read(vec![]),
                "{raw:?}"
            );
        }
    }

    fn document(file_path: &str) -> ContextDocument {
        ContextDocument {
            file_path: file_path.to_string(),
            content: format!("# {file_path}"),
        }
    }

    fn paths(documents: &[ContextDocument]) -> Vec<&str> {
        documents.iter().map(|d| d.file_path.as_str()).collect()
    }

    /// A revision holds what EVERY agent reaches. One agent must get its own.
    #[test]
    fn an_agent_is_handed_only_the_documents_its_own_patterns_reach() {
        let revision = vec![
            document("docs/glossary.md"),
            document("finance/close.md"),
            document("README.md"),
        ];

        let analyst = ContextPatterns::new(["./docs/*.md"]);
        assert_eq!(
            paths(&select(&analyst, revision.clone())),
            ["docs/glossary.md"]
        );

        let controller = ContextPatterns::new(["./finance/**/*"]);
        assert_eq!(paths(&select(&controller, revision)), ["finance/close.md"]);
    }

    /// The order is the agent's: pattern by pattern, then by path. A document
    /// two patterns reach appears once, under the first.
    #[test]
    fn documents_come_back_in_the_order_the_agent_listed_its_patterns() {
        let revision = vec![
            document("a/first.md"),
            document("z/both.md"),
            document("z/only.md"),
        ];
        let patterns = ContextPatterns::new(["./z/*.md", "./**/*.md"]);

        assert_eq!(
            paths(&select(&patterns, revision)),
            ["z/both.md", "z/only.md", "a/first.md"]
        );
    }
}
