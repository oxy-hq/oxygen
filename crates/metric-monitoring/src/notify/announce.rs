//! One announcement, start to finish: claim what is due, build the message,
//! hand it to the destination, and record how that went.
//!
//! The destination is the one thing here that leaves the process, so it is the
//! one thing behind a trait — the host posts to Slack, a test records what it
//! was handed.

use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DbErr};
use uuid::Uuid;

use super::ledger::{self, Due};
use super::message::{self, SlackMessage};

/// Where a message goes.
#[async_trait]
pub trait Destination: Send + Sync {
    /// What is recorded on the claim as where it went — the Slack channel id.
    fn id(&self) -> &str;

    /// Deliver `message`. An `Err` means it did not go out; its text is shown
    /// to whoever reads the failed run.
    async fn post(&self, message: &SlackMessage) -> Result<(), String>;
}

/// What the message says around the events themselves.
#[derive(Debug, Clone, Copy)]
pub struct Heading<'a> {
    pub workspace_name: &'a str,
    /// The workspace's Insights Inbox, when the host knows its own address.
    pub inbox_url: Option<&'a str>,
}

#[derive(Debug, thiserror::Error)]
pub enum AnnounceError {
    #[error("insight ledger: {0}")]
    Ledger(#[from] DbErr),
    /// Refused by the destination. The claim was released, so the next scan
    /// tries again.
    #[error("{0}")]
    Refused(String),
    /// Posted, but the record of it was not written. The claim goes stale and
    /// the same events can be announced a second time.
    #[error(
        "announced {events} insight(s), but recording it failed, so they may be announced again: {source}"
    )]
    Unrecorded { events: usize, source: DbErr },
}

/// Announce every event due for `due.workspace_id` and return how many that
/// was. `Ok(0)` is the ordinary case of a scan that found nothing new.
pub async fn announce(
    db: &impl ConnectionTrait,
    due: Due,
    heading: Heading<'_>,
    to: &dyn Destination,
) -> Result<usize, AnnounceError> {
    let claim_id = Uuid::new_v4();
    if ledger::claim(db, due, to.id(), claim_id).await? == 0 {
        return Ok(0);
    }
    let workspace_id = due.workspace_id;

    let posted = compose_and_post(db, workspace_id, claim_id, heading, to).await;
    let events = match posted {
        Ok(Some(events)) => events,
        // Nothing was sent, so nothing may stay claimed. A release that fails
        // costs the grace period, not the announcement.
        Ok(None) | Err(_) => {
            if let Err(e) = ledger::release(db, workspace_id, claim_id).await {
                tracing::warn!(
                    target: "metric_monitoring",
                    %workspace_id,
                    error = %e,
                    "insight announcement: claim not released; it goes stale on its own"
                );
            }
            return posted.map(|_| 0);
        }
    };

    ledger::mark_delivered(db, workspace_id, claim_id, due.now)
        .await
        .map_err(|source| AnnounceError::Unrecorded { events, source })?;
    if let Err(e) = ledger::prune(db, workspace_id).await {
        tracing::warn!(
            target: "metric_monitoring",
            %workspace_id,
            error = %e,
            "insight announcement: ledger prune failed"
        );
    }
    Ok(events)
}

/// `Ok(None)` when the claim turned out to hold nothing to say — every bucket
/// was dismissed between the claim and the read.
async fn compose_and_post(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    claim_id: Uuid,
    heading: Heading<'_>,
    to: &dyn Destination,
) -> Result<Option<usize>, AnnounceError> {
    let buckets = ledger::claimed_buckets(db, workspace_id, claim_id).await?;
    let Some(message) = message::build(heading.workspace_name, heading.inbox_url, &buckets) else {
        return Ok(None);
    };
    to.post(&message).await.map_err(AnnounceError::Refused)?;
    Ok(Some(message.events))
}
