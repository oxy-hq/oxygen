//! A **draft publish by a sandbox agent token** that was granted an app's
//! staging (`oxy_sbx_`, minted with `staging`; sandbox agent credential
//! design, "Staging option").
//!
//! `POST /api/customer-apps/publish` with `environment=staging` moves the
//! app's draft pointer, as a person's draft does. A draft is not confined to
//! staging, though: left alone, it writes the app row production shares,
//! runs as production's functions on an app with no production build, and
//! spends a retention slot a rollback target may need. So the token's draft
//! is held to six guards, all of them here ([`admit_agent_draft`]):
//!
//! | # | Guard | Refused as | What it prevents |
//! | --- | --- | --- | --- |
//! | 1 | `environment=staging`, and not `promote` | `403 sandbox_token_refused` | the token moving production's pointer |
//! | 2 | the app exists and its staging is the token's to open (oxy-authz, the `app_staging` grant) | `404 environment_not_found` | another app's or org's staging, and a token minted without it |
//! | 3 | the app is live: `published_build_id` and `published_at` both set | `409 app_not_live` | production falling back to the draft build for `/fn` and schedules |
//! | 4 | the publish's workspace is the app's | `409 project_mismatch` | a re-home of `apps.project_id` |
//! | 5 | no `name`, `branch` or `semantic_revision_id`, and the app row is not upserted | `400 publish_field_refused` | the customers' launcher label, the source branch and the row itself changing |
//! | 6 | builds production served keep a retention window of their own (`retention::beyond`) | — | ten drafts pruning the builds production rolls back to |
//!
//! Guard 5's second half is what [`admit_agent_draft`] answers: the app's id,
//! which makes the publish take the row as it is (`AppMutationRollback::
//! Untouched`) instead of upserting it. The one column of the row a draft
//! still writes is the pointer it exists to move, `apps.draft_build_id`,
//! with its `staging` mirror — the console's promote and build history read
//! that column, so leaving it behind would make the next promote ship the
//! previous draft.
//!
//! Guard 6 is not a check: it is the retention rule every publisher prunes
//! by. A draft's build is in the drafts' window, never production's.
//!
//! **Guards 3 and 4 are asked twice, the second time on the locked row.**
//! [`admit_agent_draft`] reads the app before the bundle is stored, to refuse
//! early. Seconds pass before the pointer moves, and an unpublish or a
//! re-home in between would land the draft on an app that is no longer live,
//! or no longer in that workspace. So [`move_draft_pointer`] takes the app's
//! row lock in the transaction that writes the pointer and asks both again
//! under it: a change that came first is seen, and one that comes after waits
//! for the commit.
//!
//! **The build remembers its publisher** (`agent_publish::author`:
//! `app_builds.published_token_id`, set on every build a sandbox agent token
//! publishes — this draft, and a publish to a sandbox of its own). A guard
//! asked at publish time says nothing of an app unpublished *later*, nor of a
//! person who promotes the draft without knowing whose it is. Both are closed
//! where they happen, by that mark (`custom_apps_agent_built`): production
//! never falls back to a build a token published, and no promote or rollback
//! ships one.
//!
//! Nothing here is reached by any other credential, nor by a token minted
//! without staging: `publish::target_of` answers those what it always did.

use axum::http::StatusCode;
use entity::apps;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{
    ActiveModelTrait, ActiveValue, DatabaseConnection, DbErr, EntityTrait, QuerySelect,
    TransactionTrait,
};
use uuid::Uuid;

use crate::server::api::custom_apps_env_resolve::may_open_environment;
use crate::server::api::custom_apps_environments::EnvAction;
use crate::server::api::custom_apps_publish::{PublishError, PublishInput};

/// Why a sandbox agent token's draft was refused, where the refusal is this
/// path's own. Each is answered in the token's one body, with its code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentDraftRefusal {
    /// Guard 2. The app is not there, the token was not granted its staging,
    /// or its minter may no longer open it — one answer for all three.
    #[error(
        "no staging environment to publish to: the app does not exist, or this token was not \
         granted its staging"
    )]
    NotFound,
    /// Guard 3.
    #[error(
        "app {app_slug:?} is not live: it has no production build, so production would run this \
         draft's functions. A person promotes the app first; a sandbox agent token publishes a \
         draft only beside a live build"
    )]
    AppNotLive { app_slug: String },
    /// Guard 4.
    #[error(
        "app {app_slug:?} belongs to workspace {existing_project}, but this publish targets \
         workspace {requested_project}. A sandbox agent token's draft never moves an app: \
         publish with --project {existing_project}"
    )]
    ProjectMismatch {
        app_slug: String,
        existing_project: Uuid,
        requested_project: Uuid,
    },
    /// Guard 5.
    #[error(
        "'{field}' is refused on a sandbox agent token's draft: it would change the app that \
         production serves. Publish without it; a person sets it"
    )]
    FieldRefused { field: &'static str },
}

impl AgentDraftRefusal {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::AppNotLive { .. } | Self::ProjectMismatch { .. } => StatusCode::CONFLICT,
            Self::FieldRefused { .. } => StatusCode::BAD_REQUEST,
        }
    }

    /// The code a client branches on.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "environment_not_found",
            Self::AppNotLive { .. } => "app_not_live",
            Self::ProjectMismatch { .. } => "project_mismatch",
            Self::FieldRefused { .. } => "publish_field_refused",
        }
    }
}

impl From<AgentDraftRefusal> for PublishError {
    fn from(refused: AgentDraftRefusal) -> Self {
        PublishError::AgentDraft(refused)
    }
}

/// The fields of a publish that write the app row, or pin what staging reads,
/// as this publish sent them. Guard 5's first half.
fn refused_field(input: &PublishInput) -> Option<&'static str> {
    [
        ("name", input.name.is_some()),
        ("branch", input.branch.is_some()),
        ("semantic_revision_id", input.semantic_revision_id.is_some()),
    ]
    .into_iter()
    .find_map(|(field, sent)| sent.then_some(field))
}

/// Guards 1 and 5, as far as the request alone says: a draft never promotes,
/// and sends none of the fields that change the app. Asked before anything is
/// read (`agent_publish::AgentPublish::of`), and again by
/// [`admit_agent_draft`].
pub(crate) fn refuse_shape(input: &PublishInput) -> Result<(), PublishError> {
    if input.promote {
        return Err(PublishError::SandboxTokenRefused);
    }
    match refused_field(input) {
        Some(field) => Err(AgentDraftRefusal::FieldRefused { field }.into()),
        None => Ok(()),
    }
}

/// Guard 3: production serves a build of its own. Both columns, as the serve
/// path reads them: an unpublished app keeps neither.
pub(crate) fn is_live(app: &apps::Model) -> bool {
    app.published_build_id.is_some() && app.published_at.is_some()
}

/// Guards 3 and 4, on the app row in hand.
fn refuse_app_state(app: &apps::Model, input: &PublishInput) -> Result<(), AgentDraftRefusal> {
    if !is_live(app) {
        return Err(AgentDraftRefusal::AppNotLive {
            app_slug: app.slug.clone(),
        });
    }
    if app.project_id != input.project_id {
        return Err(AgentDraftRefusal::ProjectMismatch {
            app_slug: app.slug.clone(),
            existing_project: app.project_id,
            requested_project: input.project_id,
        });
    }
    Ok(())
}

/// Whether a sandbox agent token may publish this draft to `app`'s staging:
/// every guard of the module docs, in order. Answers the app's id — the
/// publish then works on that row as it is, and never upserts it.
///
/// Called for a `PublishTarget::AgentDraft` alone, which only
/// `publish::target_of` makes, and only for a token holding a staging grant.
pub(crate) async fn admit_agent_draft(
    db: &DatabaseConnection,
    input: &PublishInput,
    app: Option<&apps::Model>,
) -> Result<Uuid, PublishError> {
    // 1 and 5: staging's pointer alone, and nothing that rewrites the app.
    refuse_shape(input)?;
    // The publisher is a person's token: a machine publish holds no staging.
    let (Some(caller), None, None) = (
        input.publisher.as_ref(),
        input.machine_app_id,
        input.published_via.as_ref(),
    ) else {
        return Err(AgentDraftRefusal::NotFound.into());
    };
    // 2: the app, and its staging by oxy-authz — the token's `app_staging`
    // grant for this app, and its minter's reach, read now.
    let Some(app) = app else {
        return Err(AgentDraftRefusal::NotFound.into());
    };
    if !may_open_environment(db, caller, app, &AppEnvironment::Staging).await {
        return Err(AgentDraftRefusal::NotFound.into());
    }
    // 3 and 4: a live app, in the workspace this publish names.
    refuse_app_state(app, input)?;
    // 5, the other half: the id is what keeps `upsert_app` off the row.
    Ok(app.id)
}

/// Move staging's pointer to a sandbox agent token's draft: `apps.draft_build_id`
/// and its `staging` mirror, in one transaction that **holds the app's row**.
///
/// Guards 3 and 4 are asked again here, of the row as it is under the lock:
/// an app unpublished or re-homed since [`admit_agent_draft`] read it is
/// refused and nothing is written — the caller rolls the stored build back —
/// and an unpublish or a re-home that arrives while this holds the row waits
/// until the pointer is committed. Every other column of the row is left as
/// it was found. The serve path's cached row is dropped once it commits.
pub async fn move_draft_pointer(
    db: &DatabaseConnection,
    app_id: Uuid,
    build_pk: Uuid,
    input: &PublishInput,
) -> Result<(), PublishError> {
    let db_err = |e: DbErr| PublishError::Db(e.to_string());
    let txn = db.begin().await.map_err(db_err)?;
    let row = apps::Entity::find_by_id(app_id)
        .lock_exclusive()
        .one(&txn)
        .await
        .map_err(db_err)?
        .ok_or(AgentDraftRefusal::NotFound)?;
    refuse_app_state(&row, input)?;
    let mut active: apps::ActiveModel = row.into();
    active.draft_build_id = ActiveValue::Set(Some(build_pk));
    active.update(&txn).await.map_err(db_err)?;
    crate::server::api::custom_apps_environments::record_move(
        &txn,
        app_id,
        &AppEnvironment::Staging,
        Some(build_pk),
        EnvAction::Publish,
        input.published_by,
    )
    .await
    .map_err(db_err)?;
    txn.commit().await.map_err(db_err)?;
    crate::server::api::custom_apps_cache::invalidate_app_resolution_cache();
    Ok(())
}

#[cfg(test)]
#[path = "agent_draft_tests.rs"]
mod tests;
