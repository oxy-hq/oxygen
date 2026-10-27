//! `oxy.ask_waiting`: an Ask run someone asked to hear about stopped to ask
//! them a clarifying question.
//!
//! The sibling of [`super::ask_finished`], for the person who left a run that
//! then needed them: it waits on their answer and nothing else will tell them.
//! `agentic-runtime`'s suspend observer ([`AskWaitingNotices`]) lands here
//! after the coordinator writes the run `awaiting_input` with its prompt.
//!
//! **Told** exactly as for `ask_finished` — the owner of the run's thread
//! while an org member, only when they asked (`notify_requested_at`), never for
//! a cancel, child, preview or non-analytics run, or an unowned thread — and
//! **only of a suspension that began after they asked.** A request made while
//! the question was already on screen is from someone who saw it; a later
//! suspension, or the end of the run, still tells them.
//!
//! **One notice per suspension.** The subject is the run and the time the
//! suspension began ([`SuspensionSubject`]), so a run that asks twice tells the
//! person twice, and the same suspension observed twice tells them once.
//!
//! **Rendered when read:** [`TITLE`]; the suspension's first prompt, at most
//! [`super::ask_finished::BODY_MAX`] characters, while that suspension is still
//! the run's latest, else the run's question; the thread as the link.

use std::collections::HashMap;
use std::sync::Arc;

use agentic_runtime::entity::{run, run_suspension};
use agentic_runtime::lifecycle::notice_request::notice_requested_at;
use agentic_runtime::lifecycle::suspend_hook::{RunSuspendedObserver, set_run_suspended_observer};
use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::ask_finished::{asker, clip, is_ask_run, log_outcome, thread_href};
use super::kinds::{PlatformNotice, SuspensionSubject};
use super::render::Rendered;
use super::send::{PlatformSent, notify};
use super::subjects::Record;

/// The title, whatever the question.
pub const TITLE: &str = "Oxygen has a question for you";

/// Install the observer. Called at `oxy serve` and `oxy worker` boot, beside
/// `ask_finished::install`: either can drive a run to a suspension.
pub fn install() {
    set_run_suspended_observer(Arc::new(AskWaitingNotices));
}

/// The suspend observer. Holds no connection: it takes the process's pool when
/// a run stops, so installing it at boot needs no database yet.
pub struct AskWaitingNotices;

#[async_trait]
impl RunSuspendedObserver for AskWaitingNotices {
    async fn root_run_suspended(&self, run_id: &str) {
        let db = match oxy::database::client::establish_connection().await {
            Ok(db) => db,
            Err(e) => {
                tracing::warn!(%run_id, error = %e, "question notice not sent: no database");
                return;
            }
        };
        log_outcome(run_id, notify_ask_waiting(&db, run_id).await);
    }
}

/// The notice for run `run_id`'s current suspension, when its thread's owner
/// asked to hear before it began. `Ok(None)` when nobody is to be told.
pub async fn notify_ask_waiting(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<Option<PlatformSent>, DbErr> {
    let Some(row) = run::Entity::find_by_id(run_id.to_string()).one(db).await? else {
        return Ok(None);
    };
    if !warrants_notice(&row) {
        return Ok(None);
    }
    let Some(asked_at) = notice_requested_at(db, run_id).await? else {
        return Ok(None);
    };
    let Some(suspension) = run_suspension::Entity::find_by_id(run_id.to_string())
        .one(db)
        .await?
    else {
        return Ok(None);
    };
    if asked_at >= suspension.created_at {
        // Asked while this question was showing: they have seen it.
        return Ok(None);
    }
    let Some((org_id, owner)) = asker(db, &row).await? else {
        return Ok(None);
    };
    let notice = PlatformNotice::AskWaiting {
        org_id,
        workspace_id: row.workspace_id,
        suspension: SuspensionSubject {
            run: row.id.clone(),
            at: suspension.created_at.with_timezone(&Utc),
        },
    };
    notify(db, &notice, &[owner]).await.map(Some)
}

/// Whether a run row is an Ask run waiting on the person's input. Pure, so
/// each exclusion is pinned by a unit test.
pub fn warrants_notice(row: &run::Model) -> bool {
    row.task_status.as_deref() == Some("awaiting_input") && is_ask_run(row)
}

/// What the notice shows of the run and its suspension.
#[derive(Debug, Clone, PartialEq)]
pub struct AskWaitingRecord {
    pub workspace_id: Uuid,
    pub thread_id: Option<Uuid>,
    /// The suspension's prompt, while it is still the run's latest.
    pub prompt: Option<String>,
    pub question: String,
}

/// The suspensions among `ids`, by id.
pub async fn load(
    db: &DatabaseConnection,
    ids: Vec<String>,
) -> Result<Vec<(String, Record)>, DbErr> {
    let subjects: Vec<(String, SuspensionSubject)> = ids
        .into_iter()
        .filter_map(|id| SuspensionSubject::parse(&id).map(|s| (id, s)))
        .collect();
    if subjects.is_empty() {
        return Ok(Vec::new());
    }
    let run_ids: Vec<String> = subjects.iter().map(|(_, s)| s.run.clone()).collect();
    let runs: HashMap<String, run::Model> = run::Entity::find()
        .filter(run::Column::Id.is_in(run_ids.clone()))
        .all(db)
        .await?
        .into_iter()
        .map(|r| (r.id.clone(), r))
        .collect();
    let suspensions: HashMap<String, run_suspension::Model> = run_suspension::Entity::find()
        .filter(run_suspension::Column::RunId.is_in(run_ids))
        .all(db)
        .await?
        .into_iter()
        .map(|s| (s.run_id.clone(), s))
        .collect();
    Ok(subjects
        .into_iter()
        .filter_map(|(id, subject)| {
            let r = runs.get(&subject.run)?;
            let prompt = suspensions
                .get(&subject.run)
                .filter(|s| s.created_at.with_timezone(&Utc) == subject.at)
                .map(|s| s.prompt.clone());
            let record = AskWaitingRecord {
                workspace_id: r.workspace_id,
                thread_id: r.thread_id,
                prompt,
                question: r.question.clone(),
            };
            Some((id, Record::AskWaiting(record)))
        })
        .collect())
}

/// The words for a waiting Ask run.
pub fn words(waiting: &AskWaitingRecord, org_slug: Option<&str>) -> Rendered {
    // The coordinator joins a suspension's prompts with newlines; the first
    // line is its first question.
    let first_prompt = waiting
        .prompt
        .as_deref()
        .and_then(|p| p.lines().map(str::trim).find(|l| !l.is_empty()));
    let body = clip(first_prompt.unwrap_or(&waiting.question));
    Rendered {
        title: TITLE.to_string(),
        body: Some(body).filter(|b| !b.is_empty()),
        href: thread_href(org_slug, waiting.workspace_id, waiting.thread_id),
    }
}

#[cfg(test)]
#[path = "ask_waiting_tests.rs"]
mod tests;
