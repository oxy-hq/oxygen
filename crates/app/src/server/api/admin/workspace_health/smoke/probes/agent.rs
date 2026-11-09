//! Agent probe: ask an agentic agent a fixed question and assert it answers.
//!
//! This is the widest probe and the only one that spends tokens — it exercises
//! the LLM key, the FSM pipeline, tool dispatch, and SQL generation in one shot.
//! Any non-error answer passes; the probe does not judge correctness, only that
//! the round-trip completes. A suspended run (the agent asked a clarifying
//! question) surfaces as an error string from the pipeline, which is the right
//! outcome: a smoke prompt that can't be answered outright is a broken probe.
//!
//! The agent is named by its workspace ref and read the way Ask reads it: from
//! the compiled revision, falling back to the working copy only on a node that
//! has one. Health evals run on serve and worker pods, which hold no checkout,
//! so resolving the ref to a file here failed every probe on the fleet.

use std::sync::Arc;

use agentic_pipeline::HeadlessError;
use agentic_pipeline::platform::PlatformContext;

use super::ProbeFailure;
use crate::agentic_wiring::project_ctx::OxyProjectContext;

pub(crate) async fn ask(
    ctx: &Arc<OxyProjectContext>,
    agent_ref: &str,
    prompt: &str,
) -> Result<(), ProbeFailure> {
    let platform: Arc<dyn PlatformContext> = ctx.clone();
    agentic_pipeline::run_agentic_eval(platform, agent_ref, prompt.to_string())
        .await
        .map(|_| ())
        .map_err(|e| failure(agent_ref, e))
}

/// An agent that could not be read is a probe we could not run (Degraded); an
/// agent that loaded and then failed — no LLM key, a bad query — is broken.
fn failure(agent_ref: &str, e: HeadlessError) -> ProbeFailure {
    match e {
        HeadlessError::Load(e) => {
            ProbeFailure::Unavailable(format!("agent '{agent_ref}' could not be loaded: {e}"))
        }
        HeadlessError::Run(e) => ProbeFailure::Broken(format!("agent '{agent_ref}' failed: {e}")),
    }
}
