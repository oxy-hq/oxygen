//! A custom-app draft build's **semantic pin** — the compiled revision its
//! staging requests read (`internal-docs/customer-apps-staging.md` D4,
//! mechanics in `internal-docs/compile-boundary.md` § "Staging revisions").
//!
//! Four modules here, and two parts that live in other crates:
//!
//! * here — read a build's pin ([`pinned_revision_for`]) and validate one at
//!   publish ([`validate_pin_for_publish`], [`check_pinnable`]);
//! * [`environment`] — an environment's own pin
//!   ([`environment::environment_pin`]) and how it combines with a build's
//!   ([`effective_pin`]); a branch with no ready compile is refused;
//! * [`resolve`] — name the app a request is for and resolve the pin a
//!   non-production request for it reads ([`resolve::pin_for_environment`]);
//! * [`drift`] — the promote notice: which semantic views/topics differ between
//!   a promoted build's pin and the revision live actually reads;
//! * `oxy_app_core::staging_pin_scope` — run a future under a pin
//!   (`with_staging_pin`) and read it back;
//! * [`request`] — read the app and the environment out of a browser
//!   data-plane request's headers, then call [`resolve::pin_for_environment`].
//!
//! **The live channel ignores pins.** Nothing here is consulted for a request
//! that is not a staging request (the app's staging host, for a viewer who may
//! open it), so a promoted build always reads `workspaces.current_revision_id` and promote
//! stays a pointer move.

pub mod drift;
pub mod environment;
pub mod request;
pub mod resolve;

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, Statement};
use uuid::Uuid;

pub use drift::{SemanticPinNotice, pin_drift};
pub use environment::{EnvironmentPin, NotReadyCause, PinNotReady, effective_pin, environment_pin};

/// The semantic revision `build_id` (an `app_builds.id`) pins, if any.
///
/// For the staging function invocation: call it with the **draft** build the
/// staging request resolved, and run the invocation inside
/// `with_staging_pin` with the result. Never call it for a live invocation —
/// the live channel ignores pins by design.
///
/// Returns `Some` only when the pinned revision still exists, is `ready`, and
/// belongs to the build's app's workspace — so a pin can never make a request
/// read another workspace's model (`open_compiled_revision` trusts a pin
/// without re-checking the workspace). Any lookup error reads as "no pin":
/// the request then serves the promoted revision, which is today's behaviour.
pub async fn pinned_revision_for(db: &DatabaseConnection, build_id: Uuid) -> Option<Uuid> {
    let sql = "SELECT r.revision_id FROM app_builds b \
               JOIN apps a ON a.id = b.app_id \
               JOIN revisions r ON r.revision_id = b.semantic_revision_id \
               WHERE b.id = $1 AND r.workspace_id = a.project_id AND r.status = 'ready'";
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [build_id.into()],
        ))
        .await;
    match row {
        Ok(Some(r)) => r.try_get::<Uuid>("", "revision_id").ok(),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(%build_id, error = %e, "staging pin lookup failed; serving the promoted revision");
            None
        }
    }
}

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
    if !oxy_app_core::compiled_reader::revision_is_servable(db, revision_id).await {
        return Err(PinRefusal::Unservable(revision_id));
    }
    Ok(rev)
}

/// Why a publish's `semantic_revision_id` was refused. Every variant is a 400
/// or 422 the publisher can act on.
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

/// Check a publish's requested pin: never with promote, and the revision must
/// be a `ready` `staging`/`main` revision of the app's own workspace.
pub async fn validate_pin_for_publish(
    db: &DatabaseConnection,
    project_id: Uuid,
    revision_id: Uuid,
    promote: bool,
) -> Result<(), PinRefusal> {
    if promote {
        return Err(PinRefusal::WithPromote);
    }
    let rev = entity::revisions::Entity::find_by_id(revision_id)
        .one(db)
        .await
        .map_err(|e| PinRefusal::Db(e.to_string()))?
        .ok_or(PinRefusal::NotFound(revision_id))?;
    check_revision(&rev, project_id)
}

/// The pure half of [`validate_pin_for_publish`].
fn check_revision(rev: &entity::revisions::Model, project_id: Uuid) -> Result<(), PinRefusal> {
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
