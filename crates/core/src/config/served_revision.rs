//! Which revision a request is served from, for the Factory's top bar.
//!
//! The IDE's header showed the working copy's branch and let you switch it.
//! There is no working copy and no switching now: what the Factory shows is
//! the compiled revision the request reads (a pinned preview's, when one is
//! pinned), and the commit it was compiled from.

use sea_orm::EntityTrait;
use serde::Serialize;
use uuid::Uuid;

use super::artifacts::ArtifactError;
use super::compiled;
use super::manager::{ConfigManager, DiskSlot, Origin};

/// The served revision's git facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServedRevision {
    pub revision_id: Uuid,
    pub git_sha: String,
    /// The branch the commit was compiled from, when the compile recorded one.
    pub branch: Option<String>,
    /// `main` for the workspace's own compile, otherwise a preview or staging kind.
    pub kind: String,
    /// RFC 3339; absent while the revision is still compiling.
    pub compiled_at: Option<String>,
    pub file_count: i32,
}

impl<S: DiskSlot> ConfigManager<S> {
    /// The revision this manager reads. [`ArtifactError::NoSource`] (retryable)
    /// when it reads none: nothing compiled yet, or single-process mode's disk.
    pub async fn served_revision(&self) -> Result<ServedRevision, ArtifactError> {
        let Origin::Compiled { revision_id, .. } = self.origin() else {
            return Err(ArtifactError::NoSource);
        };
        let row = entity::revisions::Entity::find_by_id(revision_id)
            .one(&compiled::conn().await?)
            .await
            .map_err(|e| ArtifactError::Backend(e.to_string()))?
            .ok_or(ArtifactError::NoSource)?;
        Ok(from_row(row))
    }
}

fn from_row(row: entity::revisions::Model) -> ServedRevision {
    ServedRevision {
        revision_id: row.revision_id,
        git_sha: row.git_sha,
        branch: row.branch.filter(|b| !b.is_empty()),
        kind: row.kind,
        compiled_at: row.finished_at.map(|t| t.to_rfc3339()),
        file_count: row.file_count_compiled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> entity::revisions::Model {
        let at = chrono::DateTime::parse_from_rfc3339("2026-11-13T07:00:00+00:00").unwrap();
        entity::revisions::Model {
            revision_id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            git_sha: "0123456789abcdef".into(),
            branch: Some(String::new()),
            schema_version: 1,
            status: "ready".into(),
            kind: "main".into(),
            owner_user_id: None,
            compiler_version: "test".into(),
            started_at: at,
            finished_at: Some(at),
            file_count_seen: 3,
            file_count_compiled: 2,
            file_count_failed: 1,
            error_summary: None,
        }
    }

    #[test]
    fn a_row_reads_as_its_git_facts() {
        let served = from_row(row());
        assert_eq!(served.git_sha, "0123456789abcdef");
        assert_eq!(served.branch, None, "an empty branch is no branch");
        assert_eq!(
            served.compiled_at.as_deref(),
            Some("2026-11-13T07:00:00+00:00")
        );
        assert_eq!(served.file_count, 2);
    }

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
        let err = manager.served_revision().await.expect_err("no revision");
        assert!(matches!(err, ArtifactError::NoSource), "{err:?}");
    }
}
