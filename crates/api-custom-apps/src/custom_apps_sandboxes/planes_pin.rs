//! The `semantic` plane's two steps around a planes write: ask the workspace
//! whether the branch exists **before** the choice is stored, and pin the
//! environment to it **after** it is stored.
//!
//! The work itself is `oxy-app`'s (`env_pins`), reached through
//! [`SemanticPinner`], which the composition root layers onto these routes as
//! an `Extension<Arc<dyn SemanticPinner>>`. A route that finds none answers
//! `503 semantic_pin_unavailable` to a write that needs it (a `branch:<name>`
//! choice) instead of storing a choice nothing honours. A write that names no
//! branch never asks.
//!
//! **This is the one place a branch name is resolved.** Serving an environment
//! reads the pinned revision the columns hold; it never asks where the branch
//! is.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use entity::apps;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_app_core::semantic_pin_seam::{PinRefusal, SemanticPinner};
use sea_orm::DatabaseConnection;
use serde_json::Value;
use uuid::Uuid;

use super::SandboxError;
use crate::custom_apps_planes::{PlaneSpec, RequestedPlanes};

/// The pinner a request found, if the composition root installed one.
pub(crate) type Pinner<'a> = Option<&'a Arc<dyn SemanticPinner>>;

/// A request's `preset` and `planes`, expanded and validated for
/// `environment`: the spec and the object to store.
///
/// A `semantic: branch:<name>` choice asks the workspace for the branch first
/// (`400 invalid_planes` naming it when there is none), and records the answer
/// in the `PlaneContext` that `validate` reads
/// ([`super::planes::validate_choice`], which also checks the warehouse
/// overrides against `build`'s mapping). `build` is the build the environment
/// will serve; `fork_at` is a create's `at`, which no row holds yet (`None`
/// keeps the stored row's).
pub(crate) async fn requested(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    (preset, planes): (Option<&str>, Option<&Value>),
    (build, fork_at): (Option<Uuid>, Option<DateTime<Utc>>),
    pinner: Pinner<'_>,
) -> Result<RequestedPlanes, SandboxError> {
    let requested = PlaneSpec::from_request(preset, planes)
        .map_err(|e| SandboxError::InvalidPlanes(e.to_string()))?;
    let branch = requested.spec.semantic_branch();
    if let Some(branch) = branch {
        let pinner = pinner.ok_or_else(|| SandboxError::Pin(PinRefusal::unavailable()))?;
        match pinner.check_branch(app.project_id, branch).await {
            Ok(()) => {}
            Err(refusal) if refusal.is_bad_branch() => {
                return Err(SandboxError::InvalidPlanes(format!(
                    "semantic: branch {branch:?} cannot be used: {}",
                    refusal.message
                )));
            }
            Err(refusal) => return Err(SandboxError::Pin(refusal)),
        }
    }
    super::planes::validate_choice(
        db,
        app,
        environment,
        &requested.spec,
        build,
        (fork_at, branch),
    )
    .await?;
    Ok(requested)
}

/// The write a pin follows, which decides the retry a failure names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PinAfter {
    /// `POST …/environments`: the sandbox now exists, so sending the create
    /// again answers `409 environment_exists`; the retry is a planes write.
    Create,
    /// `PUT …/environments/{name}/planes`: the same request again re-pins.
    PlanesWrite,
}

/// After a stored planes write: pin the environment to the choice's `branch`,
/// if it names one. Leaving a branch needs no call: the store itself dropped
/// the old pin ([`super::planes::set`]), and the pin reader ignores the columns
/// of any mode that is not `branch`.
///
/// A failure is logged and answered, and the planes stay stored: a planes write
/// naming the branch again re-pins ([`retry_message`] says which to send).
/// Until it does the environment is a branch with no pin, which the reader
/// refuses (`503 semantic_branch_not_ready`) rather than serving another model.
pub(crate) async fn settle(
    pinner: Pinner<'_>,
    app: &apps::Model,
    environment: &AppEnvironment,
    branch: Option<&str>,
    after: PinAfter,
) -> Result<(), SandboxError> {
    let Some(branch) = branch else {
        return Ok(());
    };
    let name = environment.name();
    let outcome = match pinner {
        Some(pinner) => pinner.pin(app.project_id, app.id, &name, branch).await,
        None => Err(PinRefusal::unavailable()),
    };
    outcome.map_err(|refusal| {
        tracing::warn!(
            app_id = %app.id, env = %name, %branch, code = refusal.code, error = %refusal,
            "the planes were stored but the environment's semantic pin could not be set"
        );
        SandboxError::Pin(PinRefusal {
            message: retry_message(after, &name, branch, &refusal.message),
            ..refusal
        })
    })
}

/// What a failed pin answers: what was stored, why the pin failed, and the
/// retry that works for `after` — never a create sent again.
pub(crate) fn retry_message(after: PinAfter, environment: &str, branch: &str, why: &str) -> String {
    let update = format!(
        "`oxyc env update <app> {environment} --plane semantic=branch:{branch}` \
         (PUT …/environments/{environment}/planes)"
    );
    match after {
        PinAfter::Create => format!(
            "the {environment} environment was created with its planes, but the semantic pin \
             could not be set ({why}); pin it with {update}, not by creating it again"
        ),
        PinAfter::PlanesWrite => format!(
            "the planes were stored, but the semantic pin could not be set ({why}); send the \
             same request again, {update}, to retry"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{PinAfter, retry_message};

    /// A create's failure never tells the caller to create again: the sandbox
    /// exists, so the retry it names is the planes write.
    #[test]
    fn each_write_names_the_retry_that_works() {
        let create = retry_message(PinAfter::Create, "dev-a1", "feat-x", "github is down");
        assert!(create.contains("was created"), "{create}");
        assert!(create.contains("not by creating it again"), "{create}");
        let write = retry_message(PinAfter::PlanesWrite, "dev-a1", "feat-x", "github is down");
        assert!(write.contains("same request again"), "{write}");
        for message in [create, write] {
            assert!(message.contains("github is down"), "{message}");
            assert!(
                message.contains("oxyc env update <app> dev-a1 --plane semantic=branch:feat-x"),
                "{message}"
            );
        }
    }
}
