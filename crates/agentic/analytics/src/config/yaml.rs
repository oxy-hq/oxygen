//! YAML deserialization types for [`AgentConfig`].
//!
//! [`AgentConfig`]: super::AgentConfig

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::llm::{ReasoningEffort, ThinkingConfig};
use crate::validation::ValidationConfig;

use super::ConfigError;

// ── YAML deserialization types ─────────────────────────────────────────────────

/// Vendor identifier for bundled semantic engine adapters.
///
/// Internal to the config layer — never part of the `SemanticEngine` public API.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VendorKind {
    Cube,
    Looker,
}

/// Configuration for an external vendor semantic engine.
///
/// ```yaml
/// semantic_engine:
///   vendor: cube
///   base_url: https://cube.example.com
///   api_token: "${CUBE_API_TOKEN}"
///
/// # — OR for Looker —
/// semantic_engine:
///   vendor: looker
///   base_url: https://myco.looker.com
///   client_id: "${LOOKER_CLIENT_ID}"
///   client_secret: "${LOOKER_CLIENT_SECRET}"
/// ```
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SemanticEngineConfig {
    pub vendor: VendorKind,
    pub base_url: String,
    /// API token (Cube).  Supports `"${ENV_VAR}"` interpolation.
    #[serde(default)]
    pub api_token: Option<String>,
    /// OAuth client ID (Looker).
    #[serde(default)]
    pub client_id: Option<String>,
    /// OAuth client secret (Looker).
    #[serde(default)]
    pub client_secret: Option<String>,
}

impl SemanticEngineConfig {
    /// Resolve `api_token`, interpolating `${VAR}` from the environment.
    ///
    /// When the field is absent from the YAML (`None`), returns `Ok("")`.
    /// `interpolate_env` only errors when a `${VAR}` placeholder is present
    /// *and* the referenced variable is unset — an absent or empty field is
    /// accepted here and validated downstream by the engine client (e.g. Cube
    /// returns a 401 on first request if the token is wrong).
    pub fn resolved_api_token(&self) -> Result<String, ConfigError> {
        let raw = self.api_token.as_deref().unwrap_or("");
        interpolate_env(raw)
    }

    pub fn resolved_client_id(&self) -> Result<String, ConfigError> {
        let raw = self.client_id.as_deref().unwrap_or("");
        interpolate_env(raw)
    }

    pub fn resolved_client_secret(&self) -> Result<String, ConfigError> {
        let raw = self.client_secret.as_deref().unwrap_or("");
        interpolate_env(raw)
    }
}

/// Interpolate `${VAR_NAME}` placeholders with environment variable values.
///
/// Returns [`ConfigError::MissingEnvVar`] when a referenced variable is absent,
/// so misconfigured deployments fail at startup rather than silently sending
/// empty credentials.
///
/// Expansion is **single-pass** — the substituted value is emitted verbatim and
/// is not rescanned for further `${…}` placeholders. An env var whose value
/// happens to contain `${SOMETHING}` will be treated as a literal, not
/// expanded recursively.
fn interpolate_env(s: &str) -> Result<String, ConfigError> {
    let mut result = String::with_capacity(s.len());
    let mut cursor = 0;
    while let Some(rel_start) = s[cursor..].find("${") {
        let start = cursor + rel_start;
        // No closing `}` — treat the rest as literal and stop.
        let Some(rel_end) = s[start..].find('}') else {
            break;
        };
        let end = start + rel_end;
        let var_name = &s[start + 2..end];
        let value = std::env::var(var_name)
            .map_err(|_| ConfigError::MissingEnvVar(var_name.to_string()))?;
        result.push_str(&s[cursor..start]);
        result.push_str(&value);
        cursor = end + 1;
    }
    result.push_str(&s[cursor..]);
    Ok(result)
}

/// Top-level agent configuration.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AgentConfig {
    /// Global instructions injected into every LLM call.
    #[serde(default)]
    pub instructions: Option<String>,

    /// IANA timezone (e.g. `America/Los_Angeles`) used to resolve the Today-is date hint and relative dates like `yesterday` or `last week`. Defaults to UTC when unset.
    #[serde(default)]
    pub timezone: Option<String>,

    /// Database names from config.yml to use as connectors.
    ///
    /// Each string is the `name:` of a database entry in the project's
    /// `config.yml`.  The HTTP layer resolves these names to live connectors
    /// (via [`BuildContext`]) so that connection details are defined once and
    /// reused across agents.  All database types supported by Oxy (DuckDB,
    /// Postgres, BigQuery, Snowflake, ClickHouse, …) can be listed here.
    ///
    /// ```yaml
    /// databases:
    ///   - local
    ///   - analytics_db
    /// ```
    #[serde(default)]
    pub databases: Vec<String>,

    /// Glob patterns for context files (`.view.yml`, `.topic.yml`, `.sql`,
    /// `.md`).  Resolved relative to the directory containing the YAML file.
    #[serde(default)]
    pub context: Vec<String>,

    /// Per-state overrides.
    #[serde(default)]
    pub states: HashMap<String, StateConfig>,

    #[serde(default)]
    pub llm: LlmConfigYaml,

    /// Global thinking/reasoning config applied to all pipeline states.
    ///
    /// Can be overridden per-state via the `states:` section.
    ///
    /// ```yaml
    /// thinking: adaptive        # shorthand
    /// # — or —
    /// thinking:
    ///   effort: medium          # low | medium | high | xhigh | max
    /// ```
    ///
    /// `{ budget_tokens: N }` is also accepted, but only Claude 4.6 and earlier
    /// take it — 4.7+ rejects that form, and `effort` is the graded control
    /// there. Prefer `effort` unless you need a hard ceiling on an older model.
    #[serde(default)]
    pub thinking: Option<ThinkingConfigYaml>,

    /// Validation rule configuration.
    ///
    /// When absent, the 15 default rules run with their default parameters
    /// ([`ValidationConfig::default_all_rules`]). When present, it **replaces**
    /// that set rather than adjusting it: only the rules it lists, and does not
    /// mark `enabled: false`, run. A stage it lists no rules for is not checked
    /// at all, and an empty section (`validation: {}`) runs no rules. To switch
    /// off one rule or tune its parameters, list every rule that should still
    /// run.
    /// This, for example, runs outlier detection with a tighter threshold and
    /// nothing else:
    ///
    /// ```yaml
    /// validation:
    ///   rules:
    ///     solved:
    ///       - name: outlier_detection
    ///         enabled: true
    ///         threshold_sigma: 3.0
    ///         min_rows: 6
    /// ```
    #[serde(default)]
    pub validation: Option<ValidationConfig>,

    /// Optional vendor semantic engine configuration.
    ///
    /// When absent, the solver routes exclusively through the internal
    /// semantic model and LLM paths (identical to today's behaviour).
    #[serde(default)]
    pub semantic_engine: Option<SemanticEngineConfig>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct StateConfig {
    /// Additional instructions injected for this state only.
    #[serde(default)]
    pub instructions: Option<String>,

    /// Thinking/reasoning config for this state.
    #[serde(default)]
    pub thinking: Option<ThinkingConfigYaml>,

    /// Maximum tool-use rounds before failing.
    #[serde(default)]
    pub max_retries: Option<u32>,

    /// Model ID override for this state.
    ///
    /// When set, this model is used instead of the global `llm.model`.
    /// The vendor, API key, and base URL from the global `llm:` section are
    /// inherited; only the model ID is replaced.
    ///
    /// ```yaml
    /// states:
    ///   clarifying:
    ///     model: claude-haiku-4-5   # cheap model for fast triage
    ///   solving:
    ///     model: claude-opus-4-6    # powerful model for SQL generation
    /// ```
    #[serde(default)]
    pub model: Option<String>,
}

/// Thinking configuration as expressed in YAML.
///
/// Accepts a shorthand string (`"adaptive"`, `"disabled"`, `"effort:<level>"`)
/// or a map with either `budget_tokens` or `effort`.
///
/// Which forms a model accepts is not a preference. On Claude 4.7 and later,
/// `budget_tokens` is rejected by the API and `effort` is the only graded
/// control over thinking depth; on 4.6 and earlier, `budget_tokens` works and
/// is the only way to set a hard ceiling. The provider translates a
/// `budget_tokens` config on a model that refuses it rather than failing the
/// request, and warns.
///
/// Because this enum is `#[serde(untagged)]`, schemars emits a JSON Schema
/// `oneOf` with each variant represented by its structural shape (a plain
/// string for [`ThinkingConfigYaml::Shorthand`] and an object for
/// [`ThinkingConfigYaml::Manual`] / [`ThinkingConfigYaml::Effort`]). Serde
/// discriminates at runtime by trying variants in declaration order, so
/// there is no parse ambiguity. Some IDEs, however, validate each `oneOf`
/// branch independently and may annotate a valid string with
/// "invalid against variant 2 / variant 3" warnings — those are cosmetic
/// and the file still deserializes correctly at runtime.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ThinkingConfigYaml {
    /// Shorthand string: `"adaptive"`, `"disabled"`, or `"effort:<level>"`
    /// for any level below.
    Shorthand(String),
    /// Explicit budget: `budget_tokens: N`.
    ///
    /// Claude 4.6 and earlier only. Claude 4.7+ removed this form; the
    /// Anthropic provider translates it to an effort level there rather than
    /// letting the request 400, and warns. Prefer `effort:` on those models.
    Manual { budget_tokens: u32 },
    /// Reasoning effort: `effort: low|medium|high|xhigh|max`.
    ///
    /// On Claude this is `output_config.effort` alongside adaptive thinking --
    /// the only graded control over thinking depth on 4.7+. On OpenAI the top
    /// two levels clamp to `high`.
    Effort { effort: String },
}

impl ThinkingConfigYaml {
    /// Convert to the runtime [`ThinkingConfig`].
    pub fn to_thinking_config(&self) -> ThinkingConfig {
        match self {
            ThinkingConfigYaml::Shorthand(s) if s.eq_ignore_ascii_case("adaptive") => {
                ThinkingConfig::Adaptive
            }
            // Shorthand "effort:low", "effort::low", etc.
            ThinkingConfigYaml::Shorthand(s) if s.to_ascii_lowercase().starts_with("effort:") => {
                let rest = s["effort:".len()..].trim_start_matches(':');
                parse_effort_level(rest)
            }
            ThinkingConfigYaml::Shorthand(_) => ThinkingConfig::Disabled,
            ThinkingConfigYaml::Manual { budget_tokens } => ThinkingConfig::Manual {
                budget_tokens: *budget_tokens,
            },
            // Map form: `effort: low|medium|high`
            ThinkingConfigYaml::Effort { effort } => parse_effort_level(effort),
        }
    }
}

/// Parse an effort level string into [`ThinkingConfig::Effort`].
/// Defaults to `Medium` for unrecognised values.
///
/// Five levels, because Anthropic accepts five. The OpenAI provider clamps the
/// top two to `high` on its way out.
fn parse_effort_level(s: &str) -> ThinkingConfig {
    let level = match s.trim().to_ascii_lowercase().as_str() {
        "low" => ReasoningEffort::Low,
        "high" => ReasoningEffort::High,
        "xhigh" => ReasoningEffort::XHigh,
        "max" => ReasoningEffort::Max,
        _ => ReasoningEffort::Medium,
    };
    ThinkingConfig::Effort(level)
}

/// LLM vendor / backend selector.
///
/// ```yaml
/// llm:
///   vendor: open_ai_compat   # Ollama, vLLM, LM Studio, LangDock, …
///   model: llama3.2
///   base_url: http://localhost:11434/v1
/// ```
///
/// `rename_all = "snake_case"` derives `open_ai` / `open_ai_compat` from the
/// variant names. The elided spellings (`openai`, `openai_compat`) are the ones
/// people actually reach for — and were what this doc comment advertised while
/// serde rejected them — so both are accepted as aliases.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LlmVendor {
    /// Anthropic Messages API (default).  Uses `ANTHROPIC_API_KEY`.
    #[default]
    Anthropic,
    /// OpenAI Responses API (`/v1/responses`).  Uses `OPENAI_API_KEY`.
    ///
    /// Note this is the **Responses** API, not Chat Completions — third-party
    /// OpenAI-compatible gateways that only implement `/chat/completions`
    /// (LangDock, many EU/self-hosted proxies) will 404 here. Use
    /// [`LlmVendor::OpenAiCompat`] for those.
    #[serde(alias = "openai")]
    OpenAi,
    /// OpenAI-compatible Chat Completions API (`/v1/chat/completions`).
    /// Suitable for Ollama, vLLM, LM Studio, LangDock, and similar
    /// self-hosted or third-party gateways.  Uses `OPENAI_API_KEY` as
    /// fallback (or pass `api_key` directly).
    #[serde(alias = "openai_compat")]
    OpenAiCompat,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct LlmConfigYaml {
    /// Named model reference from the project's `config.yml`.
    ///
    /// When set, the vendor, api_key, and base_url of that model entry are used
    /// as defaults and any other fields in this section override them.
    ///
    /// ```yaml
    /// llm:
    ///   ref: openai-4o-mini   # model name from config.yml
    ///   model: gpt-5.4        # override the model_ref only
    /// ```
    #[serde(default, rename = "ref")]
    pub model_ref: Option<String>,

    /// Backend vendor.  Defaults to [`LlmVendor::Anthropic`] when neither this
    /// field nor `ref` supplies one.
    ///
    /// Overrides the vendor resolved from `ref` when both are set — which is
    /// what lets a `config.yml` model supply the managed `key_var` secret and
    /// `api_url` while the agent redirects the call to a different wire
    /// protocol:
    ///
    /// ```yaml
    /// llm:
    ///   ref: o4-mini            # inherits key_var secret + api_url
    ///   vendor: open_ai_compat  # …but talk /chat/completions, not /responses
    /// ```
    ///
    /// `Option` rather than a defaulted `LlmVendor` on purpose: the override
    /// above is only expressible if "user wrote `anthropic`" is distinguishable
    /// from "field absent". While this was a bare `LlmVendor` it silently
    /// defaulted, so the ref's vendor always won and this field did nothing
    /// whenever `ref` was set.
    #[serde(default)]
    pub vendor: Option<LlmVendor>,

    /// Model ID.  Overrides the `model_ref` resolved from `ref`.
    #[serde(default)]
    pub model: Option<String>,

    /// API key.  Falls back to `ANTHROPIC_API_KEY` (Anthropic) or
    /// `OPENAI_API_KEY` (OpenAi / OpenAiCompat) environment variables.
    #[serde(default)]
    pub api_key: Option<String>,

    /// Base URL override.
    ///
    /// - Anthropic: proxy URL (default: `https://api.anthropic.com/v1/messages`)
    /// - OpenAi: Responses API base (default: `https://api.openai.com/v1/responses`)
    /// - OpenAiCompat: local server root, e.g. `http://localhost:11434/v1`
    #[serde(default)]
    pub base_url: Option<String>,

    /// Maximum output tokens per call.  Defaults to 4096.
    #[serde(default)]
    pub max_tokens: Option<u32>,

    /// Default thinking/reasoning config for all pipeline states.
    ///
    /// Takes precedence over the top-level `thinking:` field on `AgentConfig`.
    ///
    /// ```yaml
    /// llm:
    ///   thinking: adaptive
    /// ```
    #[serde(default)]
    pub thinking: Option<ThinkingConfigYaml>,

    /// "Extended thinking" mode preset: an alternative model + thinking configuration
    /// activated at runtime via the UI toggle.
    ///
    /// ```yaml
    /// llm:
    ///   thinking: effort:low
    ///   extended_thinking:
    ///     model: gpt-5.4
    ///     thinking: effort:medium
    /// ```
    #[serde(default)]
    pub extended_thinking: Option<ExtendedThinkingConfigYaml>,
}

/// Extended thinking mode preset configuration.
///
/// Overrides the default model and/or thinking config when the user
/// activates "extended thinking" mode from the UI.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ExtendedThinkingConfigYaml {
    /// Model ID override for extended thinking mode.
    #[serde(default)]
    pub model: Option<String>,
    /// Thinking config override for extended thinking mode.
    #[serde(default)]
    pub thinking: Option<ThinkingConfigYaml>,
}
