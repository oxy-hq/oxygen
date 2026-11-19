//! Running another app's function for `ctx.apps.call` (`host/apps_op.rs`).
//!
//! A call runs the callee exactly as a queued run does: the callee's build in
//! the caller's environment, its own `ctx.env` secrets, capabilities and OLTP
//! schema, in a
//! fresh isolate from the warm pool on its own thread. What differs is that it
//! is in process, not queued; that it is admitted by the callee's `callable`
//! mark (`queued_admission`, trigger `call`); that its timeout is the smallest
//! of what the caller asked, the callee function's own and what the caller has
//! left; and that its invocation row has mode `call` and names the caller's
//! invocation as its parent. It runs as the chain's person when there is one
//! (their credential's ceiling included, after `Action::AppCall` allowed it
//! at the host), else as the system. Its output never enters Oxy's Postgres.

use std::time::Duration;

use entity::apps;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_custom_apps::calls::{CallErrorKind, callee_budget};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::pipeline::{self, PrepareError};
use super::queued_admission::{QueuedRefusal, QueuedTrigger};
use super::{QueuedBy, QueuedReport, QueuedRun, runtime};

/// Where a call puts the callee in its chain: the callee's position (its
/// caller's next depth), the chain's root and the person behind it.
#[derive(Debug, Clone)]
pub(crate) struct ChainStart {
    pub position: i32,
    pub root: Uuid,
    pub actor: Option<String>,
    pub budget: std::sync::Arc<super::limits::ChainBudget>,
    /// The (app, function) pairs above the callee on its synchronous chain,
    /// its caller last: what a cycle is detected against.
    pub path: Vec<(String, String)>,
}

/// One call, from the caller's side.
#[derive(Debug, Clone)]
pub(crate) struct CallRun {
    /// What the callee's `ctx.caller` reports.
    pub caller: runtime::CtxCaller,
    /// The callee's depth: its caller's next depth.
    pub position: i32,
    /// The first event or invocation of the chain.
    pub root: Uuid,
    /// `timeoutMs`, when the caller gave one.
    pub asked: Option<Duration>,
    /// What the caller has left of its own budget.
    pub remaining: Duration,
    /// The calling invocation: the callee's `parent_invocation_id`.
    pub parent: Uuid,
    /// The person the chain runs for, when one is: the callee runs as them.
    /// `None` runs it as the system.
    pub person: Option<super::host::apps_op::Person>,
    /// The chain's shared budget, which the callee's own calls draw on too.
    pub chain_budget: std::sync::Arc<super::limits::ChainBudget>,
    /// The synchronous chain down to and including the caller.
    pub path: Vec<(String, String)>,
    /// What the callee may spend ([`callee_budget`]); set once the callee's own
    /// timeout is known.
    pub budget: Duration,
}

impl CallRun {
    pub(super) fn chain(&self) -> ChainStart {
        ChainStart {
            position: self.position,
            root: self.root,
            actor: self.caller.actor.clone(),
            budget: self.chain_budget.clone(),
            path: self.path.clone(),
        }
    }
}

/// A call that did not return the callee's answer, before the callee ran or
/// while it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallFailure {
    pub kind: CallErrorKind,
    pub message: String,
}

impl CallFailure {
    fn of(kind: CallErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// The failure a refusal to run the callee is, for the caller.
fn failure_of(e: PrepareError, app: &apps::Model, function: &str) -> CallFailure {
    let target = format!("{}/{function}", app.slug);
    match e {
        PrepareError::NoBuild(environment) => CallFailure::of(
            CallErrorKind::NotDeployed,
            format!("{} has no build in the {environment} environment", app.slug),
        ),
        PrepareError::FunctionNotFound { .. } => CallFailure::of(
            CallErrorKind::NotFound,
            format!("{target}: no such function in the live build"),
        ),
        PrepareError::QueuedRefused(refused) => {
            let kind = match refused.why {
                QueuedRefusal::NotCallable => CallErrorKind::NotCallable,
                QueuedRefusal::Expired => CallErrorKind::NotDeployed,
                _ => CallErrorKind::Refused,
            };
            CallFailure::of(kind, format!("{target}: {}", refused.message()))
        }
        PrepareError::Refused(refused) => CallFailure::of(
            CallErrorKind::Refused,
            format!("{target}: {}", refused.message()),
        ),
        other => CallFailure::of(
            CallErrorKind::Failed,
            format!("{target}: {}", other.into_message(function)),
        ),
    }
}

/// Run `function` of `app` in `environment` for `call`, with `input` as the
/// request body, and report how it ended. `cancel` ends the callee; it is the
/// caller's own, so a caller that stops takes its calls with it.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_call(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    function: &str,
    mut call: CallRun,
    input: Vec<u8>,
    cancel: tokio_util::sync::CancellationToken,
    query_exec: std::sync::Arc<dyn super::seam::FunctionQueryExecutor>,
    preagg: oxy_app_core::preagg_cache_ctx::PreaggCacheCtx,
) -> Result<QueuedReport, CallFailure> {
    let door = pipeline::Door::Queued {
        trigger: QueuedTrigger::Call,
    };
    let admitted = pipeline::admit(db, app, environment, function, door)
        .await
        .map_err(|e| failure_of(e, app, function))?;
    let own = super::resolve_timeout(&admitted.manifest);
    call.budget = callee_budget(call.asked, own, call.remaining).ok_or_else(|| {
        CallFailure::of(
            CallErrorKind::Timeout,
            "the caller has no time left to wait for an answer",
        )
    })?;
    let admission = admitted.admission.clone();
    let prepared = pipeline::prepare(db, app, admission, admitted.build_id, function)
        .await
        .map_err(|e| failure_of(e, app, function))?;
    let owner_id = super::org_owner(db, app.org_id)
        .await
        .map_err(|e| CallFailure::of(CallErrorKind::Failed, e))?;
    let queued = QueuedBy {
        mode: "call",
        trigger: QueuedTrigger::Call,
        credential_token_id: None,
    };
    let invocation_id = super::insert_queued_invocation(
        db,
        app,
        &admitted,
        function,
        &queued,
        super::QueuedFor {
            parent: Some(call.parent),
            user_id: call.person.as_ref().map(|p| p.user_id),
        },
    )
    .await;
    let run = QueuedRun {
        db,
        app,
        admitted: &admitted,
        function_name: function,
        queued: &queued,
        owner_id,
        invocation_id,
        cancel,
        query_exec,
        preagg,
        event_run: None,
        call: Some(call),
    };
    Ok(super::run_queued(run, prepared, input, None).await)
}
