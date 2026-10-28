//! `ChatRunExecutor`: drives a queued chat run
//! (`TaskSpec::Custom { kind: "analytics_chat_run" }`) on whichever process
//! claimed it. Registered into the `CustomTaskRegistry` by
//! `server::router::recovery`; `PipelineTaskExecutor` delegates the kind here.
//!
//! Every plain chat reaches it: `agentic-http`'s `create_run` queues the kind
//! (`queue_chat`). It was registered a release ahead of that start, so no
//! driver is left that fails the kind as unknown — the order the custom-app
//! ask took (`internal-docs/worker-fleet.md` § "Chat runs").
//!
//! It is the handler's drive by construction where it can be, and says where
//! it cannot:
//!
//! * **The pipeline is the handler's.** `agentic_pipeline::queued_run::ChatStart`
//!   is the one description both build from: the thread travels, the thinking
//!   mode travels, and no human-input provider is installed over the default,
//!   so the agent can still stop to ask its user a question. (`TaskSpec::Agent`,
//!   what the scheduler seeds, does neither.) Only a plain chat is queued:
//!   onboarding and the Builder domain edit the working copy and stay with the
//!   request that starts them.
//! * **The identity is the request's.** The run records its requester
//!   ([`super::requester`]); the context is built for that user at that role.
//!   A run with no record is failed, never driven as the driver.
//! * **No Builder bridges of its own.** The run's pipeline is built without
//!   them, as an ask's is. Nothing in an analytics run reaches for them today:
//!   its delegation to the Builder is switched off in the solver
//!   (`clarifying::BUILDER_DELEGATION_ENABLED = false`), and Phase 3b removes
//!   it.
//! * **Its pipeline starts at most once**, through [`super::attempt`]: a
//!   re-claimed run resumes from its suspension (recovery's), closes as
//!   interrupted, or steps aside for the attempt still beating.

use std::sync::Arc;

use agentic_core::delegation::{TaskAssignment, TaskSpec};
use agentic_pipeline::platform::Requester;
use agentic_pipeline::queued_run::{CHAT_RUN_KIND, ChatStart};
use agentic_runtime::entity::run;
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use tracing::Instrument;

use super::requester::{RequesterContextError, requester_context};
use super::{Settled, admission, attempt, beside_the_pipeline, heartbeat};
use crate::agentic_wiring::OxyProjectContext;
use crate::server::api::projects::agent_ask::executor::SchemaCache;

pub struct ChatRunExecutor {
    pub db: DatabaseConnection,
    /// The driver's shared schema cache, as the handler passes its process's.
    pub schema_cache: Option<SchemaCache>,
}

/// Refuse any spec that is not a queued chat, rather than guess at it.
pub fn accept(spec: &TaskSpec) -> Result<(), String> {
    match spec {
        TaskSpec::Custom { kind, .. } if kind == CHAT_RUN_KIND => Ok(()),
        TaskSpec::Custom { kind, .. } => Err(format!("unknown chat-run kind: {kind}")),
        other => Err(format!("unexpected spec for a chat run: {other:?}")),
    }
}

#[async_trait]
impl TaskExecutor for ChatRunExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        accept(&assignment.spec)?;
        let span = tracing::info_span!("analytics_chat_run.job", run_id = %assignment.run_id);
        let run_id = assignment.run_id.as_str();
        Ok(attempt(
            &self.db,
            run_id,
            |run| async move {
                prepare(&self.db, &run)
                    .await
                    .map_err(|e| unprepared(run_id, e))
            },
            |prepared, beating| self.start(prepared, beating),
        )
        .instrument(span)
        .await)
    }
}

impl ChatRunExecutor {
    async fn start(&self, prepared: PreparedChat, beating: heartbeat::Beating) -> ExecutingTask {
        let PreparedChat {
            run_id,
            chat,
            context,
        } = prepared;
        let builder = chat
            .builder(Arc::new(context), self.schema_cache.clone())
            .existing_run(run_id.clone());
        // Boxed: the start's future is a large one, and this runs on a driver
        // task's stack beside the claim loop's.
        match Box::pin(builder.start(&self.db)).await {
            Ok(started) => beside_the_pipeline(started, beating),
            Err(e) => {
                beating.stop().await;
                tracing::warn!(%run_id, error = %e, "chat run: pipeline start failed");
                Settled::failed(e.to_string()).into_task()
            }
        }
    }
}

/// Everything a chat run needs once its identity has been re-established.
pub struct PreparedChat {
    pub run_id: String,
    pub chat: ChatStart,
    /// Built for the requester: their id as subject, at their recorded role.
    pub context: OxyProjectContext,
}

/// Why a claimed chat run could not be prepared.
#[derive(Debug, thiserror::Error)]
pub enum PrepareError {
    /// The row does not describe a chat anyone may drive: it names no agent,
    /// or records no requester (or one that does not read back).
    #[error("{0}")]
    NotAChat(String),
    #[error(transparent)]
    Context(#[from] RequesterContextError),
}

/// Rebuild, on the driver, what the handler had when it started the chat.
pub async fn prepare(
    db: &DatabaseConnection,
    run: &run::Model,
) -> Result<PreparedChat, PrepareError> {
    let chat = ChatStart::from_run(run).map_err(PrepareError::NotAChat)?;
    let requester = Requester::recorded(run.metadata.as_ref())
        .map_err(PrepareError::NotAChat)?
        .ok_or_else(|| {
            PrepareError::NotAChat(
                "the run records no requester, so there is nobody to run it as".to_string(),
            )
        })?;
    let context = requester_context(db, run.workspace_id, &requester).await?;
    Ok(PreparedChat {
        run_id: run.id.clone(),
        chat,
        context,
    })
}

/// The report of an attempt whose chat could not be prepared. Nothing has
/// begun, so a failure that may pass is handed back to be tried again; one
/// that will not ends the run, on its log.
fn unprepared(run_id: &str, e: PrepareError) -> Settled {
    match &e {
        PrepareError::Context(ctx) if ctx.is_transient() => admission::step_aside(
            run_id,
            format!("the chat run could not be prepared yet: {e}"),
        ),
        _ => Settled::failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use agentic_core::delegation::TaskOutcome;

    use super::*;

    #[test]
    fn only_a_queued_chat_is_accepted() {
        assert!(accept(&agentic_pipeline::queued_run::chat_run_spec()).is_ok());
        let ask = TaskSpec::Custom {
            kind: "custom_app_agent_ask".into(),
            payload: serde_json::json!({}),
        };
        assert!(accept(&ask).unwrap_err().contains("unknown chat-run kind"));
        let agent = TaskSpec::Agent {
            agent_id: "sales".into(),
            question: "q".into(),
            extra: None,
        };
        assert!(accept(&agent).unwrap_err().contains("unexpected spec"));
    }

    /// Nothing has begun, so a context that may build a minute later is tried
    /// again; everything else ends the run on its log.
    #[test]
    fn only_a_failure_that_may_pass_is_retried() {
        for transient in [
            RequesterContextError::Lookup("connection reset".into()),
            RequesterContextError::NotCompiled,
        ] {
            let settled = unprepared("run-1", PrepareError::Context(transient));
            assert!(settled.event.is_none());
            assert!(matches!(settled.outcome, TaskOutcome::Deferred { .. }));
        }
        for permanent in [
            PrepareError::NotAChat("the run records no requester".into()),
            PrepareError::Context(RequesterContextError::WorkspaceGone),
            PrepareError::Context(RequesterContextError::NoPath),
            PrepareError::Context(RequesterContextError::Build),
            PrepareError::Context(RequesterContextError::Role("root".into())),
        ] {
            let reason = permanent.to_string();
            let settled = unprepared("run-1", permanent);
            let (event_type, _) = settled.event.expect("on the log");
            assert_eq!(event_type, "error", "{reason}");
            assert!(matches!(&settled.outcome, TaskOutcome::Failed(m) if *m == reason));
        }
    }
}
