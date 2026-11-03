//! The port a planes write calls to pin a custom-app environment's semantic
//! model to a workspace git branch.
//!
//! The environment planes routes (`PUT …/planes`, `POST …/environments`) live
//! in `oxy-api-custom-apps`, which cannot depend on `oxy-app`; the branch
//! lookup, the compile and the pin columns live in `oxy-app`
//! (`server::compile_reconcile::env_pins`). So the planes write names this
//! trait, `oxy-app` implements it, and the composition root (`oxy-server`)
//! layers the implementation onto the custom-app routes as an
//! `Extension<Arc<dyn SemanticPinner>>` — the way `AppLifecycle` reaches the
//! partner console and `FunctionQueryExecutor` reaches the serve route.
//!
//! **A route that finds no pinner answers `503`, it does not skip the pin.**
//! A `branch:<name>` write with the extension missing would otherwise store a
//! choice nothing honours, so the handler turns the absence into
//! [`PinRefusal::unavailable`].
//!
//! **The branch name is resolved here and nowhere else**: a planes write
//! ([`SemanticPinner::check_branch`], [`SemanticPinner::pin`]) and a push to
//! the branch. No request that serves an environment asks where a branch is.

use axum::http::StatusCode;
use uuid::Uuid;

/// A pin operation that was refused, with what the route answers.
///
/// `status` and `code` are the refusal's own (`compile_request::Refusal`'s
/// status; `code` names its kind). The planes write maps an unknown or
/// malformed branch (`400`/`404`) to `400 invalid_planes` and answers anything
/// else with this status and code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct PinRefusal {
    pub status: StatusCode,
    /// A stable, machine-readable kind. Every code `oxy-app`'s pinner emits:
    /// `unknown_branch`, `invalid_branch`, `branch_conflict`,
    /// `branch_needs_working_copy`, `github_unavailable`, `compile_refused`
    /// (a commit that is not one, or a kind this call does not compile),
    /// `workspace_not_found`, `internal`; plus `semantic_pin_unavailable`,
    /// which is [`PinRefusal::unavailable`]'s, from a process with no pinner.
    pub code: &'static str,
    pub message: String,
}

impl PinRefusal {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    /// The pinner is not installed in this process, so a branch cannot be
    /// pinned or checked.
    pub fn unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "semantic_pin_unavailable",
            "this server cannot pin an environment's semantic model to a branch: the pin seam \
             is not installed in this process",
        )
    }

    /// Whether the branch itself is the problem (it does not exist, or the
    /// name is malformed), as opposed to a server or GitHub failure. The
    /// planes write answers these `400 invalid_planes`.
    pub fn is_bad_branch(&self) -> bool {
        matches!(self.code, "unknown_branch" | "invalid_branch")
    }
}

/// Pins an environment's semantic plane to a git branch of the app's
/// workspace. `workspace_id` is `apps.project_id`.
#[async_trait::async_trait]
pub trait SemanticPinner: Send + Sync {
    /// Whether `branch` exists in the workspace. Writes nothing and queues no
    /// compile; the planes write calls it before it stores anything.
    async fn check_branch(&self, workspace_id: Uuid, branch: &str) -> Result<(), PinRefusal>;

    /// Point `env_name` of `app_id` at the head of `branch`: compile it (or
    /// reuse a ready revision) and store the pin. An environment whose head is
    /// still compiling keeps serving the model it had until the compile lands.
    /// A pin for a branch the environment has since left (a later planes write
    /// replaced the choice) writes nothing and is not an error: the later
    /// write's own pin is the one that counts.
    async fn pin(
        &self,
        workspace_id: Uuid,
        app_id: Uuid,
        env_name: &str,
        branch: &str,
    ) -> Result<(), PinRefusal>;
}
