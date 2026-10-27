//! `oxy.run_failed`: an automation or Airway run failed and nothing will retry
//! it.
//!
//! `agentic-runtime` calls [`RunFailureNotices`] after it writes a root run
//! terminal-failed (the coordinator past its retry check, a root timeout, a
//! recovery retirement or dead letter). This reads the row and decides:
//!
//! - only automations and Airway pipelines, and only the root of a run tree;
//! - not a cancel. A cancel can land as `failed` ("cancelled by user") on
//!   several defensive paths, but every one of them sets
//!   `cancel_requested_at` first;
//! - not a workspace preview's run, which nobody started on purpose.
//!
//! Told: whoever started it (`metadata.started_by`, stamped by the HTTP
//! routes) while they are a member of the org; otherwise — a scheduled run,
//! or a starter who left — the workspace's admins. Schedules record no
//! creator, so there is no one closer to tell.
//!
//! An Ask (analytics) run is not this kind's, but a failed one whose thread
//! owner asked to hear when it ended gets `oxy.ask_finished` from the same
//! observer ([`super::ask_finished`]).
//!
//! **Once per failing streak, then daily** ([`run_streak`]), so a pipeline
//! that fails every hour does not email its admins every hour. A person is
//! told about a failure unless they were told about a run of the same streak
//! in the last [`REMINDER_AFTER`]: the first failure after a success tells
//! them, the next ones do not, and the first one a day later reminds them
//! ("still failing"). The check runs under a lock on the source, so two of
//! its runs failing at once tell each person once.

use std::sync::Arc;

use agentic_runtime::entity::run;
use agentic_runtime::lifecycle::failure_hook::{RunFailureObserver, set_run_failure_observer};
use async_trait::async_trait;
use chrono::Utc;
use entity::workspaces;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait, TransactionTrait};
use uuid::Uuid;

use super::kinds::PlatformNotice;
use super::people;
use super::run_streak::{self, REMINDER_AFTER, Source};
use super::send::{self, PlatformSent, notify};
use super::subjects::run_record;

/// Install the observer. Called at `oxy serve` and `oxy worker` boot: either
/// can drive a run to its end.
pub fn install() {
    set_run_failure_observer(Arc::new(RunFailureNotices));
}

/// The observer. Holds no connection: it takes the process's pool when a run
/// fails, so installing it at boot needs no database yet.
pub struct RunFailureNotices;

#[async_trait]
impl RunFailureObserver for RunFailureNotices {
    async fn root_run_failed(&self, run_id: &str) {
        let db = match oxy::database::client::establish_connection().await {
            Ok(db) => db,
            Err(e) => {
                tracing::warn!(%run_id, error = %e, "run failure notice not sent: no database");
                return;
            }
        };
        announce_failure(&db, run_id).await;
    }
}

/// What the observer does once it has a connection: the `oxy.run_failed`
/// notice when the run warrants one, then the `oxy.ask_finished` one when it
/// is an Ask run whose asker wanted to hear. Logs, never fails.
pub async fn announce_failure(db: &DatabaseConnection, run_id: &str) {
    match notify_run_failed(db, run_id).await {
        Ok(Some(sent)) if sent.recipients == 0 && sent.already_told > 0 => tracing::info!(
            %run_id,
            already_told = sent.already_told,
            "run failure notice not sent: already told about this failing streak"
        ),
        Ok(Some(sent)) if sent.recipients == 0 => {
            tracing::info!(%run_id, "run failure notice not sent: no one to tell")
        }
        Ok(Some(sent)) => {
            tracing::info!(%run_id, recipients = sent.recipients, "run failure notice sent")
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(%run_id, error = %e, "run failure notice not sent"),
    }
    super::ask_finished::log_outcome(
        run_id,
        super::ask_finished::notify_ask_finished(db, run_id).await,
    );
}

/// The notice for run `run_id`, if it is one that warrants one.
pub async fn notify_run_failed(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<Option<PlatformSent>, DbErr> {
    let Some(row) = run::Entity::find_by_id(run_id.to_string()).one(db).await? else {
        return Ok(None);
    };
    if !warrants_notice(&row) {
        return Ok(None);
    }
    let Some(record) = run_record(&row) else {
        return Ok(None);
    };
    let Some(org_id) = workspaces::Entity::find_by_id(row.workspace_id)
        .one(db)
        .await?
        .and_then(|w| w.org_id)
    else {
        return Ok(None);
    };
    let notice = match record.airway {
        true => PlatformNotice::AirwayRunFailed {
            org_id,
            workspace_id: row.workspace_id,
            run_id: row.id.clone(),
        },
        false => PlatformNotice::AutomationRunFailed {
            org_id,
            workspace_id: row.workspace_id,
            run_id: row.id.clone(),
        },
    };
    let recipients = recipients(db, org_id, &row).await?;
    let Some(source) = Source::of(&row) else {
        return notify(db, &notice, &recipients).await.map(Some);
    };
    notify_once_per_streak(db, &source, &row.id, &notice, &recipients)
        .await
        .map(Some)
}

/// [`notify`] the `recipients` not told about `run_id`'s streak in the last
/// [`REMINDER_AFTER`]. The streak is read under the source's lock and the send
/// written in the same transaction, so a second failure deciding at the same
/// moment sees the first one's notice.
async fn notify_once_per_streak(
    db: &DatabaseConnection,
    source: &Source,
    run_id: &str,
    notice: &PlatformNotice,
    recipients: &[Uuid],
) -> Result<PlatformSent, DbErr> {
    let mut people: Vec<Uuid> = Vec::with_capacity(recipients.len());
    for r in recipients {
        if !people.contains(r) {
            people.push(*r);
        }
    }
    let workspace = send::delivery_workspace(db, notice).await?;
    let txn = db.begin().await?;
    source.lock(&txn).await?;
    let streak = run_streak::streak_of(&txn, run_id).await?;
    let quiet_since = Utc::now() - REMINDER_AFTER;
    let told = run_streak::told_after(&txn, notice, streak, &people, quiet_since).await?;
    let fresh: Vec<Uuid> = people
        .iter()
        .copied()
        .filter(|u| !told.contains(u))
        .collect();
    if fresh.is_empty() {
        txn.rollback().await?;
        return Ok(PlatformSent {
            id: None,
            recipients: 0,
            already_told: people.len(),
        });
    }
    let mut sent = send::write(&txn, notice, &fresh, workspace).await?;
    txn.commit().await?;
    send::after_commit(db, notice, &sent, workspace).await;
    sent.already_told += told.len();
    Ok(sent)
}

/// Whether a run row is a terminal failure someone should hear about. Pure,
/// so each exclusion is pinned by a unit test.
pub fn warrants_notice(row: &run::Model) -> bool {
    let failed = matches!(row.task_status.as_deref(), Some("failed" | "timed_out"));
    failed
        && !cancelled(row)
        && !previewed(row)
        && row.parent_run_id.is_none()
        && run_record(row).is_some()
}

/// A cancel. It can land as `failed` ("cancelled by user") on several
/// defensive paths, but every one of them sets `cancel_requested_at` first.
pub(super) fn cancelled(row: &run::Model) -> bool {
    row.cancel_requested_at.is_some() || row.error_message.as_deref() == Some("cancelled by user")
}

/// A workspace preview's run: nobody is told about one.
pub(super) fn previewed(row: &run::Model) -> bool {
    row.metadata
        .as_ref()
        .and_then(|m| m.get(agentic_runtime::crud::PREVIEW_STAMP_KEY))
        .is_some_and(|v| !v.is_null())
}

/// The starter while still a member, else the workspace's admins.
async fn recipients(
    db: &DatabaseConnection,
    org_id: Uuid,
    row: &run::Model,
) -> Result<Vec<Uuid>, DbErr> {
    let starter = row
        .metadata
        .as_ref()
        .and_then(|m| m.get(agentic_pipeline::STARTED_BY_KEY))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());
    if let Some(user) = starter
        && people::is_org_member(db, org_id, user).await?
    {
        return Ok(vec![user]);
    }
    people::workspace_admins(db, org_id, row.workspace_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn failed_automation() -> run::Model {
        let now = Utc::now().fixed_offset();
        run::Model {
            id: "r-1".into(),
            question: String::new(),
            answer: None,
            error_message: Some("step 2: query failed".into()),
            thread_id: None,
            source_type: Some(agentic_pipeline::WORKFLOW_SOURCE_TYPE.into()),
            metadata: Some(serde_json::json!({ "workflow_ref": "a.automation.yml" })),
            parent_run_id: None,
            schedule_id: None,
            task_status: Some("failed".into()),
            task_metadata: None,
            attempt: 0,
            recovery_requested_at: None,
            driver_id: None,
            driver_heartbeat_at: None,
            cancel_requested_at: None,
            workspace_id: Uuid::nil(),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn a_failed_or_timed_out_root_automation_warrants_a_notice() {
        assert!(warrants_notice(&failed_automation()));
        let mut timed_out = failed_automation();
        timed_out.task_status = Some("timed_out".into());
        assert!(warrants_notice(&timed_out));
    }

    #[test]
    fn cancels_children_previews_and_other_runs_do_not() {
        let mut r = failed_automation();
        r.cancel_requested_at = Some(Utc::now().fixed_offset());
        assert!(!warrants_notice(&r), "a cancel recorded as failed");
        let mut r = failed_automation();
        r.error_message = Some("cancelled by user".into());
        assert!(!warrants_notice(&r), "a cancel without the flag");
        let mut r = failed_automation();
        r.parent_run_id = Some("r-0".into());
        assert!(!warrants_notice(&r), "a child run");
        let mut r = failed_automation();
        r.metadata = Some(serde_json::json!({
            "workflow_ref": "a.automation.yml",
            agentic_runtime::crud::PREVIEW_STAMP_KEY: { "revision_id": "x" },
        }));
        assert!(!warrants_notice(&r), "a preview run");
        let mut r = failed_automation();
        r.source_type = Some("analytics".into());
        assert!(!warrants_notice(&r), "an analytics run");
        let mut r = failed_automation();
        r.task_status = Some("done".into());
        assert!(!warrants_notice(&r), "a finished run");
    }
}
