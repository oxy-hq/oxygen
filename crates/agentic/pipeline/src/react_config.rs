//! What a react run does with the `.agentic.yml` keys the FSM reads: the keys
//! it cannot honor are refused before any connector or model work, and the
//! data-freshness hint it can honor is computed here.

use std::collections::HashMap;
use std::sync::Arc;

use agentic_analytics::SemanticCatalog;
use agentic_analytics::compute_freshness_hint;
use agentic_analytics::config::{
    AgentConfig, BuildContext, ConfigError, LlmConfigYaml, LlmVendor, StateConfig,
};
use agentic_connector::DatabaseConnector;
use agentic_llm::engine::EngineConfig;

/// Refuses an agent that sets a key the react runtime ignores, so a config
/// that relied on it never runs with it silently missing. Keys are checked
/// in a fixed order (state names sorted) so the refusal names the same key
/// every time. Every other key is honored; the full list, key by key, is in
/// `internal-docs/agent-engine-runtime.md` ("Config keys under react").
pub(crate) fn refuse_unsupported(
    config: &AgentConfig,
    ctx: &BuildContext,
) -> Result<(), ConfigError> {
    let mut found = unsupported_agent_keys(config, ctx);
    let mut states: Vec<&String> = config.states.keys().collect();
    states.sort_unstable();
    for name in states {
        found.extend(unsupported_state_keys(name, &config.states[name]));
    }
    match found.into_iter().next() {
        Some(key) => Err(ConfigError::ReactUnsupported { key }),
        None => Ok(()),
    }
}

/// The top-level and `llm:` keys react ignores. Both structs are
/// destructured whole, so a new key has to be placed: honored (bound to
/// `_`, with where react reads it) or refused (pushed).
fn unsupported_agent_keys(config: &AgentConfig, ctx: &BuildContext) -> Vec<String> {
    let AgentConfig {
        instructions: _,  // the system prompt (ReactInputs.instructions)
        timezone: _,      // the date hint (effective_timezone)
        databases: _,     // the connectors the host resolves
        context: _,       // search/read files and the semantic catalog
        states: _,        // per state, in unsupported_state_keys
        llm,              // below
        thinking: _,      // solving_thinking
        validation,       // refused: react runs no validation rules
        semantic_engine,  // refused: react routes only through airlayer
        runtime: _,       // what picked react
        semantic_only: _, // the query tool and prompt (ReactInputs.semantic_only)
    } = config;
    let LlmConfigYaml {
        model_ref: _,         // solving_model_info
        vendor: _,            // solving_model_info
        model: _,             // solving_model_info
        api_key: _,           // solving_model_info
        base_url: _,          // solving_model_info
        max_tokens,           // honored on Anthropic only, below
        thinking: _,          // solving_thinking
        extended_thinking: _, // the served toggle (extended_thinking_overrides)
        advisor,              // react_llm_client; Anthropic only, below
    } = llm;
    let mut found = Vec::new();
    if semantic_engine.is_some() {
        found.push("semantic_engine".into());
    }
    if validation.is_some() {
        found.push("validation".into());
    }
    // Only the Anthropic provider applies an output cap; the others would
    // drop it silently, so a set `llm.max_tokens` there is refused.
    let vendor = config.solving_model_info(ctx).vendor;
    if max_tokens.is_some() && !matches!(vendor, LlmVendor::Anthropic) {
        found.push("llm.max_tokens".into());
    }
    // The advisor is Anthropic's server tool; no other provider runs it.
    if advisor.is_some() && !matches!(vendor, LlmVendor::Anthropic) {
        found.push("llm.advisor".into());
    }
    found
}

/// The keys of `states.<name>` react ignores. The react loop is one model
/// client, the FSM's `solving` one, so only `solving`'s `model` and
/// `thinking` reach it; `instructions` and `max_retries` reach it on no
/// state. Destructured whole, so a new state key has to be placed here.
fn unsupported_state_keys(name: &str, state: &StateConfig) -> Vec<String> {
    let StateConfig {
        instructions,
        thinking,
        max_retries,
        model,
    } = state;
    let solving = name == "solving";
    let set = [
        ("instructions", instructions.is_some()),
        ("thinking", thinking.is_some() && !solving),
        ("max_retries", max_retries.is_some()),
        ("model", model.is_some() && !solving),
    ];
    set.into_iter()
        .filter(|(_, refused)| *refused)
        .map(|(key, _)| format!("states.{name}.{key}"))
        .collect()
}

/// The engine's settings: the solving state's thinking, the dynamic hint and
/// `llm.max_tokens` (which [`refuse_unsupported`] let through only for a
/// provider that applies it).
pub(crate) fn engine_config(
    config: &AgentConfig,
    ctx: &BuildContext,
    system_date_hint: String,
) -> EngineConfig {
    EngineConfig {
        thinking: config.solving_thinking(ctx),
        system_date_hint: Some(system_date_hint),
        max_tokens: config.llm.max_tokens,
        ..EngineConfig::default()
    }
}

/// The connector `query` uses when it is given no `database`: the agent's
/// `extra_default_connector`, else the first name in order (the FSM's rule).
pub(crate) fn default_connector(ctx: &BuildContext) -> String {
    ctx.extra_default_connector
        .clone()
        .or_else(|| ctx.extra_connectors.keys().min().cloned())
        .unwrap_or_default()
}

/// The dynamic system suffix: the date hint, and the data-coverage line
/// beside it when the catalog declares freshness views, as the FSM's
/// `system_hint` does. Kept out of the cached prefix.
pub(crate) async fn dynamic_hint(
    date: String,
    workspace_key: &str,
    catalog: Option<&SemanticCatalog>,
    connectors: &HashMap<String, Arc<dyn DatabaseConnector>>,
    default: &str,
) -> String {
    let Some(catalog) = catalog else { return date };
    match compute_freshness_hint(workspace_key, catalog, connectors, default).await {
        Some(freshness) => format!("{date}\n{freshness}"),
        None => date,
    }
}

#[cfg(test)]
#[path = "react_config_tests.rs"]
pub(crate) mod tests;
