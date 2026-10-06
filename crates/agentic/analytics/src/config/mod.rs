//! Configuration layer for [`AnalyticsSolver`].
//!
//! Loads an agent YAML config file, resolves context globs (`.view.yml`,
//! `.topic.yml`, `.sql`, `.md`), builds the semantic catalog, constructs
//! the LLM client, and wires everything into an [`AnalyticsSolver`].
//!
//! # Example YAML
//!
//! ```yaml
//! instructions: |
//!   You are a revenue analytics assistant for Acme Corp.
//!   Always report currency in USD.
//!
//! databases:
//!   - type: duckdb
//!     path: ./data/warehouse.duckdb
//!
//! llm:
//!   model: claude-opus-4-6
//!   max_tokens: 4096
//!
//! context:
//!   - ./semantics/*.view.yml
//!   - ./semantics/*.topic.yml
//!   - ./example_sql/*.sql
//!   - ./docs/*.md
//!   - ./procedures/**/*.procedure.yml
//!
//! # Optional: delegate query execution to a vendor semantic engine.
//! # When present, the engine path is tried first (highest priority).
//! # Omit this section entirely to use the internal compiler + LLM only.
//!
//! # Cube:
//! semantic_engine:
//!   vendor: cube
//!   base_url: https://cube.example.com
//!   api_token: "${CUBE_API_TOKEN}"
//!
//! # Looker:
//! # semantic_engine:
//! #   vendor: looker
//! #   base_url: https://myco.looker.com
//! #   client_id: "${LOOKER_CLIENT_ID}"
//! #   client_secret: "${LOOKER_CLIENT_SECRET}"
//!
//! states:
//!   clarifying:
//!     instructions: |
//!       "Last quarter" means the most recent completed fiscal quarter.
//!     thinking: adaptive
//!     max_retries: 3
//!     model: claude-haiku-4-5   # fast/cheap model for triage
//!   solving:
//!     instructions: Prefer CTEs over subqueries.
//!     thinking:
//!       budget_tokens: 10000
//!     max_retries: 2
//!     model: claude-opus-4-6    # powerful model for SQL generation
//!   # Reasoning effort (shorthand or map form). On Claude this is
//!   # `output_config.effort` with adaptive thinking, and on 4.7+ it is the
//!   # only way to ask for less thinking -- `budget_tokens` is rejected there.
//!   # Levels: low | medium | high | xhigh | max.
//!   diagnosing:
//!     thinking: "effort:high"
//!   interpreting:
//!     thinking:
//!       effort: medium
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono_tz::Tz;

use crate::catalog::SchemaCatalog;
use crate::engine::cube::CubeEngine;
use crate::engine::looker::LookerEngine;
use crate::engine::{EngineError, SemanticEngine};
#[cfg(test)]
use crate::llm::ReasoningEffort;
use crate::llm::{
    AnthropicProvider, DEFAULT_MODEL, LlmClient, OpenAiCompatProvider, OpenAiProvider,
    ThinkingConfig,
};
use crate::semantic::SemanticCatalog;
use crate::solver::AnalyticsSolver;
use crate::validation::Validator;

pub mod error;
pub mod yaml;

#[cfg(test)]
mod domain_docs_tests;
#[cfg(test)]
mod tests;

pub use error::ConfigError;
pub use yaml::{
    AgentConfig, ExtendedThinkingConfigYaml, LlmConfigYaml, LlmVendor, SemanticEngineConfig,
    StateConfig, ThinkingConfigYaml, VendorKind,
};

/// Files bucketed by type after glob expansion.
#[derive(Debug, Default)]
pub struct ResolvedContext {
    /// `.view.yml` and `.topic.yml` files for the semantic catalog.
    pub semantic_files: Vec<PathBuf>,
    /// `.sql` example files to inject into the Solving prompt.
    pub sql_examples: Vec<String>,
    /// `.md` documentation files to inject into Clarifying and Interpreting.
    pub domain_docs: Vec<String>,
    /// `.procedure.yml`, `.procedure.yaml`, `.automation.yml`, `.automation.yaml`,
    /// and `.sql` files discovered via context globs.
    ///
    /// When non-empty, the HTTP layer wires an `OxyAutomationRunner` initialised
    /// with these paths so the `search_automations` tool can locate them without
    /// scanning the entire project directory.
    pub automation_files: Vec<PathBuf>,
    /// Database names discovered in context files:
    ///
    /// - `data_source:` field of `.view.yml` files
    /// - `/* oxy:\n    database: <name> */` header comment of `.sql` files
    ///
    /// Surfaced for the caller to validate or auto-wire project connectors.
    pub referenced_databases: Vec<String>,
}

// ── Context extraction helpers ─────────────────────────────────────────────────

/// Extract the datasource name from a view YAML file without performing a full parse.
///
/// Accepts both `datasource` (airlayer convention) and `data_source` (legacy).
fn extract_view_data_source(content: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct ViewMeta {
        datasource: Option<String>,
        data_source: Option<String>,
    }
    serde_yaml::from_str::<ViewMeta>(content)
        .ok()
        .and_then(|v| v.datasource.or(v.data_source))
        .filter(|s| !s.is_empty())
}

/// Extract all `database:` values from an `execute_sql` task in an automation YAML file.
///
/// Recursively walks the YAML value tree so that databases nested inside
/// `loop_sequential` (or any other nested task container) are also discovered.
fn extract_automation_databases(content: &str) -> Vec<String> {
    fn collect(val: &serde_yaml::Value, out: &mut Vec<String>) {
        match val {
            serde_yaml::Value::Mapping(m) => {
                if let Some(serde_yaml::Value::String(db)) =
                    m.get(serde_yaml::Value::String("database".into()))
                    && !db.is_empty()
                    && !out.contains(db)
                {
                    out.push(db.clone());
                }
                for (_, v) in m {
                    collect(v, out);
                }
            }
            serde_yaml::Value::Sequence(seq) => {
                for item in seq {
                    collect(item, out);
                }
            }
            _ => {}
        }
    }
    let mut databases = Vec::new();
    if let Ok(val) = serde_yaml::from_str::<serde_yaml::Value>(content) {
        collect(&val, &mut databases);
    }
    databases
}

/// Parsed contents of the leading `/* oxy: ... */` comment block in a SQL file.
///
/// Re-exported from `agentic_core::subrun` so the parser has a single home.
pub use agentic_core::subrun::OxyCommentBlock;

/// Parse the leading `/* oxy: ... */` comment block of a SQL file.
///
/// Re-exported from `agentic_core::subrun` so the format has a single source of truth.
pub use agentic_core::subrun::parse_oxy_comment_block;

/// Push `name` into `list` if it is not already present.
fn push_unique(list: &mut Vec<String>, name: String) {
    if !list.contains(&name) {
        list.push(name);
    }
}

// ── ResolvedModelInfo ─────────────────────────────────────────────────────────

/// Resolved model information from the project's `config.yml`.
///
/// Groups the model name, vendor, API key, and base URL that were scattered
/// across separate [`BuildContext`] fields.  Constructed by the HTTP layer
/// from `ProjectManager::resolve_model()`.
#[derive(Debug, Clone)]
pub struct ResolvedModelInfo {
    /// Model name (e.g. `"claude-opus-4-6"`, `"gpt-4.1"`).
    pub model: String,
    /// LLM vendor resolved from the project model config.
    pub vendor: LlmVendor,
    /// Resolved API key from the model's `key_var` in `config.yml`.
    pub api_key: Option<String>,
    /// Base URL resolved from the project model config (e.g. Ollama's
    /// `api_url` or a custom OpenAI-compat endpoint).
    pub base_url: Option<String>,
    /// Azure deployment ID (e.g. `"my-gpt4o-deployment"`). Present only for
    /// Azure OpenAI models configured with `azure_deployment_id` in config.yml.
    pub azure_deployment_id: Option<String>,
    /// Azure API version (e.g. `"2025-03-01-preview"`). Present only for
    /// Azure OpenAI models configured with `azure_api_version` in config.yml.
    pub azure_api_version: Option<String>,
    /// Extra request headers from the model's `headers:` block, already
    /// resolved (a `{ env_var: … }` entry is looked up in the secret store by
    /// the host before it lands here). Needed by gateways that authenticate or
    /// route on a header rather than the bearer token.
    pub headers: Option<HashMap<String, String>>,
}

// ── BuildContext ──────────────────────────────────────────────────────────────

/// Project-level context passed to [`AgentConfig::build_solver_with_context`].
///
/// Constructed by the HTTP layer from `WorkspaceManager` so that
/// `agentic-analytics` itself does not need a hard dependency on the `oxy`
/// crate.
#[derive(Default)]
pub struct BuildContext {
    /// Pre-built database connectors keyed by logical name.
    pub extra_connectors: HashMap<String, Arc<dyn agentic_connector::DatabaseConnector>>,
    /// Name of the connector to treat as default.
    pub extra_default_connector: Option<String>,
    /// Resolved project model info (model name, vendor, API key, base URL).
    pub project_model_info: Option<ResolvedModelInfo>,
    /// Project-level timezone (from `config.yml` `timezone:`). Used as the
    /// fallback for the date hint when the agent YAML omits `timezone:`.
    pub timezone: Option<Tz>,
    /// Optional schema cache shared across requests.
    pub schema_cache: Option<Arc<Mutex<HashMap<String, SchemaCatalog>>>>,
    /// Runtime thinking config override (from UI "extended thinking" mode toggle).
    pub thinking_override: Option<ThinkingConfig>,
    /// Runtime model override (from UI "extended thinking" mode toggle).
    pub model_override: Option<String>,
    /// The local-rollup short-circuit. `None` when no rebuild worker is
    /// running (CLI, tests).
    pub preagg: Option<agentic_semantic::compile::PreaggContext>,
    /// Stamped on every inference span the solver's clients open — the
    /// agent, tenant and conversation for per-tenant accounting in the
    /// platform trace store. See `agentic_llm::genai`.
    pub genai: agentic_llm::GenAiContext,
    /// The agent's `.md` context documents, already resolved by the host.
    ///
    /// `Some` REPLACES whatever [`AgentConfig::resolve_context`] globbed from
    /// `base_dir`, including with an empty list: the host has answered, and
    /// its answer is the same on a pod with no working copy as on one with
    /// the files. `None` means no host answered (the CLI's default context,
    /// a test), and the documents come from `base_dir` as before.
    pub domain_docs: Option<Vec<String>>,
}

// ── AgentConfig methods ───────────────────────────────────────────────────────

// ── Engine factory ────────────────────────────────────────────────────────────

/// Build a bundled [`SemanticEngine`] adapter from the YAML `semantic_engine` block.
///
/// Covers the two bundled adapters (Cube, Looker).  External engines are
/// supplied programmatically via [`AnalyticsSolverBuilder::engine_arc`] and
/// never pass through this factory.
///
/// Does **not** call `ping()` — the caller is responsible for the startup
/// health-check so it can map [`EngineError::EngineUnreachable`] to
/// [`ConfigError::EngineConnectionError`].
fn build_engine(cfg: &SemanticEngineConfig) -> Result<Box<dyn SemanticEngine>, ConfigError> {
    match cfg.vendor {
        VendorKind::Cube => {
            let token = cfg.resolved_api_token()?;
            Ok(Box::new(CubeEngine::new(cfg.base_url.clone(), token)))
        }
        VendorKind::Looker => {
            let client_id = cfg.resolved_client_id()?;
            let client_secret = cfg.resolved_client_secret()?;
            Ok(Box::new(LookerEngine::new(
                cfg.base_url.clone(),
                client_id,
                client_secret,
            )))
        }
    }
}

/// Resolve which vendor an agent talks to.
///
/// Precedence: an explicitly-written `llm.vendor` → the vendor inherited from
/// `llm.ref:` → [`LlmVendor::Anthropic`].
///
/// The first rung is what lets a `config.yml` model supply a managed `key_var`
/// secret and `api_url` while the agent overrides the *wire protocol* — the
/// only way to reach an OpenAI-compatible gateway that serves
/// `/chat/completions` but 404s on `/responses`.
///
/// Shared by the FSM path and the brief path; both computed it inline before,
/// and they had already drifted apart.
pub(crate) fn resolve_vendor<'a>(
    yaml_vendor: Option<&'a LlmVendor>,
    ref_vendor: Option<&'a LlmVendor>,
) -> &'a LlmVendor {
    const DEFAULT_VENDOR: &LlmVendor = &LlmVendor::Anthropic;
    yaml_vendor.or(ref_vendor).unwrap_or(DEFAULT_VENDOR)
}

/// Build an [`LlmClient`] from the resolved vendor / key / model / base_url.
///
/// Extracted so it can be called both for the global client and for per-state
/// model overrides (which inherit vendor, key, and base_url).
///
/// When `azure_deployment_id` and `azure_api_version` are both `Some`, the
/// model is Azure OpenAI: `OpenAiCompatProvider` is used with the full Azure
/// Chat Completions URL regardless of `vendor`.
/// Everything needed to construct an [`LlmClient`], grouped so the builder
/// doesn't grow past the argument limit as vendors gain knobs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LlmClientSpec<'a> {
    pub vendor: &'a LlmVendor,
    pub api_key: &'a str,
    pub model: &'a str,
    pub base_url: Option<&'a str>,
    pub azure_deployment_id: Option<&'a str>,
    pub azure_api_version: Option<&'a str>,
    /// Extra request headers, already resolved from `key_var`-style
    /// references to literal values.
    pub headers: Option<&'a HashMap<String, String>>,
}

pub(crate) fn build_llm_client(spec: LlmClientSpec<'_>) -> LlmClient {
    let LlmClientSpec {
        vendor,
        api_key,
        model,
        base_url,
        azure_deployment_id,
        azure_api_version,
        headers,
    } = spec;
    let extra_headers = || headers.cloned().unwrap_or_default();

    if let (Some(deployment_id), Some(api_version), Some(base)) =
        (azure_deployment_id, azure_api_version, base_url)
    {
        return LlmClient::with_provider(
            OpenAiCompatProvider::for_azure(api_key, model, base, deployment_id, api_version)
                .with_headers(extra_headers()),
        );
    }
    if azure_deployment_id.is_some() && azure_api_version.is_some() && base_url.is_none() {
        tracing::warn!(
            "Azure config has deployment_id and api_version set but no base_url; \
             falling back to standard OpenAI."
        );
    } else if azure_deployment_id.is_some() != azure_api_version.is_some() {
        tracing::warn!(
            "Azure config is incomplete: both azure_deployment_id and azure_api_version must \
             be set together. Falling back to standard OpenAI."
        );
    }
    match vendor {
        LlmVendor::Anthropic => {
            // `base_url` used to be dropped here, so a `vendor: anthropic`
            // model with a custom `api_url` silently talked to
            // api.anthropic.com — the same class of leak as pointing
            // `vendor: openai` at a Chat-Completions-only gateway. The value is
            // a root; the provider appends `/messages`, exactly as the OpenAI
            // providers append `/responses` and `/chat/completions`.
            let provider = match base_url {
                Some(url) => AnthropicProvider::with_base_url(api_key, model, url),
                None => AnthropicProvider::new(api_key, model),
            };
            LlmClient::with_provider(provider.with_headers(extra_headers()))
        }
        LlmVendor::OpenAi => {
            let provider = match base_url {
                Some(url) => OpenAiProvider::with_base_url(api_key, model, url),
                None => OpenAiProvider::new(api_key, model),
            };
            LlmClient::with_provider(provider.with_headers(extra_headers()))
        }
        LlmVendor::OpenAiCompat => {
            let url = base_url.unwrap_or("http://localhost:11434/v1");
            LlmClient::with_provider(
                OpenAiCompatProvider::new(api_key, model, url).with_headers(extra_headers()),
            )
        }
    }
}

/// Parse the optional `timezone:` config value into a [`Tz`].
///
/// Returns `Ok(None)` when unset (callers fall back to UTC). An unrecognized
/// name yields [`ConfigError::InvalidTimezone`] so misconfiguration fails at
/// solver-build time rather than silently resolving relative dates to the
/// wrong day.
pub(crate) fn parse_timezone(name: Option<&str>) -> Result<Option<Tz>, ConfigError> {
    match name {
        Some(n) => n
            .parse::<Tz>()
            .map(Some)
            .map_err(|_| ConfigError::InvalidTimezone(n.to_string())),
        None => Ok(None),
    }
}

/// Resolve the effective timezone for the date hint.
///
/// The per-agent `.agentic.yml` `timezone:` (`agent_tz`) takes precedence and
/// hard-errors on a bad IANA name; otherwise the project-level `config.yml`
/// timezone (`project_tz`, already parsed) is used; otherwise `None` (UTC).
pub(crate) fn resolve_timezone(
    agent_tz: Option<&str>,
    project_tz: Option<Tz>,
) -> Result<Option<Tz>, ConfigError> {
    Ok(parse_timezone(agent_tz)?.or(project_tz))
}

impl AgentConfig {
    /// Parse an [`AgentConfig`] from a YAML string.
    pub fn from_yaml(yaml: &str) -> Result<Self, ConfigError> {
        Ok(serde_yaml::from_str(yaml)?)
    }

    /// Load an [`AgentConfig`] from a YAML file.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        Self::from_yaml(&content)
    }

    /// Glob-expand `context` patterns and bucket files by extension.
    ///
    /// Patterns are resolved relative to `base_dir`.
    ///
    /// `domain_docs` here is what `base_dir` happens to hold. A host that
    /// answers for the agent's markdown (`BuildContext::domain_docs`) replaces
    /// it, because `base_dir` is not the same directory on every node and
    /// markdown is not in all of them.
    pub fn resolve_context(&self, base_dir: &Path) -> Result<ResolvedContext, ConfigError> {
        let mut ctx = ResolvedContext::default();

        for pattern in &self.context {
            // Make the pattern absolute by prepending base_dir when relative.
            let abs_pattern = if Path::new(pattern).is_absolute() {
                pattern.clone()
            } else {
                base_dir.join(pattern).to_string_lossy().into_owned()
            };

            for entry in glob::glob(&abs_pattern)? {
                let path = entry.map_err(|e| ConfigError::Io(e.into_error()))?;
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

                if name.ends_with(".procedure.yml")
                    || name.ends_with(".procedure.yaml")
                    || name.ends_with(".automation.yml")
                    || name.ends_with(".automation.yaml")
                {
                    let content = std::fs::read_to_string(&path).map_err(ConfigError::Io)?;
                    for db in extract_automation_databases(&content) {
                        push_unique(&mut ctx.referenced_databases, db);
                    }
                    ctx.automation_files.push(path);
                } else if name.ends_with(".view.yml") || name.ends_with(".view.yaml") {
                    // Quick parse to surface the data_source before full catalog load.
                    let content = std::fs::read_to_string(&path).map_err(ConfigError::Io)?;
                    if let Some(db) = extract_view_data_source(&content) {
                        push_unique(&mut ctx.referenced_databases, db);
                    }
                    ctx.semantic_files.push(path);
                } else if name.ends_with(".topic.yml") || name.ends_with(".topic.yaml") {
                    ctx.semantic_files.push(path);
                } else if path.extension().is_some_and(|e| e == "sql") {
                    let content = std::fs::read_to_string(&path).map_err(ConfigError::Io)?;
                    // A `/* oxy: ... */` header opts the SQL file in as a
                    // verified query — searchable via `search_automations` and
                    // executed verbatim. Files without the header stay as
                    // LLM style examples only, so users can drop reference
                    // SQL into `context` without it becoming an authoritative
                    // answer for some unrelated question.
                    match parse_oxy_comment_block(&content) {
                        Some(block) => {
                            if let Some(db) = block.database {
                                push_unique(&mut ctx.referenced_databases, db);
                            }
                            ctx.automation_files.push(path);
                        }
                        None => {
                            ctx.sql_examples.push(content);
                        }
                    }
                } else if path.extension().is_some_and(|e| e == "md") {
                    let content = std::fs::read_to_string(&path).map_err(ConfigError::Io)?;
                    ctx.domain_docs.push(content);
                }
                // Other extensions are silently ignored.
            }
        }

        Ok(ctx)
    }

    /// Build a fully configured [`AnalyticsSolver`] using only the inline config.
    ///
    /// Delegates to [`Self::build_solver_with_context`] with a default
    /// [`BuildContext`].  Use `build_solver_with_context` directly when you
    /// want to inject project-level databases or model overrides.
    pub async fn build_solver(
        &self,
        base_dir: &Path,
    ) -> Result<(AnalyticsSolver, Vec<PathBuf>), ConfigError> {
        self.build_solver_with_context(base_dir, BuildContext::default())
            .await
    }

    /// Build a fully configured [`AnalyticsSolver`] using connectors supplied
    /// by the HTTP layer via [`BuildContext`].
    ///
    /// # Steps
    ///
    /// 1. Resolve context globs → [`ResolvedContext`].
    /// 2. Load semantic files → `Option<SemanticCatalog>`.
    /// 3. Merge [`BuildContext::extra_connectors`] and their introspected schemas.
    /// 4. Build [`SemanticCatalog`] from semantic + merged schema.
    /// 5. Build [`LlmClient`] with project/env-var fallbacks.
    /// 6. Construct [`AnalyticsSolver`] with context and global thinking.
    ///
    /// Returns `(solver, automation_files)` where `automation_files` is the list
    /// of `.procedure.yml` paths discovered via `context` globs.
    pub async fn build_solver_with_context(
        &self,
        base_dir: &Path,
        build_ctx: BuildContext,
    ) -> Result<(AnalyticsSolver, Vec<PathBuf>), ConfigError> {
        // 1. Resolve context.
        let ctx = self.resolve_context(base_dir)?;
        // 2. Merge connectors injected by the HTTP layer from `databases:` names.
        if build_ctx.extra_connectors.is_empty() {
            return Err(ConfigError::NoDatabases);
        }

        let mut connectors: HashMap<String, Arc<dyn agentic_connector::DatabaseConnector>> =
            HashMap::new();

        for (name, connector) in build_ctx.extra_connectors {
            connectors.insert(name, connector);
        }

        let default_connector = build_ctx
            .extra_default_connector
            // `min()`, not `next()`: HashMap order is nondeterministic, and an
            // arbitrary-but-stable default beats a different one per process.
            // Unreachable on the main path, but it is the same tail
            // `resolve_solution_connector` deletes one crate over.
            .or_else(|| connectors.keys().min().cloned())
            .unwrap_or_default();

        // 3. Load semantic catalog (after connectors so we have dialect info).
        //    Schema introspection is no longer done eagerly — the LLM uses
        //    `list_tables` / `describe_table` tools on demand.
        let dialect_map =
            crate::airlayer_compat::build_dialect_map(&connectors, &default_connector);
        let catalog = if ctx.semantic_files.is_empty() {
            SemanticCatalog::empty()
        } else {
            SemanticCatalog::load_files(&ctx.semantic_files, dialect_map)
                .map_err(ConfigError::SemanticError)?
        };

        // 5. Build LLM client.
        //
        // Precedence (highest → lowest):
        //   a. Explicit `llm:` fields in the agent YAML.
        //   b. Project model resolved from config.yml via BuildContext.
        //   c. Environment variables / built-in defaults.
        //
        // When `llm.model` is absent in the YAML the project model config
        // (vendor, model_ref, api_key, base_url) is used wholesale.
        let pmi = &build_ctx.project_model_info;
        let model = self
            .llm
            .model
            .as_deref()
            .or(pmi.as_ref().map(|m| m.model.as_str()))
            .unwrap_or(DEFAULT_MODEL)
            .to_string();

        // Vendor precedence: explicit `llm.vendor` → ref's vendor → default.
        //
        // An explicitly-written `llm.vendor` always wins, including over a
        // `ref`. That combination is the only way to inherit a config.yml
        // model's managed `key_var` secret and `api_url` while overriding the
        // wire protocol — needed for OpenAI-compatible gateways that serve
        // `/chat/completions` but not `/responses` (LangDock and similar).
        //
        // This used to be unexpressible: `llm.vendor` was a defaulted
        // `LlmVendor`, so "user wrote anthropic" and "field absent" were
        // indistinguishable and the code conservatively let the ref's vendor
        // win whenever a ref was present — silently ignoring the field its own
        // docs said would override. `Option` makes the distinction real.
        let effective_vendor =
            resolve_vendor(self.llm.vendor.as_ref(), pmi.as_ref().map(|m| &m.vendor));

        // API key: YAML → project resolved key → vendor env var.
        let api_key = self
            .llm
            .api_key
            .clone()
            .or_else(|| pmi.as_ref().and_then(|m| m.api_key.clone()))
            .or_else(|| match effective_vendor {
                LlmVendor::Anthropic => std::env::var("ANTHROPIC_API_KEY").ok(),
                LlmVendor::OpenAi | LlmVendor::OpenAiCompat => std::env::var("OPENAI_API_KEY").ok(),
            })
            .unwrap_or_default();

        // Base URL: YAML → project resolved URL.
        let effective_base_url = self
            .llm
            .base_url
            .as_deref()
            .or(pmi.as_ref().and_then(|m| m.base_url.as_deref()));

        // Azure fields and custom headers come from the project model config
        // (not overridable per-state).
        let azure_deployment_id = pmi.as_ref().and_then(|m| m.azure_deployment_id.as_deref());
        let azure_api_version = pmi.as_ref().and_then(|m| m.azure_api_version.as_deref());
        let headers = pmi.as_ref().and_then(|m| m.headers.as_ref());

        let spec = LlmClientSpec {
            vendor: effective_vendor,
            api_key: &api_key,
            model: &model,
            base_url: effective_base_url,
            azure_deployment_id,
            azure_api_version,
            headers,
        };
        let client = build_llm_client(spec).with_genai_context(build_ctx.genai.clone());

        // Build per-state clients for states that declare a `model:` override.
        // Inherits vendor / api_key / base_url / headers / azure from the global config.
        let state_clients: std::collections::HashMap<String, LlmClient> = self
            .states
            .iter()
            .filter_map(|(state_name, state_cfg)| {
                state_cfg.model.as_deref().map(|state_model| {
                    let c = build_llm_client(LlmClientSpec {
                        model: state_model,
                        ..spec
                    })
                    .with_genai_context(build_ctx.genai.clone());
                    (state_name.clone(), c)
                })
            })
            .collect();

        // 6. Build validator from config (defaults to all rules enabled).
        let validator = match &self.validation {
            Some(cfg) => Validator::from_config(cfg)
                .map_err(|e| ConfigError::ValidationError(e.to_string()))?,
            None => Validator::default_validator(),
        };

        // 7. Build and health-check the vendor engine (if configured).
        let engine: Option<Arc<dyn SemanticEngine>> =
            if let Some(engine_cfg) = &self.semantic_engine {
                let engine = build_engine(engine_cfg)?;
                engine.ping().await.map_err(|e| match e {
                    EngineError::EngineUnreachable(msg) => ConfigError::EngineConnectionError(msg),
                    other => ConfigError::EngineConnectionError(other.to_string()),
                })?;
                Some(Arc::from(engine))
            } else {
                None
            };

        // 8. Construct solver with all connectors.
        // Per-agent `.agentic.yml` `timezone:` wins; otherwise fall back to
        // the project-level `config.yml` timezone threaded in via BuildContext;
        // otherwise UTC. An invalid per-agent value hard-errors.
        let timezone = resolve_timezone(self.timezone.as_deref(), build_ctx.timezone)?;
        let mut solver = AnalyticsSolver::new_multi(client, catalog, connectors, default_connector)
            .with_global_instructions(self.instructions.clone())
            .with_timezone(timezone)
            .with_sql_examples(ctx.sql_examples)
            .with_domain_docs(build_ctx.domain_docs.unwrap_or(ctx.domain_docs))
            .with_state_configs(self.states.clone())
            .with_state_clients(state_clients)
            .with_validator(validator)
            .with_max_tokens(self.llm.max_tokens)
            .with_workspace_path(base_dir.to_path_buf());

        // Ambient data-freshness hint: probe each view's watermark once per
        // build (TTL-cached process-wide) so every state's dynamic system
        // suffix carries live coverage boundaries beside the date hint.
        // Best-effort: `None` (no targets / probes failed) means the suffix
        // is the date hint alone, exactly as before.
        solver.freshness_hint = crate::solver::freshness_hint::compute_freshness_hint(
            &base_dir.display().to_string(),
            &*solver.catalog,
            &solver.connectors,
            &solver.default_connector,
        )
        .await;

        // Wire global thinking config: llm.thinking > top-level thinking.
        let effective_thinking = self.llm.thinking.as_ref().or(self.thinking.as_ref());
        if let Some(thinking_cfg) = effective_thinking {
            solver = solver.with_global_thinking(thinking_cfg.to_thinking_config());
        }

        // Apply runtime overrides from the UI "extended thinking" mode toggle.
        // These override *all* config — including per-state thinking and
        // model overrides — so the extended thinking preset wins unconditionally.
        let extended_thinking_active =
            build_ctx.thinking_override.is_some() || build_ctx.model_override.is_some();
        if let Some(thinking) = build_ctx.thinking_override {
            solver = solver.with_global_thinking(thinking);
        }
        if let Some(ref override_model) = build_ctx.model_override {
            let override_client = build_llm_client(LlmClientSpec {
                model: override_model,
                ..spec
            })
            .with_genai_context(build_ctx.genai.clone());
            solver = solver.with_client_override(override_client);
        }
        // `with_client_override` already sets `extended_thinking_active = true`
        // when a model override is present.  Handle the case where only a
        // thinking override is provided (no model override).
        if extended_thinking_active && !solver.extended_thinking_active {
            solver.extended_thinking_active = true;
        }

        if let Some(e) = engine {
            solver = solver.with_engine(e);
        }

        // Layer-1 preagg wiring: when the host provides a cache and a scan
        // path, the Specifying stage will short-circuit successful airlayer
        // compiles to `SolutionPayload::Preaggregation` whenever a built
        // rollup covers the request, and the Executing stage runs DuckDB
        // against the cached Parquet instead of round-tripping to the
        // warehouse.
        solver = solver.with_preagg(build_ctx.preagg);

        Ok((solver, ctx.automation_files))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
