//! `AnomalyNotifyExecutor`: runs one `anomaly_notify` `TaskSpec::Custom` on the
//! global-run fleet. Registered by `build_custom_task_registry`
//! (`server::router::recovery`). The mechanism is
//! `internal-docs/agentic-runtime-integration.md` ("One-shot queue work").
//!
//! All this file decides is *where* a workspace's message goes — its org's
//! Slack installation, the channel the payload names. What is announced, and
//! the claim around it, is `oxy_metric_monitoring::notify`.

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use chrono::Utc;
use entity::{organizations, workspaces};
use futures::future::FutureExt;
use oxy_metric_monitoring::notify::{Destination, Due, Heading, SlackMessage, announce};
use oxy_slack_client::SlackClient;
use sea_orm::{DatabaseConnection, EntityTrait};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{ANOMALY_NOTIFY_KIND, AnomalyNotifyPayload};
use crate::integrations::slack::config::SlackConfig;
use crate::integrations::slack::services::installations::InstallationsService;

pub struct AnomalyNotifyExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for AnomalyNotifyExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let TaskSpec::Custom { kind, payload } = &assignment.spec else {
            return Err(format!(
                "unexpected spec for AnomalyNotifyExecutor: {:?}",
                assignment.spec
            ));
        };
        if kind != ANOMALY_NOTIFY_KIND {
            return Err(format!("unknown insights-delivery kind: {kind}"));
        }
        let payload: AnomalyNotifyPayload = serde_json::from_value(payload.clone())
            .map_err(|e| format!("bad insights-delivery payload: {e}"))?;

        let (event_tx, event_rx) = mpsc::channel(16);
        let (outcome_tx, outcome_rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let db = self.db.clone();

        tokio::spawn(async move {
            let _ = event_tx
                .send((
                    "anomaly_notify_started".into(),
                    serde_json::json!({ "workspace_id": payload.workspace_id }),
                ))
                .await;
            let _ = outcome_tx.send(outcome_of(&db, &payload).await).await;
        });

        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel,
            answers: None,
        })
    }
}

/// Run the delivery and turn whatever happened into a terminal outcome — a
/// panic included, or the run would sit `running` with nothing left to end it.
async fn outcome_of(db: &DatabaseConnection, payload: &AnomalyNotifyPayload) -> TaskOutcome {
    let workspace_id = payload.workspace_id;
    let result = std::panic::AssertUnwindSafe(deliver(db, payload))
        .catch_unwind()
        .await;
    match result {
        Ok(Ok(answer)) => TaskOutcome::Done {
            answer,
            metadata: Some(serde_json::json!({ "workspace_id": workspace_id })),
        },
        Ok(Err(e)) => TaskOutcome::Failed(e),
        Err(_) => {
            tracing::error!(target: "metric_anomalies", %workspace_id, "insights delivery panicked");
            TaskOutcome::Failed("insights delivery panicked".to_string())
        }
    }
}

/// Find the workspace's Slack and announce what is due. The `Ok` text is the
/// run's answer; the `Err` text is what a failed run shows, so each one names
/// what to change.
async fn deliver(
    db: &DatabaseConnection,
    payload: &AnomalyNotifyPayload,
) -> Result<String, String> {
    let workspace = workspaces::Entity::find_by_id(payload.workspace_id)
        .one(db)
        .await
        .map_err(|e| format!("load workspace: {e}"))?
        .ok_or("the workspace no longer exists")?;
    let org_id = workspace
        .org_id
        .ok_or("the workspace belongs to no org, so it has no Slack to post to")?;
    let installation = InstallationsService::find_active_by_org(org_id)
        .await
        .map_err(|e| format!("load Slack installation: {e}"))?
        .ok_or("`.monitor.yml` has a `notify:` block, but this org has not connected Slack")?;
    let bot_token = InstallationsService::decrypt_bot_token(&installation)
        .await
        .map_err(|e| format!("open the Slack bot token: {e}"))?;

    let to = SlackChannel {
        client: SlackClient::new(),
        bot_token,
        channel: payload.notify.slack_channel.clone(),
    };
    let inbox_url = inbox_url(db, org_id, workspace.id).await;
    let heading = Heading {
        workspace_name: &workspace.name,
        inbox_url: inbox_url.as_deref(),
    };
    let due = Due {
        workspace_id: workspace.id,
        min_severity: payload.notify.min_severity,
        now: Utc::now(),
    };
    match announce(db, due, heading, &to).await {
        Ok(0) => Ok("nothing new to announce".to_string()),
        Ok(events) => Ok(format!("announced {events} insight(s) in {}", to.channel)),
        Err(e) => Err(e.to_string()),
    }
}

/// The workspace's Insights Inbox on the main site, when this deployment knows
/// its own address. The same base the Slack bot links threads with.
async fn inbox_url(db: &DatabaseConnection, org_id: Uuid, workspace_id: Uuid) -> Option<String> {
    let base = &SlackConfig::cached().as_runtime()?.app_base_url;
    let slug = organizations::Entity::find_by_id(org_id)
        .one(db)
        .await
        .ok()
        .flatten()?
        .slug;
    // Slack's `<url|label>` has no escape for `|`.
    Some(
        format!("{base}/{slug}/workspaces/{workspace_id}/ide/semantic?view=anomalies")
            .replace('|', ""),
    )
}

/// One channel in an org's Slack, posted to as that org's Oxygen app.
pub struct SlackChannel {
    pub client: SlackClient,
    pub bot_token: String,
    pub channel: String,
}

#[async_trait]
impl Destination for SlackChannel {
    fn id(&self) -> &str {
        &self.channel
    }

    async fn post(&self, message: &SlackMessage) -> Result<(), String> {
        self.client
            .chat_post_message_with_blocks(
                &self.bot_token,
                &self.channel,
                &message.text,
                None,
                Some(message.blocks.clone()),
            )
            .await
            .map(|_| ())
            .map_err(|e| explain(&self.channel, &e.to_string()))
    }
}

/// Slack's refusals that a person can fix, said as the fix. Anything else is
/// passed through as Slack worded it.
fn explain(channel: &str, error: &str) -> String {
    let fix = if error.ends_with("not_in_channel") {
        "the Oxygen Slack app is not in that channel; invite it there"
    } else if error.ends_with("channel_not_found") {
        "Slack knows no such channel for the Oxygen app; check the id, and invite the app if the \
         channel is private"
    } else if error.ends_with("is_archived") {
        "that channel is archived"
    } else {
        return format!("posting to Slack channel {channel} failed: {error}");
    };
    format!("posting to Slack channel {channel} failed: {fix} ({error})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_someone_can_fix_says_how() {
        let said = explain(
            "C0123ABCDEF",
            "slack chat.postMessage not ok: not_in_channel",
        );
        assert!(
            said.contains("C0123ABCDEF") && said.contains("invite it"),
            "{said}"
        );
        assert!(
            said.ends_with("(slack chat.postMessage not ok: not_in_channel)"),
            "{said}"
        );
    }

    #[test]
    fn any_other_refusal_is_passed_through_as_slack_worded_it() {
        assert_eq!(
            explain("C0123ABCDEF", "slack chat.postMessage not ok: ratelimited"),
            "posting to Slack channel C0123ABCDEF failed: slack chat.postMessage not ok: ratelimited"
        );
    }
}
