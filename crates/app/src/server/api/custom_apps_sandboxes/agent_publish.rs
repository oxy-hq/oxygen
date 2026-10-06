//! A publish by a **sandbox agent token** (`oxy_sbx_`): what it may target,
//! what it is told when refused, and its audit row (sandbox agent credential
//! design §1 row P1, §3.3).
//!
//! The token publishes to a sandbox it created itself and nowhere else:
//!
//! * [`AgentPublish::of`] is asked first by `publish_to`, whoever called it:
//!   a publish to the app's channels, or one that promotes, is
//!   `SandboxTokenRefused`. This is the second refusal of a production write
//!   (decision 6). It reads the publisher and the target and nothing the
//!   route allow-list reads, so removing that list does not open it.
//! * [`AgentPublish::sees`] answers every "you may not" before the bundle is
//!   stored as the sandbox not existing, so the token does not learn which
//!   orgs, apps or sandboxes do.
//! * The pointer move decides ownership again on the row it locks
//!   (`publish::move_pointer`, `own::lock_own`).

use entity::apps;
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use crate::server::api::custom_apps_publish::{
    PublishError, PublishInput, PublishResult, PublishTarget,
};

/// The sandbox agent token a publish arrived with, if that is its credential.
pub(crate) fn token_of(input: &PublishInput) -> Option<Uuid> {
    input
        .publisher
        .as_ref()
        .and_then(|caller| caller.sandbox_agent())
        .map(|reach| reach.token_id)
}

/// A publish the token may attempt: to one named sandbox, not promoting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentPublish {
    environment: String,
}

impl AgentPublish {
    /// `None` for every other credential. For the token: the sandbox it
    /// names, or `SandboxTokenRefused` when the publish would move staging's
    /// or production's pointer.
    pub(crate) fn of(
        input: &PublishInput,
        target: &PublishTarget,
    ) -> Result<Option<Self>, PublishError> {
        if token_of(input).is_none() {
            return Ok(None);
        }
        match target {
            PublishTarget::Sandbox(environment) if !input.promote => Ok(Some(Self {
                environment: environment.name(),
            })),
            _ => Err(PublishError::SandboxTokenRefused),
        }
    }

    /// `refused`, as the token is told it. A refusal that says the org, the
    /// workspace or the app is not the publisher's to use is answered as the
    /// sandbox not existing. Everything else describes the request or the
    /// bundle, and is passed through.
    pub(crate) fn sees(&self, refused: PublishError) -> PublishError {
        match refused {
            PublishError::UnknownOrg(_)
            | PublishError::UnknownProject(..)
            | PublishError::OxyAccessDenied { .. }
            | PublishError::SandboxRefused => PublishError::UnknownEnvironment {
                name: self.environment.clone(),
            },
            other => other,
        }
    }
}

/// One audit row for a publish: the person as the actor, their key or token
/// stamped by `for_request`, the environment as the target. Written for every
/// credential, not a sandbox agent token alone.
pub(crate) async fn audit(db: &DatabaseConnection, actor: &RequestActor, result: &PublishResult) {
    let app = match apps::Entity::find_by_id(result.app_id).one(db).await {
        Ok(Some(app)) => app,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(app_id = %result.app_id, error = %e, "sandbox publish: audit skipped");
            return;
        }
    };
    let Some(environment) = AppEnvironment::parse(&result.environment) else {
        return;
    };
    let entry = AuditEntry::for_request(actor, "app.environment.published");
    let entry = super::ops::scoped(entry, &app, &environment)
        .metadata(serde_json::json!({ "build_id": result.build_id }));
    audit::record_best_effort(db, entry).await;
}

#[cfg(test)]
#[path = "agent_publish_tests.rs"]
mod tests;
