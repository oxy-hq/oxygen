//! An analytics run whose pipeline a driver starts from the queue, not the
//! request that asked for it.
//!
//! The request does two things and starts nothing:
//!
//! 1. [`check_analytics_agent`] answers the questions a client should hear on
//!    the start itself — the agent does not exist (`404`), or the workspace is
//!    not compiled yet (`503`, retryable) — by the same reads
//!    `PipelineBuilder::start` makes before it inserts a run.
//! 2. [`seed_queued_analytics_run`] writes the run, its extension row and its
//!    `queued` Global entry in one transaction. Without the transaction a
//!    failure between them leaves a `running` run that nothing will ever
//!    drive. The queue's wake-up is a NOTIFY held until commit, so no driver
//!    is woken for a row it cannot see yet.
//!
//! The driver that claims the entry starts the pipeline with
//! `PipelineBuilder::existing_run`, so it inserts nothing: the row this seed
//! wrote is the run.
//!
//! **The submitter must not `register` the run in its `RuntimeState`.** A
//! process holding a notifier for a run it does not drive parks its chat
//! stream on a notifier nothing rings (`agentic-http` `routes/run.rs`,
//! `timer_is_the_wake_source`). Every process but the driver keeps no
//! notifier, so its streams poll; the driver registers one and wakes
//! instantly.

use std::sync::Arc;

use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud;
use sea_orm::{DatabaseConnection, DbErr, TransactionTrait};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::platform::PlatformContext;
use crate::{PipelineError, ThinkingMode, run_metadata};

/// What [`seed_queued_analytics_run`] writes.
#[derive(Debug, Clone)]
pub struct QueuedAnalyticsRun {
    pub run_id: String,
    pub workspace_id: Uuid,
    pub agent_id: String,
    pub question: String,
    pub thread_id: Option<Uuid>,
    pub thinking_mode: ThinkingMode,
    /// Host keys for the run's `metadata`, merged under the pipeline's own as
    /// `PipelineBuilder::run_metadata` does — who asked, for instance.
    pub metadata: Map<String, Value>,
    /// The entry a driver claims. Its executor starts the pipeline on this run.
    pub spec: TaskSpec,
}

/// Write the run, its analytics extension row and its `queued` Global entry,
/// atomically. Run row first: the extension row and the queue row both
/// reference it.
///
/// The run's `metadata` carries exactly what `PipelineBuilder::start` writes
/// for a run it inserts (`agent_id`, `thinking_mode`), then the host's keys. A
/// workspace preview's run is never seeded here: its platform holds every
/// write in the request's process, and a driver elsewhere has no such hold.
pub async fn seed_queued_analytics_run(
    db: &DatabaseConnection,
    run: &QueuedAnalyticsRun,
) -> Result<(), DbErr> {
    let mut metadata = serde_json::json!({
        "agent_id": run.agent_id,
        "thinking_mode": run.thinking_mode.to_db(),
    });
    run_metadata::merge(&mut metadata, &run.metadata);

    let txn = db.begin().await?;
    crud::insert_run(
        &txn,
        &run.run_id,
        &run.question,
        run.thread_id,
        "analytics",
        Some(metadata),
        run.workspace_id,
    )
    .await?;
    agentic_analytics::insert_run_meta(&txn, &run.run_id, &run.agent_id, run.thinking_mode.to_db())
        .await?;
    crud::enqueue_task(
        &txn,
        &run.run_id,
        &run.run_id,
        None,
        &run.spec,
        None,
        crud::TaskScope::Global,
    )
    .await?;
    txn.commit().await
}

/// Can `agent_id` be started in this workspace? The reads
/// `PipelineBuilder::start` makes before it inserts a run, and nothing else:
/// the agent's definition (compiled, or this node's working copy) and the
/// context documents it names. An error is the one the start would have
/// returned — `PipelineError::is_retryable` for "not compiled yet".
pub async fn check_analytics_agent(
    platform: &Arc<dyn PlatformContext>,
    agent_id: &str,
) -> Result<(), PipelineError> {
    let ctx_root = platform.context_root().await;
    crate::load_analytics_agent(platform.as_ref(), agent_id, ctx_root.path())
        .await
        .map(|_| ())
}

/// Which chat starts go on the queue, from what the request asked for.
///
/// A plain analytics chat does, started by a request that resolved who asked.
/// Everything else is built and driven in the request's process:
///
/// * **the Builder domain**, and **onboarding** (its own prompt, tool
///   allowlist, LLM client and auto-accept): they edit the working copy;
/// * **a workspace preview's** platform, which holds every write in that
///   process — a driver elsewhere has no such hold;
/// * **a start on a branch** (`?branch=`): its context is built on that
///   branch's working copy and revision, and a driver rebuilds the promoted
///   one, so the agent checked at the start is not the agent that would run;
/// * **no requester**: the only other context a driver has is its own, with no
///   subject.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatAdmission {
    pub builder_domain: bool,
    pub onboarding: bool,
    pub auto_accept: bool,
    pub working_copy_branch: Option<String>,
}

impl ChatAdmission {
    /// The requester to run the chat as, when this start goes on the queue.
    pub fn queueable(&self, platform: &dyn PlatformContext) -> Option<crate::platform::Requester> {
        if self.builder_domain
            || self.onboarding
            || self.auto_accept
            || self.working_copy_branch.is_some()
            || platform.is_workspace_preview()
        {
            return None;
        }
        platform.requester()
    }
}

/// Which agentic-workflow run starts go on the queue: one started for a
/// requester, not on a workspace preview's platform (it holds every write in
/// its own process) and not on a `?branch=` (its context is that branch's, and
/// a driver would rebuild the promoted one, so the file the request named is
/// not the file that would run). The rest keep their co-located coordinator.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutomationAdmission {
    pub working_copy_branch: Option<String>,
}

impl AutomationAdmission {
    /// The requester to run the workflow as, when this start goes on the queue.
    pub fn queueable(&self, platform: &dyn PlatformContext) -> Option<crate::platform::Requester> {
        if self.working_copy_branch.is_some() || platform.is_workspace_preview() {
            return None;
        }
        platform.requester()
    }
}

/// Was `run_id` put on the Global queue — its root entry is not owned by a
/// scoped coordinator in the process that started it? True for a run a queued
/// start seeded and for one whose answer was already queued
/// ([`crate::queued_answer`]), whatever the entry's status now (a parked root's
/// claim is `completed`). False for a run driven in its request's process, and
/// for one with no entry.
pub async fn started_on_the_queue(db: &DatabaseConnection, run_id: &str) -> Result<bool, DbErr> {
    Ok(crud::get_queue_entry(db, run_id)
        .await?
        .is_some_and(|entry| !entry.scope_owned))
}

/// Is `run_id` held by the queue — a `Global` entry that is `queued` or
/// `claimed`, so a driver anywhere has it or is about to take it? Such a run is
/// that driver's to close on a cancel: it turns the durable flag into a proper
/// `cancelled` within about 5 s. A scoped entry belongs to a coordinator in the
/// process that started it, which answers for it there.
pub async fn held_by_the_queue(db: &DatabaseConnection, run_id: &str) -> Result<bool, DbErr> {
    Ok(crud::get_queue_entry(db, run_id)
        .await?
        .is_some_and(|entry| {
            !entry.scope_owned && matches!(entry.queue_status.as_str(), "queued" | "claimed")
        }))
}

/// Why [`queue_chat`] started nothing.
#[derive(Debug, thiserror::Error)]
pub enum QueueChatError {
    /// The agent cannot start: what `PipelineBuilder::start` would have said.
    #[error(transparent)]
    Agent(PipelineError),
    #[error("could not queue the chat: {0}")]
    Seed(DbErr),
}

/// Queue a chat: check its agent can start ([`check_analytics_agent`]), then
/// seed the run with its requester, its extension row and its `queued` Global
/// [`CHAT_RUN_KIND`] entry in one transaction. Returns the run id. Registers
/// nothing anywhere — see the module docs.
pub async fn queue_chat(
    db: &DatabaseConnection,
    platform: &Arc<dyn PlatformContext>,
    chat: ChatStart,
    requester: crate::platform::Requester,
) -> Result<String, QueueChatError> {
    check_analytics_agent(platform, &chat.agent_id)
        .await
        .map_err(QueueChatError::Agent)?;
    let run = QueuedAnalyticsRun {
        run_id: Uuid::new_v4().to_string(),
        workspace_id: chat.workspace_id,
        agent_id: chat.agent_id,
        question: chat.question,
        thread_id: chat.thread_id,
        thinking_mode: chat.thinking_mode,
        metadata: [(
            crate::platform::Requester::KEY.to_string(),
            requester.to_metadata(),
        )]
        .into_iter()
        .collect(),
        spec: chat_run_spec(),
    };
    seed_queued_analytics_run(db, &run)
        .await
        .map_err(QueueChatError::Seed)?;
    Ok(run.run_id)
}

/// `TaskSpec::Custom` discriminator of a queued chat run (`/analytics`).
///
/// Not the run's `source_type`: a chat run is `analytics`, which is what its
/// stream, its thread history and a resume key on, and what the placement gate
/// (`drive_policy_for`) matches.
pub const CHAT_RUN_KIND: &str = "analytics_chat_run";

/// The spec a queued chat run is enqueued with. The payload is empty: the run
/// row is the payload ([`ChatStart::from_run`]).
pub fn chat_run_spec() -> TaskSpec {
    TaskSpec::Custom {
        kind: CHAT_RUN_KIND.to_string(),
        payload: serde_json::json!({}),
    }
}

/// What a chat run's pipeline is built from — one definition for the request
/// that starts one and the driver that claims one, so a queued chat is driven
/// the way the handler drives it: on its thread (a follow-up sees the earlier
/// turns), at its thinking mode, and with no human-input provider over the
/// default, so the agent can still stop to ask its user a question.
///
/// Only a plain chat is described here. Onboarding (its own prompt, tool
/// allowlist, LLM client and auto-accept) and the Builder domain edit the
/// working copy, and stay with the request that starts them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatStart {
    pub workspace_id: Uuid,
    pub agent_id: String,
    pub question: String,
    pub thread_id: Option<Uuid>,
    pub thinking_mode: ThinkingMode,
}

impl ChatStart {
    /// The chat a run row describes, as [`seed_queued_analytics_run`] wrote it.
    pub fn from_run(run: &agentic_runtime::entity::run::Model) -> Result<Self, String> {
        let metadata = run.metadata.as_ref();
        let agent_id = metadata
            .and_then(|m| m.get("agent_id"))
            .and_then(|v| v.as_str())
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "the run names no agent".to_string())?;
        let thinking_mode = match metadata
            .and_then(|m| m.get("thinking_mode"))
            .and_then(|v| v.as_str())
        {
            Some("extended_thinking") => ThinkingMode::ExtendedThinking,
            _ => ThinkingMode::Auto,
        };
        Ok(Self {
            workspace_id: run.workspace_id,
            agent_id: agent_id.to_string(),
            question: run.question.clone(),
            thread_id: run.thread_id,
            thinking_mode,
        })
    }

    /// The builder for this chat. The caller adds what differs between the two
    /// drives: the request names no run (the insert is the start's) and adds
    /// its process's builder runners; a driver adds `existing_run`.
    pub fn builder(
        &self,
        platform: Arc<dyn PlatformContext>,
        schema_cache: Option<
            Arc<std::sync::Mutex<std::collections::HashMap<String, crate::AnalyticsSchemaCatalog>>>,
        >,
    ) -> crate::PipelineBuilder {
        let mut builder = crate::PipelineBuilder::new(platform)
            .workspace_id(self.workspace_id)
            .question(&self.question)
            .thinking_mode(self.thinking_mode);
        if let Some(cache) = schema_cache {
            builder = builder.schema_cache(cache);
        }
        if let Some(thread_id) = self.thread_id {
            builder = builder.thread(thread_id);
        }
        builder.analytics(&self.agent_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(metadata: serde_json::Value) -> agentic_runtime::entity::run::Model {
        let now = agentic_runtime::crud::now();
        agentic_runtime::entity::run::Model {
            id: "run-1".into(),
            question: "how did the stores do?".into(),
            answer: None,
            error_message: None,
            thread_id: Some(Uuid::new_v4()),
            source_type: Some("analytics".into()),
            metadata: Some(metadata),
            parent_run_id: None,
            schedule_id: None,
            task_status: Some("running".into()),
            task_metadata: None,
            attempt: 0,
            recovery_requested_at: None,
            driver_id: None,
            driver_heartbeat_at: None,
            cancel_requested_at: None,
            workspace_id: Uuid::new_v4(),
            created_at: now,
            updated_at: now,
        }
    }

    /// What the seed writes is what the driver reads back: the agent, the
    /// question, the thread, the workspace and the thinking mode.
    #[test]
    fn a_seeded_chat_reads_back_as_it_was_started() {
        for mode in [ThinkingMode::Auto, ThinkingMode::ExtendedThinking] {
            let row = run(serde_json::json!({
                "agent_id": "sales",
                "thinking_mode": mode.to_db(),
            }));
            let chat = ChatStart::from_run(&row).expect("readable");
            assert_eq!(
                chat,
                ChatStart {
                    workspace_id: row.workspace_id,
                    agent_id: "sales".into(),
                    question: row.question.clone(),
                    thread_id: row.thread_id,
                    thinking_mode: mode,
                }
            );
        }
    }

    #[test]
    fn a_run_that_names_no_agent_is_not_a_chat() {
        for agent in [
            serde_json::json!(null),
            serde_json::json!(""),
            serde_json::json!(3),
        ] {
            let err =
                ChatStart::from_run(&run(serde_json::json!({ "agent_id": agent }))).unwrap_err();
            assert!(err.contains("no agent"), "{err}");
        }
    }

    #[test]
    fn a_requester_record_reads_back_as_written_and_a_broken_one_is_an_error() {
        let written = crate::platform::Requester {
            user_id: Uuid::new_v4(),
            role: Some("member".into()),
        };
        let metadata =
            serde_json::json!({ crate::platform::Requester::KEY: written.to_metadata() });
        assert_eq!(
            crate::platform::Requester::recorded(Some(&metadata)),
            Ok(Some(written))
        );
        assert_eq!(
            crate::platform::Requester::recorded(Some(&serde_json::json!({}))),
            Ok(None)
        );
        let broken = serde_json::json!({ crate::platform::Requester::KEY: { "user_id": "x" } });
        assert!(crate::platform::Requester::recorded(Some(&broken)).is_err());
    }
}
