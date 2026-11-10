//! `oxy.ask_finished`: the Ask answer someone asked to hear about is ready —
//! and `oxy.ask_failed`: it could not be given (the run failed or timed out).
//! Two kinds, so a push, which carries only its kind's label, says which.
//!
//! **Nothing is sent by default.** A client asks while the run is going —
//! `POST …/analytics/runs/{id}/notify` records `agentic_runs.notify_requested_at`
//! — and someone who stays and watches gets nothing. When the run ends, one of
//! `agentic-runtime`'s observers lands here: [`AskFinishedNotices`] for a run
//! written `done`, `RunFailureNotices` (`run_failed.rs`) for one written
//! failed or timed out. A request that finds the run already ended reaches
//! [`AskFinishedNotices`] through its own port
//! (`agentic_runtime::lifecycle::notice_request`), never the failure observer:
//! that one also announces `oxy.run_failed` by email and Slack, which a thread
//! owner asking about their answer must not be able to re-send.
//!
//! **Told:** the owner of the run's thread, while a member of the org — the
//! only person allowed to ask. A thread nobody owns tells nobody. Never a
//! cancel, a child run, a preview's run, or a run that is not an analytics
//! one.
//!
//! **At most once per run**, by `send::notify`'s per (recipient, org, kind,
//! subject) check — which is also what makes the observer and the request
//! route racing to send harmless.
//!
//! **Rendered when read**, from the run: [`READY`] for `ask_finished`,
//! [`COULD_NOT`] for `ask_failed`; the question as the body, at most
//! [`BODY_MAX`] characters; the thread as the link.

use std::sync::Arc;

use agentic_runtime::entity::run;
use agentic_runtime::lifecycle::done_hook::{RunDoneObserver, set_run_done_observer};
use agentic_runtime::lifecycle::notice_request::{
    NoticeRequestObserver, notice_requested, set_notice_request_observer,
};
use async_trait::async_trait;
use entity::{threads, workspaces};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::kinds::PlatformNotice;
use super::people;
use super::render::Rendered;
use super::run_failed::{cancelled, previewed};
use super::send::{PlatformSent, notify};
use super::subjects::Record;

/// `oxy.ask_finished`'s title.
pub const READY: &str = "Your answer is ready";
/// `oxy.ask_failed`'s title.
pub const COULD_NOT: &str = "Oxygen couldn't answer that";
/// The longest body, in characters: the question, clipped.
pub const BODY_MAX: usize = 120;

/// The runs' `source_type` for Ask (`agentic-pipeline`'s analytics domain).
const ANALYTICS: &str = "analytics";

/// Install the observers. Called at `oxy serve` and `oxy worker` boot, beside
/// `run_failed::install`: either can drive a run to its end, and either can
/// answer the request route.
pub fn install() {
    set_run_done_observer(Arc::new(AskFinishedNotices));
    set_notice_request_observer(Arc::new(AskFinishedNotices));
}

/// The done observer, and the request route's for a run that already ended.
/// Holds no connection: it takes the process's pool when it is called, so
/// installing it at boot needs no database yet.
pub struct AskFinishedNotices;

impl AskFinishedNotices {
    async fn send(run_id: &str) {
        let db = match oxy::database::client::establish_connection().await {
            Ok(db) => db,
            Err(e) => {
                tracing::warn!(%run_id, error = %e, "answer notice not sent: no database");
                return;
            }
        };
        log_outcome(run_id, notify_ask_finished(&db, run_id).await);
    }
}

#[async_trait]
impl RunDoneObserver for AskFinishedNotices {
    async fn root_run_done(&self, run_id: &str) {
        Self::send(run_id).await;
    }
}

#[async_trait]
impl NoticeRequestObserver for AskFinishedNotices {
    async fn requested_after_end(&self, run_id: &str) {
        Self::send(run_id).await;
    }
}

/// What a send for `run_id` did, in the log.
pub(super) fn log_outcome(run_id: &str, outcome: Result<Option<PlatformSent>, DbErr>) {
    match outcome {
        Ok(Some(sent)) if sent.recipients > 0 => tracing::info!(%run_id, "answer notice sent"),
        Ok(Some(_)) => tracing::debug!(%run_id, "answer notice not sent: already told"),
        Ok(None) => {}
        Err(e) => tracing::warn!(%run_id, error = %e, "answer notice not sent"),
    }
}

/// The notice for run `run_id`, when its thread's owner asked for one:
/// `oxy.ask_finished` for a run that is `done`, `oxy.ask_failed` for one that
/// failed or timed out. `Ok(None)` when nobody is to be told.
pub async fn notify_ask_finished(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<Option<PlatformSent>, DbErr> {
    let Some(row) = run::Entity::find_by_id(run_id.to_string()).one(db).await? else {
        return Ok(None);
    };
    if !warrants_notice(&row) || !notice_requested(db, run_id).await? {
        return Ok(None);
    }
    let Some((org_id, owner)) = asker(db, &row).await? else {
        return Ok(None);
    };
    let (workspace_id, run_id) = (row.workspace_id, row.id.clone());
    let notice = match row.task_status.as_deref() {
        Some("done") => PlatformNotice::AskFinished {
            org_id,
            workspace_id,
            run_id,
        },
        _ => PlatformNotice::AskFailed {
            org_id,
            workspace_id,
            run_id,
        },
    };
    notify(db, &notice, &[owner]).await.map(Some)
}

/// The org and the person to tell about `row`: its thread's owner, while a
/// member of the workspace's org. `None` for an unowned thread, or an owner who
/// has left.
pub(super) async fn asker(
    db: &DatabaseConnection,
    row: &run::Model,
) -> Result<Option<(Uuid, Uuid)>, DbErr> {
    let Some(thread) = row.thread_id else {
        return Ok(None);
    };
    let Some(owner) = threads::Entity::find_by_id(thread)
        .one(db)
        .await?
        .and_then(|t| t.user_id)
    else {
        return Ok(None);
    };
    let Some(org_id) = workspaces::Entity::find_by_id(row.workspace_id)
        .one(db)
        .await?
        .and_then(|w| w.org_id)
    else {
        return Ok(None);
    };
    Ok(people::is_org_member(db, org_id, owner)
        .await?
        .then_some((org_id, owner)))
}

/// Whether a run row is an Ask run that ended in a way to tell its asker
/// about. Pure, so each exclusion is pinned by a unit test. Whether anyone
/// asked is the separate [`notice_requested`].
pub fn warrants_notice(row: &run::Model) -> bool {
    let ended = matches!(
        row.task_status.as_deref(),
        Some("done" | "failed" | "timed_out")
    );
    ended && is_ask_run(row)
}

/// A root analytics run on a thread, not cancelled and not a preview's: the
/// runs whose asker may be told anything.
pub(super) fn is_ask_run(row: &run::Model) -> bool {
    !cancelled(row)
        && !previewed(row)
        && row.parent_run_id.is_none()
        && row.source_type.as_deref() == Some(ANALYTICS)
        && row.thread_id.is_some()
}

/// What the notice shows of the run.
#[derive(Debug, Clone, PartialEq)]
pub struct AskRecord {
    pub workspace_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub question: String,
}

/// The runs among `ids`, by id.
pub async fn load(
    db: &DatabaseConnection,
    ids: Vec<String>,
) -> Result<Vec<(String, Record)>, DbErr> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(run::Entity::find()
        .filter(run::Column::Id.is_in(ids))
        .all(db)
        .await?
        .into_iter()
        .map(|r| {
            let record = AskRecord {
                workspace_id: r.workspace_id,
                thread_id: r.thread_id,
                question: r.question,
            };
            (r.id, Record::Ask(record))
        })
        .collect())
}

/// The words for an ended Ask run, by the notice's `kind`: the kind, not the
/// run's status, says how it ended. The link opens its thread, in the web app
/// and — read from the same path — natively in the mobile apps.
pub fn words(ask: &AskRecord, kind: &str, org_slug: Option<&str>) -> Rendered {
    Rendered {
        title: title(kind).to_string(),
        body: Some(clip(&ask.question)).filter(|b| !b.is_empty()),
        href: thread_href(org_slug, ask.workspace_id, ask.thread_id),
    }
}

/// The title of `kind`, one of the two this module sends.
pub fn title(kind: &str) -> &'static str {
    if kind == super::kinds::ASK_FAILED {
        COULD_NOT
    } else {
        READY
    }
}

/// `/{org}/workspaces/{ws}/threads/{thread}`: the thread, in the web app and —
/// read from the same path — natively in the mobile apps.
pub(super) fn thread_href(
    org_slug: Option<&str>,
    workspace_id: Uuid,
    thread_id: Option<Uuid>,
) -> Option<String> {
    org_slug
        .zip(thread_id)
        .map(|(slug, thread)| format!("/{slug}/workspaces/{workspace_id}/threads/{thread}"))
}

/// `question`, trimmed, and cut to [`BODY_MAX`] characters with an ellipsis
/// when longer.
pub(super) fn clip(question: &str) -> String {
    let q = question.trim();
    if q.chars().count() <= BODY_MAX {
        return q.to_string();
    }
    let head: String = q.chars().take(BODY_MAX - 1).collect();
    format!("{}…", head.trim_end())
}

#[cfg(test)]
#[path = "ask_finished_tests.rs"]
mod tests;
