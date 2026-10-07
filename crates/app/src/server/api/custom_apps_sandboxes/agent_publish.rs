//! A publish by a **sandbox agent token** (`oxy_sbx_`): what it may target,
//! what it is told when refused, and its audit row (sandbox agent credential
//! design §1 row P1, §3.3).
//!
//! The token publishes to a sandbox it created itself — and, when it was
//! granted an app's staging, a draft to that staging (`agent_draft`) — and
//! nowhere else:
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

/// The sandbox agent token a build is marked with
/// (`app_builds.published_token_id`): the publisher's, for **every** build
/// such a token publishes — to a sandbox of its own, or as a draft to staging.
/// `None`, the column's `NULL`, for every publish by anyone else.
///
/// Nobody approved the code in a build an agent published, wherever it went:
/// promote latest takes an app's newest build whatever served it, and a
/// rollback names any retained build, so a sandbox build is as shippable as a
/// draft. The mark is what every promote path refuses
/// (`custom_apps_agent_built`). It changes nothing a sandbox does with its own
/// build: a sandbox serves what its row names, and no reader of the row, of
/// its creation or of its retention window looks at the mark.
pub(crate) fn author(input: &PublishInput) -> Option<Uuid> {
    token_of(input)
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
    ///
    /// A draft to staging (`PublishTarget::AgentDraft`) is the one publish
    /// that moves staging's pointer for the token. It is held here, before
    /// anything is read, to the shape `agent_draft` asks of it — never a
    /// promote, none of the fields that rewrite the app — and to the rest of
    /// its guards by `admit_agent_draft`. `PublishTarget::Channels` stays
    /// refused: only `publish::target_of` makes the draft target, and only
    /// for a token holding a staging grant.
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
            PublishTarget::AgentDraft => {
                super::agent_draft::refuse_shape(input)?;
                Ok(Some(Self {
                    environment: AppEnvironment::Staging.name(),
                }))
            }
            _ => Err(PublishError::SandboxTokenRefused),
        }
    }

    /// The environment that is not there, as this publish is told it: the
    /// sandbox it named, or — for a draft — the staging it was not granted.
    fn not_found(&self) -> PublishError {
        if self.environment == AppEnvironment::Staging.name() {
            return super::agent_draft::AgentDraftRefusal::NotFound.into();
        }
        PublishError::UnknownEnvironment {
            name: self.environment.clone(),
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
            | PublishError::SandboxRefused => self.not_found(),
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

#[cfg(test)]
#[path = "agent_publish_mark_tests.rs"]
mod mark_tests;
