//! A workspace's model for one completion, with no tools and no streaming:
//! [`prepare_one_shot`] resolves it once, [`OneShotCompleter`] runs it. A
//! host crate that may not import `agentic-llm` reaches a workspace's model
//! this way (`oxy-app`'s eval judge, `oxy-api-custom-apps`' advisory notes on
//! a build's checks); a user message that carries pictures is [`images`]'.

mod images;

pub use images::{ImageInput, ImageLimits, ImageMediaType, ImageRequest, OneShotImageError};

use agentic_llm::LlmClient;

use crate::platform::ProjectContext;

/// A prepared one-shot completer: a resolved workspace model with its client
/// already built, reusable across many `system` + `user` completions (no tools,
/// no streaming). Resolve once via [`prepare_one_shot`], then `complete` per
/// call — cloning is cheap (the inner [`LlmClient`] is `Arc`-backed).
///
/// A user message that carries pictures goes through
/// [`OneShotCompleter::complete_with_images`] ([`images`]).
#[derive(Clone)]
pub struct OneShotCompleter {
    client: LlmClient,
    model: String,
}

impl OneShotCompleter {
    /// The resolved model name (e.g. `"claude-opus-4-8"`), for reporting
    /// which model judged.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Run a single completion. Returns the model's text, or a message
    /// describing why the call failed.
    pub async fn complete(&self, system: &str, user: &str) -> Result<String, String> {
        self.client
            .complete(system, user)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Resolve a workspace model (explicit `model_ref`, else the project default)
/// and build a reusable [`OneShotCompleter`]. This is how hosts (e.g. `oxy-app`'s
/// eval judge) reach the agentic LLM stack for a non-streaming completion instead
/// of building a client themselves; `agent_name` labels the genai/tracing spans.
///
/// An explicit-but-unknown model is an error, not a silent fallback to the
/// default — matching the pre-migration judge, so a misconfigured `judge_model`
/// (a typo, or an as-yet-unsupported vendor) fails loud rather than scoring on a
/// different model.
pub async fn prepare_one_shot(
    ctx: &dyn ProjectContext,
    model_ref: Option<&str>,
    agent_name: &str,
) -> Result<OneShotCompleter, String> {
    let info = match model_ref {
        Some(name) => ctx.resolve_model(Some(name), false).await.ok_or_else(|| {
            format!("LLM model '{name}' could not be resolved from project config")
        })?,
        None => ctx
            .resolve_model(None, false)
            .await
            .ok_or_else(|| "no default LLM model configured".to_string())?,
    };
    let genai = agentic_llm::GenAiContext {
        agent_name: Some(agent_name.to_string()),
        workspace_id: Some(ctx.workspace_id())
            .filter(|id| !id.is_nil())
            .map(|id| id.to_string()),
        ..Default::default()
    };
    Ok(OneShotCompleter {
        client: crate::platform::build_llm_client(&info).with_genai_context(genai),
        model: info.model.clone(),
    })
}
