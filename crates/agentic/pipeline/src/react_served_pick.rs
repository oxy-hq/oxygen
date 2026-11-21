//! Which runtime a served analytics start runs on, decided once before the
//! run row is written, and the `runtime` stamp that records it.

use agentic_analytics::config::{AgentConfig, AgentRuntime};
use sea_orm::DatabaseConnection;
use serde_json::Value;

use super::{FSM, REACT, RUNTIME_KEY};
use crate::PipelineError;
use crate::platform::PlatformContext;
use crate::react_resume::{
    PriorLog, RecordedRuntime, holds_react, prior_log, react_stamp, read_log, runtime_of,
};

/// What [`decide`] reads.
pub(crate) struct PickInput<'a> {
    pub platform: &'a dyn PlatformContext,
    pub db: &'a DatabaseConnection,
    pub config: &'a AgentConfig,
    pub agent_id: &'a str,
    pub run_id: &'a str,
    /// Whether the run can put a question to a person (no injected human
    /// input: not a delegation child, not an auto-accept chat).
    pub can_ask: bool,
    /// Whether the run row exists already (`skip_db_insert`).
    pub existing_run: bool,
    pub recorded: &'a RecordedRuntime,
}

/// What a served start runs on.
pub(crate) struct Pick {
    pub react: bool,
    /// The `runtime` stamp for a row this start inserts
    /// ([`stamp_runtime`]); an existing row already has it merged.
    pub runtime_patch: Option<Value>,
    /// The run's log, read only for a run recorded `react`.
    pub log: Option<PriorLog>,
}

/// Decided before the insert: a react run's row says so from its first
/// moment (`runtime: react`), which is what a resume routes on. A re-driven
/// run whose log holds a react attempt stays on react, whatever the flag or
/// the agent's key now say. On an existing row the stamp is merged here: a
/// `react` stamp that fails fails the start (a resume routes on it); an
/// `fsm` one is logged and the start goes on.
pub(crate) async fn decide(input: PickInput<'_>) -> Result<Pick, PipelineError> {
    let PickInput {
        platform,
        db,
        config,
        agent_id,
        run_id,
        can_ask,
        existing_run,
        recorded,
    } = input;
    let recorded = match existing_run && can_ask {
        true => runtime_of(db, run_id, recorded).await,
        false => None,
    };
    let log = prior_log(recorded.as_deref(), || read_log(db, run_id)).await;
    let continued = log.as_ref().is_some_and(holds_react);
    let react = continued || picks_served(platform, config, agent_id, can_ask).await;
    let patch = match continued {
        true => Some(react_stamp()),
        false => runtime_patch(config, react),
    };
    if existing_run && let Some(patch) = patch.clone() {
        stamp_existing_run(db, run_id, patch, react).await?;
    }
    Ok(Pick {
        react,
        runtime_patch: patch,
        log,
    })
}

/// Whether this served run goes to the react runtime: the agent opts in,
/// the run can put a question to a person (`can_ask`: not a delegation
/// child, not an auto-accept chat, which have no one to answer `ask_user`),
/// and the org's `react_runtime` flag is on. An opt-in that runs on the FSM
/// is logged once per run, so it is visible rather than silent.
pub(crate) async fn picks_served(
    platform: &dyn PlatformContext,
    config: &AgentConfig,
    agent_id: &str,
    can_ask: bool,
) -> bool {
    if config.runtime != AgentRuntime::React {
        return false;
    }
    let why = if !can_ask {
        "the run cannot ask a person a question"
    } else if !platform.react_runtime_enabled().await {
        "the react_runtime flag is off for this org"
    } else {
        return true;
    };
    tracing::info!(
        agent_id,
        reason = why,
        "agent opts into runtime: react; this run uses the fsm"
    );
    false
}

/// The `runtime` key for this attempt of an agent that opts in: `react`
/// when it was picked, else `fsm`, so the key always names the runtime of
/// the latest attempt (a re-driven run whose flag went off since its first
/// attempt is re-stamped). `None` for an agent that never opted in.
pub(crate) fn runtime_patch(config: &AgentConfig, picked: bool) -> Option<Value> {
    if config.runtime != AgentRuntime::React {
        return None;
    }
    let runtime = if picked { REACT } else { FSM };
    Some(serde_json::json!({ RUNTIME_KEY: runtime }))
}

/// Merges `patch` (from [`runtime_patch`]) into run metadata being inserted.
pub(crate) fn stamp_runtime(metadata: &mut Value, patch: &Value) {
    if let (Some(map), Some(keys)) = (metadata.as_object_mut(), patch.as_object()) {
        map.extend(keys.clone());
    }
}

/// Merges `patch` into a run row someone else inserted (the queued chat's
/// handler, a re-driven attempt), keeping its other metadata. A failed
/// `react` stamp is an error; a failed `fsm` stamp only leaves the key of an
/// earlier attempt, so it is logged.
async fn stamp_existing_run(
    db: &DatabaseConnection,
    run_id: &str,
    patch: Value,
    react: bool,
) -> Result<(), PipelineError> {
    let merged = agentic_runtime::crud::merge_run_metadata(db, run_id, patch).await;
    match merged {
        Ok(_) => Ok(()),
        Err(e) if react => Err(e.into()),
        Err(e) => {
            tracing::warn!(run_id, error = %e, "react: runtime: fsm stamp not written");
            Ok(())
        }
    }
}
