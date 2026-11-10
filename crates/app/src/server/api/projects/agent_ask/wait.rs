//! `wait_seconds` on an ask's start: hold the response until the ask settles,
//! for a caller that wants the answer and not a stream.
//!
//! **A way to deliver the result, not a way to run.** The ask starts exactly
//! as it does without a wait — queued for any driver on the fleet, or driven
//! here on a staging host — and this only watches its row. A response that
//! runs out of wait is the ordinary `202`, and the caller reads the rest from
//! the stream (`GET …/agents/runs/{run_id}/events`) as before; the run never
//! depends on the request that is waiting for it.
//!
//! **Settled** is a terminal status, or `awaiting_input`: a run parked on a
//! clarifying question waits for a person, with no timeout, so holding the
//! request for it would only run the wait out. The question comes back
//! instead, and the caller answers it the way `useAgentRun` does: a new ask on
//! the same thread.
//!
//! **How it notices.** Every event append NOTIFYs `oxy_run_events`, which the
//! router's LISTEN connection turns into a ring on
//! [`RemoteWakes`](agentic_runtime::lifecycle::remote_wake::RemoteWakes),
//! from whichever pod drives the run, this one included. A ring re-reads the
//! row. The row is the authority, and its terminal update can land just after
//! the run's last event, so a quiet [`RECHECK`] re-reads it too: at most one
//! primary-key read a second for the length of the wait.

use std::sync::Arc;
use std::time::Duration;

use agentic_runtime::entity::run_suspension;
use agentic_runtime::lifecycle::remote_wake::RemoteWakes;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait};
use serde::Serialize;
use tokio::time::Instant;

/// The longest wait a start accepts. The router answers `408` at 60 s
/// (`finalize_router`), and a start spends some of that before it waits.
pub const MAX_WAIT_SECONDS: u32 = 45;

/// How long a wait goes without a NOTIFY before it re-reads the row anyway.
const RECHECK: Duration = Duration::from_secs(1);

/// How an ask settled within its wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    Done {
        answer: String,
    },
    /// `failed`, or `timed_out` when `timed_out`.
    Failed {
        timed_out: bool,
        error: String,
    },
    Cancelled,
    /// Parked on a clarifying question. Answer it with a new ask on the same
    /// thread.
    AwaitingInput {
        question: String,
        suggestions: Vec<String>,
    },
}

/// What a start that waited adds to its response: the run's own status name,
/// so a caller branches on the same words the stream and the run row use, and
/// whatever that status carries. `running` when the wait ran out first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AskOutcome {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestions: Option<Vec<String>>,
}

impl AskOutcome {
    pub fn of(settled: Option<Settled>) -> Self {
        let bare = |status| AskOutcome {
            status,
            answer: None,
            error: None,
            question: None,
            suggestions: None,
        };
        match settled {
            None => bare("running"),
            Some(Settled::Done { answer }) => AskOutcome {
                answer: Some(answer),
                ..bare("done")
            },
            Some(Settled::Failed { timed_out, error }) => AskOutcome {
                error: Some(error),
                ..bare(if timed_out { "timed_out" } else { "failed" })
            },
            Some(Settled::Cancelled) => bare("cancelled"),
            Some(Settled::AwaitingInput {
                question,
                suggestions,
            }) => AskOutcome {
                question: Some(question),
                suggestions: Some(suggestions),
                ..bare("awaiting_input")
            },
        }
    }

    /// Settled within the wait: the response is the answer, not a receipt.
    pub fn settled(&self) -> bool {
        self.status != "running"
    }
}

/// The outcome a start reports after waiting `wait_seconds` for `run_id`;
/// `None` when it did not wait. A failed read ends the wait early as
/// `running`: the ask is started either way, and the stream has everything.
pub(super) async fn outcome(
    state: &agentic_http::AgenticState,
    run_id: &str,
    wait_seconds: u32,
) -> Option<AskOutcome> {
    if wait_seconds == 0 {
        return None;
    }
    let wait = Duration::from_secs(wait_seconds.into());
    match wait_for_settle(&state.db, &state.runtime.remote_wakes, run_id, wait).await {
        Ok(settled) => Some(AskOutcome::of(settled)),
        Err(e) => {
            tracing::warn!(%run_id, error = %e, "custom-app ask: wait read failed");
            Some(AskOutcome::of(None))
        }
    }
}

/// Wait up to `wait` for run `run_id` to settle. `None` when it is still
/// going at the deadline.
pub async fn wait_for_settle(
    db: &DatabaseConnection,
    wakes: &Arc<RemoteWakes>,
    run_id: &str,
    wait: Duration,
) -> Result<Option<Settled>, DbErr> {
    // Subscribed before the first read, so an append between the read and the
    // park is a stored permit, not a missed ring.
    let wake = wakes.subscribe(run_id);
    let deadline = Instant::now() + wait;
    loop {
        if let Some(settled) = read_settled(db, run_id).await? {
            return Ok(Some(settled));
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        let recheck = (deadline - now).min(RECHECK);
        tokio::select! {
            _ = wake.notified() => {}
            _ = tokio::time::sleep(recheck) => {}
        }
    }
}

/// Read the run once: settled, or `None` while it is still going (or gone).
async fn read_settled(db: &DatabaseConnection, run_id: &str) -> Result<Option<Settled>, DbErr> {
    let Some(run) = agentic_runtime::crud::get_run(db, run_id).await? else {
        return Ok(None);
    };
    let error = || {
        run.error_message
            .clone()
            .unwrap_or_else(|| "The run failed before it could report an error.".into())
    };
    Ok(match run.task_status.as_deref() {
        Some("done") => Some(Settled::Done {
            answer: run.answer.clone().unwrap_or_default(),
        }),
        Some(status @ ("failed" | "timed_out")) => Some(Settled::Failed {
            timed_out: status == "timed_out",
            error: error(),
        }),
        Some("cancelled") => Some(Settled::Cancelled),
        Some("awaiting_input") => awaiting_input(db, run_id).await?,
        _ => None,
    })
}

/// The question a parked run asked. `None` while its suspension row is not
/// written yet: the status can flip first, and the next read finds both.
async fn awaiting_input(db: &DatabaseConnection, run_id: &str) -> Result<Option<Settled>, DbErr> {
    let Some(row) = run_suspension::Entity::find_by_id(run_id.to_string())
        .one(db)
        .await?
    else {
        return Ok(None);
    };
    Ok(Some(Settled::AwaitingInput {
        question: row.prompt,
        suggestions: serde_json::from_value(row.suggestions).unwrap_or_default(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timed_out_run_is_named_as_one_and_carries_its_error() {
        let out = AskOutcome::of(Some(Settled::Failed {
            timed_out: true,
            error: "slow".into(),
        }));
        assert_eq!(
            serde_json::to_value(&out).unwrap(),
            serde_json::json!({ "status": "timed_out", "error": "slow" })
        );
        assert!(out.settled());
    }

    #[test]
    fn a_question_comes_back_with_its_suggestions() {
        let out = AskOutcome::of(Some(Settled::AwaitingInput {
            question: "Which region?".into(),
            suggestions: vec!["EU".into()],
        }));
        assert_eq!(
            serde_json::to_value(&out).unwrap(),
            serde_json::json!({
                "status": "awaiting_input",
                "question": "Which region?",
                "suggestions": ["EU"],
            })
        );
    }

    #[test]
    fn a_wait_that_ran_out_says_running_and_nothing_else() {
        let out = AskOutcome::of(None);
        assert_eq!(
            serde_json::to_value(&out).unwrap(),
            serde_json::json!({ "status": "running" })
        );
        assert!(!out.settled());
    }
}
