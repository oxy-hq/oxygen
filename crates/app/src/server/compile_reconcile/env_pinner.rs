//! `oxy-app`'s implementation of the pin port the environment planes routes
//! call ([`oxy_app_core::semantic_pin_seam::SemanticPinner`]): a thin adapter
//! over [`super::env_pins`] that turns `compile_request::Refusal` into the
//! refusal the route answers with.
//!
//! Installed by the composition root (`oxy-server`) as an
//! `Extension<Arc<dyn SemanticPinner>>` on the custom-app routes. It holds no
//! state: each call opens its own connection, as the planes handlers do.

use entity::workspaces;
use oxy_app_core::semantic_pin_seam::{PinRefusal, SemanticPinner};
use sea_orm::{DatabaseConnection, DbErr, EntityTrait};
use uuid::Uuid;

use super::env_pins;
use crate::server::compile_request::Refusal;

/// The production [`SemanticPinner`].
#[derive(Debug, Clone, Copy, Default)]
pub struct EnvPinner;

#[async_trait::async_trait]
impl SemanticPinner for EnvPinner {
    async fn check_branch(&self, workspace_id: Uuid, branch: &str) -> Result<(), PinRefusal> {
        let db = connect().await?;
        let workspace = workspace(&db, workspace_id).await?;
        env_pins::branch_exists(&workspace, branch)
            .await
            .map_err(refusal)
    }

    async fn pin(
        &self,
        workspace_id: Uuid,
        app_id: Uuid,
        env_name: &str,
        branch: &str,
    ) -> Result<(), PinRefusal> {
        let db = connect().await?;
        let workspace = workspace(&db, workspace_id).await?;
        let outcome = env_pins::pin_to_branch(&db, &workspace, app_id, env_name, branch)
            .await
            .map_err(refusal)?;
        tracing::info!(%app_id, env = env_name, %branch, ?outcome, "pinned an environment's semantic plane");
        Ok(())
    }
}

async fn connect() -> Result<DatabaseConnection, PinRefusal> {
    oxy::database::client::establish_connection()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "semantic pin: could not connect");
            internal()
        })
}

async fn workspace(db: &DatabaseConnection, id: Uuid) -> Result<workspaces::Model, PinRefusal> {
    workspaces::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(database)?
        .ok_or_else(|| {
            PinRefusal::new(
                axum::http::StatusCode::NOT_FOUND,
                "workspace_not_found",
                format!("the app's workspace {id} does not exist"),
            )
        })
}

fn database(error: DbErr) -> PinRefusal {
    tracing::error!(%error, "semantic pin: database error");
    internal()
}

/// A server-side failure as the client sees it: the status and code, and no
/// detail. The detail is logged where the failure is handled.
fn internal() -> PinRefusal {
    PinRefusal::new(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "internal error",
    )
}

/// A `compile_request` refusal as the route's: its own status and message,
/// and a code naming its kind. An internal failure keeps its status and code
/// but not its text, which can carry database or token-service detail: that is
/// logged, and the client gets `internal error`.
pub(crate) fn refusal(refusal: Refusal) -> PinRefusal {
    if let Refusal::Internal(detail) = &refusal {
        tracing::error!(%detail, "semantic pin: the compile request failed");
        return internal();
    }
    let code = match &refusal {
        Refusal::InvalidBranch(_) => "invalid_branch",
        Refusal::UnknownBranch(_) => "unknown_branch",
        Refusal::Conflict(_) => "branch_conflict",
        Refusal::NeedsWorkingCopy { .. } => "branch_needs_working_copy",
        Refusal::GitHubUnavailable { .. } => "github_unavailable",
        Refusal::NotACommit(_) | Refusal::UnsupportedKind(_) => "compile_refused",
        Refusal::Internal(_) => "internal",
    };
    PinRefusal::new(refusal.status(), code, refusal.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    /// A refusal keeps its own status and message, and only the two refusals
    /// about the branch itself are `is_bad_branch` (the planes write answers
    /// those `400 invalid_planes`; GitHub being down stays a 503).
    #[test]
    fn a_refusal_keeps_its_status_and_only_the_branch_ones_are_bad_branches() {
        let unknown = refusal(Refusal::UnknownBranch("branch \"x\" is unknown".into()));
        assert_eq!(unknown.status, StatusCode::NOT_FOUND);
        assert_eq!(unknown.code, "unknown_branch");
        assert!(unknown.message.contains("\"x\""));
        assert!(unknown.is_bad_branch());

        let invalid = refusal(Refusal::InvalidBranch("no".into()));
        assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
        assert!(invalid.is_bad_branch());

        let down = refusal(Refusal::GitHubUnavailable {
            branch: "x".into(),
            message: "timeout".into(),
        });
        assert_eq!(down.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(down.code, "github_unavailable");
        assert!(!down.is_bad_branch());

        // Internal detail (database, token minting) never reaches the client.
        let internal = refusal(Refusal::Internal(
            "database error: connection refused at 10.0.0.7".into(),
        ));
        assert_eq!(internal.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(internal.code, "internal");
        assert_eq!(internal.message, "internal error");

        let conflict = refusal(Refusal::Conflict("dirty".into()));
        assert_eq!(conflict.status, StatusCode::CONFLICT);
        assert!(!conflict.is_bad_branch());
    }
}
