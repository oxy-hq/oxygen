//! Which runtime a served analytics start runs on, decided once before the
//! run row is written, and the `runtime` stamp that records it.

use agentic_analytics::config::AgentConfig;
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
    /// The log holds a react attempt: the run continues on react whatever
    /// the flag or the agent's databases say now.
    pub continued: bool,
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
    let react = continued || picks_served(platform, agent_id, can_ask).await;
    let patch = match continued {
        true => Some(react_stamp()),
        false => runtime_patch(react, existing_run),
    };
    if existing_run && let Some(patch) = patch.clone() {
        stamp_existing_run(db, run_id, patch, react).await?;
    }
    Ok(Pick {
        react,
        runtime_patch: patch,
        log,
        continued,
    })
}

/// Whether this served run goes to the react runtime: the org's
/// `react_runtime` flag is on, and the run can put a question to a person
/// (`can_ask`: not a delegation child, not an auto-accept chat, which have no
/// one to answer `ask_user`). The flag is the only switch: an agent's
/// `runtime:` key is read by `oxy test` alone, so turning react off never
/// needs a pull request. An agent react cannot serve falls back to the FSM
/// once its connectors are built ([`serves_react`]).
pub(crate) async fn picks_served(
    platform: &dyn PlatformContext,
    agent_id: &str,
    can_ask: bool,
) -> bool {
    if !platform.react_runtime_enabled().await {
        return false;
    }
    if !can_ask {
        tracing::info!(
            agent_id,
            "react_runtime is on; this run cannot ask a person, so it uses the fsm"
        );
        return false;
    }
    true
}

/// The `runtime` key for this attempt: `react` when it was picked. A row
/// that existed before this start (a re-driven run) is stamped `fsm` when it
/// was not, so the key names the latest attempt's runtime even if an earlier
/// one was stamped `react`. A fresh FSM run carries no key.
pub(crate) fn runtime_patch(picked: bool, existing_run: bool) -> Option<Value> {
    match (picked, existing_run) {
        (true, _) => Some(serde_json::json!({ RUNTIME_KEY: REACT })),
        (false, true) => Some(serde_json::json!({ RUNTIME_KEY: FSM })),
        (false, false) => None,
    }
}

/// What [`serves_react`] checks an agent against: the inputs the react
/// build would get.
pub(crate) struct ServeCheck<'a> {
    pub platform: &'a dyn PlatformContext,
    pub config: &'a AgentConfig,
    pub inputs: &'a super::ServedInputs,
}

/// After the connectors are built: whether a picked run stays on react. A
/// run continuing a react attempt always does. A fresh one falls back to the
/// FSM, with its row re-stamped `fsm` and the reason logged, when the react
/// build would refuse the agent ([`cannot_serve`]). With the flag the only
/// switch, an org's other agents keep answering.
pub(crate) async fn serves_react(
    db: &DatabaseConnection,
    run_id: &str,
    pick: &Pick,
    check: ServeCheck<'_>,
) -> bool {
    if !pick.react || pick.continued {
        return pick.react;
    }
    let Some(why) = cannot_serve(check).await else {
        return true;
    };
    tracing::info!(run_id, reason = %why, "react_runtime is on; this agent runs on the fsm");
    let fsm = serde_json::json!({ RUNTIME_KEY: FSM });
    let _ = stamp_existing_run(db, run_id, fsm, false).await;
    false
}

/// Why the react build would refuse the agent, or `None`: the build's own
/// [`crate::react_run::preflight`], on the context the build would get
/// ([`super::build_context`], with the thinking and model overrides). Not
/// checked: `semantic_only` with no semantic model, a react-only key an FSM
/// agent does not set.
pub(crate) async fn cannot_serve(check: ServeCheck<'_>) -> Option<String> {
    let inputs = super::read_only_inputs(check.platform, check.inputs.clone()).await;
    let ctx = super::build_context(check.platform, check.config, inputs);
    crate::react_run::preflight(check.config, &ctx).await.err()
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
