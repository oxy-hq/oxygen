//! Which compiled revision a request may be pinned to by id: a `ready`
//! `staging`/`main` revision of the request's own workspace
//! ([`check_revision`], the rule a custom-app publish applies too) whose
//! compiled config the runtime can load ([`check_pinnable`], for a workspace
//! preview header).
//!
//! Here, beside [`crate::compiled_reader`], rather than in
//! `oxy_custom_apps::staging_pin`, because two domains apply the rule: a
//! custom app's publish and a workspace preview's header (`oxy-previews`),
//! and neither may depend on the other (`internal-docs/crate-tiers.md`,
//! stage A). `oxy_custom_apps::staging_pin` re-exports it under its old names.
//! It picks a revision; it reads no artifact.

use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

/// Whether `revision_id` may be pinned for a request on `workspace_id`: a
/// `ready` `staging`/`main` revision of that workspace (the publish-time check)
/// whose compiled config the runtime can load. For a caller that pins by id per
/// request — a workspace preview header.
///
/// The config check matters more here than at publish: a request whose pinned
/// config will not load falls back to the last-known-good MAIN revision, so the
/// preview would answer with main's content under a preview label.
pub async fn check_pinnable(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    revision_id: Uuid,
) -> Result<entity::revisions::Model, PinRefusal> {
    let rev = entity::revisions::Entity::find_by_id(revision_id)
        .one(db)
        .await
        .map_err(|e| PinRefusal::Db(e.to_string()))?
        .ok_or(PinRefusal::NotFound(revision_id))?;
    check_revision(&rev, workspace_id)?;
    if !crate::compiled_reader::revision_is_servable(db, revision_id).await {
        return Err(PinRefusal::Unservable(revision_id));
    }
    Ok(rev)
}

/// Why a pin was refused: a publish's `semantic_revision_id`, or a workspace
/// preview header's revision. For a publish, every variant is a 400 or 422 the
/// publisher can act on.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PinRefusal {
    #[error(
        "a pinned semantic revision is staging-only: it cannot be combined with --promote. \
         Merge the branch so main compiles, then promote the build"
    )]
    WithPromote,
    #[error("semantic revision {0} not found")]
    NotFound(Uuid),
    #[error(
        "semantic revision {revision} belongs to workspace {actual}, not this app's workspace {expected}"
    )]
    OtherWorkspace {
        revision: Uuid,
        expected: Uuid,
        actual: Uuid,
    },
    #[error("semantic revision {revision} is {status}, not ready — wait for the compile to finish")]
    NotReady { revision: Uuid, status: String },
    #[error(
        "semantic revision {revision} has kind {kind}; only a staging or main revision can be pinned"
    )]
    WrongKind { revision: Uuid, kind: String },
    #[error("semantic revision {0}'s compiled config does not load on this server")]
    Unservable(Uuid),
    #[error("database error: {0}")]
    Db(String),
}

/// The pure half of the pin check: the revision belongs to `project_id` and is
/// a `ready` `staging`/`main` revision. A custom-app publish
/// (`oxy_custom_apps::staging_pin::validate_pin_for_publish`) applies it alone;
/// [`check_pinnable`] adds that the compiled config loads.
pub fn check_revision(rev: &entity::revisions::Model, project_id: Uuid) -> Result<(), PinRefusal> {
    if rev.workspace_id != project_id {
        return Err(PinRefusal::OtherWorkspace {
            revision: rev.revision_id,
            expected: project_id,
            actual: rev.workspace_id,
        });
    }
    if rev.kind != "staging" && rev.kind != "main" {
        return Err(PinRefusal::WrongKind {
            revision: rev.revision_id,
            kind: rev.kind.clone(),
        });
    }
    if rev.status != "ready" {
        return Err(PinRefusal::NotReady {
            revision: rev.revision_id,
            status: rev.status.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rev(workspace_id: Uuid, kind: &str, status: &str) -> entity::revisions::Model {
        let now = chrono::Utc::now().fixed_offset();
        entity::revisions::Model {
            revision_id: Uuid::new_v4(),
            workspace_id,
            git_sha: "abc".into(),
            branch: Some("feature".into()),
            schema_version: 1,
            status: status.into(),
            kind: kind.into(),
            owner_user_id: None,
            compiler_version: "x".into(),
            started_at: now,
            finished_at: Some(now),
            file_count_seen: 0,
            file_count_compiled: 0,
            file_count_failed: 0,
            error_summary: None,
        }
    }

    #[test]
    fn a_ready_staging_or_main_revision_of_the_workspace_is_pinnable() {
        let ws = Uuid::new_v4();
        assert_eq!(check_revision(&rev(ws, "staging", "ready"), ws), Ok(()));
        assert_eq!(check_revision(&rev(ws, "main", "ready"), ws), Ok(()));
    }

    #[test]
    fn other_workspace_draft_and_unready_are_refused() {
        let ws = Uuid::new_v4();
        assert!(matches!(
            check_revision(&rev(Uuid::new_v4(), "staging", "ready"), ws),
            Err(PinRefusal::OtherWorkspace { .. })
        ));
        assert!(matches!(
            check_revision(&rev(ws, "draft", "ready"), ws),
            Err(PinRefusal::WrongKind { .. })
        ));
        assert!(matches!(
            check_revision(&rev(ws, "staging", "compiling"), ws),
            Err(PinRefusal::NotReady { .. })
        ));
    }

    #[test]
    fn the_promote_refusal_names_the_way_forward() {
        let msg = PinRefusal::WithPromote.to_string();
        assert!(msg.contains("staging-only"), "{msg}");
        assert!(msg.contains("Merge the branch"), "{msg}");
    }
}
