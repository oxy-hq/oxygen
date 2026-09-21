use crate::types::SemanticQueryParams;
use garde::Validate;
use indoc::indoc;
use itertools::Itertools;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cmp::PartialEq;
use std::collections::HashMap;
use std::hash::Hash;
use std::path::PathBuf;
use utoipa::ToSchema;

pub use variables::{Variable, Variables};

use super::validate::validate_task;
use crate::adapters::secrets::SecretsManager;
use crate::config::validate::validate_file_path;
use crate::config::validate::{
    ValidationContext, validate_consistency_prompt, validate_database_exists, validate_env_var,
    validate_looker_integration_exists, validate_omni_integration_exists,
    validate_task_data_reference,
};
pub use automation::{AutomationWithRawVariables, WorkflowWithRawVariables};
pub use duckdb::{
    CatalogConfig, DuckDBOptions, DuckDbS3Mirror, DuckDbS3Table, DuckLakeConfig, S3StorageSecret,
    StorageConfig,
};
pub use oxy_llm::{
    AnthropicModelConfig, GeminiModelConfig, HeaderValue, Model, OPENAI_API_URL, OllamaModelConfig,
    OpenAIModelConfig, default_openai_api_url,
};
use oxy_shared::errors::OxyError;

mod automation;
mod duckdb;
mod variables;

/// Configuration for the background pre-aggregation refresh worker.
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct RefreshWorkerConfig {
    /// Set to `false` to disable the background worker entirely.
    #[serde(default = "default_true", skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// How often the worker wakes up to check staleness (e.g. "30s", "5m").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat: Option<String>,
    /// How long a cached refresh_key result is valid before re-evaluating (e.g. "120s").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renewal_threshold: Option<String>,
}

fn default_true() -> Option<bool> {
    Some(true)
}

/// Top-level `pre_aggregations:` block in `config.yml`.
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct PreaggConfig {
    /// Warehouse schema where pre-agg tables are created. Defaults to `"AIRLAYER"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Database connector used for pre-agg builds. Defaults to each view's own `datasource`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_worker: Option<RefreshWorkerConfig>,
}

/// Configuration for the built-in builder copilot agent.
///
/// Example:
///   `builder_agent: { model: "claude-sonnet-4-6" }`
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(untagged)]
pub enum BuilderAgentConfig {
    /// Built-in copilot configured only with a model name.
    Builtin { model: String },
}

#[derive(Serialize, Deserialize, Validate, Debug, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
#[garde(context(ValidationContext))]
pub struct Config {
    #[garde(dive)]
    pub defaults: Option<Defaults>,
    // Both default to empty, like `integrations` below. Without it `serde`
    // makes them REQUIRED keys, and because the struct is
    // `deny_unknown_fields` a `config.yml` that simply has nothing to declare
    // — a workspace with only integrations, or only databases — fails to parse
    // as a whole. `storage.rs` then continues with `Config::default()`, so one
    // absent key silently discards every integration, database and model the
    // file DID declare, and it resurfaces downstream as "not configured",
    // reading as the customer's mistake. Seen in production as
    // "config.yml is present but does not parse" / missing field `models`.
    //
    // Empty is a valid value for both, not a placeholder: `validate_models`
    // iterates and `#[garde(dive)]` descends, so neither validator has
    // anything to object to.
    #[serde(default)]
    #[garde(custom(validate_models))]
    pub models: Vec<Model>,
    #[serde(default)]
    #[garde(dive)]
    pub databases: Vec<Database>,
    #[garde(skip)]
    pub builder_agent: Option<BuilderAgentConfig>,

    /// IANA timezone (e.g. `America/Los_Angeles`) for resolving relative
    /// dates ("yesterday", "last week") in agentic runs. When unset, dates
    /// resolve in UTC. A per-agent `.agentic.yml` `timezone:` overrides this
    /// for that agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[garde(skip)]
    pub timezone: Option<String>,

    #[serde(skip)]
    #[garde(skip)]
    #[schemars(skip)]
    pub workspace_path: PathBuf,

    #[serde(default)]
    #[garde(skip)]
    pub integrations: Vec<Integration>,

    /// Legacy `slack:` section — tolerated for backward compatibility but no longer read.
    /// Users should remove this from config.yml; Slack is now configured per-org via OAuth.
    #[serde(default, skip_serializing, rename = "slack")]
    #[garde(skip)]
    #[schemars(skip)]
    pub slack_legacy: Option<serde_yaml::Value>,

    /// Optional MCP configuration for exposing resources as tools
    /// If not specified, all agents and automations are exposed by default
    #[serde(skip_serializing_if = "Option::is_none")]
    #[garde(dive)]
    pub mcp: Option<McpConfig>,

    /// Per-workspace health checks, and how often they run. Drives the
    /// workspace's `health_eval` schedule row.
    ///
    /// **Unset means off** — health checks are opt-in. Writing the block is the
    /// opt-in and the cadence defaults to 1h, so `health_check: {}` is enough to
    /// turn them on; `enabled: false` inside a block turns them back off. See
    /// `health_check::resolve_enabled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[garde(skip)]
    pub health_check: Option<crate::config::health_check::HealthCheckConfig>,

    /// Branches that are protected: saving a file while on one of these branches
    /// will auto-create a new feature branch instead of writing directly.
    /// Defaults to [default_branch] (usually "main") when not set.
    ///
    /// Example config.yml:
    ///   protected_branches:
    ///     - main
    ///     - develop
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[garde(skip)]
    pub protected_branches: Option<Vec<String>>,

    /// Branch that new worktrees fork from when saving on a protected branch.
    /// Defaults to the currently checked-out branch (usually the default branch)
    /// when not set.  Use this when Oxy serves from a "deployment" branch but
    /// new work should fork from an "integration" branch (e.g. serve from
    /// `deploy`, fork new work from `main`).
    ///
    /// Example config.yml:
    ///   base_branch: main
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[garde(skip)]
    pub base_branch: Option<String>,

    /// External repositories (dbt, LookML, data models, etc.) to surface in the IDE.
    ///
    /// Example config.yml:
    ///   repositories:
    ///     - name: dbt-models
    ///       path: ../my-dbt-project
    ///     - name: lookml
    ///       git_url: https://github.com/acme/lookml-repo
    ///       branch: main
    #[serde(default, skip_serializing_if = "Vec::is_empty", alias = "data_repos")]
    #[garde(skip)]
    pub repositories: Vec<Repository>,

    /// Pre-aggregation configuration (schema, database, background worker).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[garde(skip)]
    pub pre_aggregations: Option<PreaggConfig>,

    /// Deprecated: admin configuration is now set via the `OXY_OWNER` environment variable.
    /// Kept for backward compatibility so existing configs don't silently break.
    #[serde(default, skip_serializing)]
    #[garde(skip)]
    #[schemars(skip)]
    pub admins: Vec<String>,
}

/// An external repository (dbt, LookML, data models, etc.) linked to an Oxy project.
/// Either `path` or `git_url` must be set.
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
pub struct Repository {
    /// Display name used as the path prefix `@{name}/` in the IDE.
    pub name: String,
    /// Local filesystem path, relative to the project root or absolute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Remote git URL to clone. Repo is cloned to `.repositories/{name}/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_url: Option<String>,
    /// Branch to check out when cloning from `git_url`. Defaults to HEAD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// GitHub namespace UUID used to get a fresh installation token for push.
    /// Set when the repo was linked via the GitHub App.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_namespace_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Validate)]
#[garde(context(ValidationContext))]
pub struct Integration {
    #[garde(skip)]
    pub name: String,
    #[serde(flatten)]
    #[garde(skip)]
    pub integration_type: IntegrationType,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum IntegrationType {
    #[serde(rename = "omni")]
    Omni(OmniIntegration),
    #[serde(rename = "looker")]
    Looker(LookerIntegration),
    #[serde(rename = "toast")]
    Toast(ToastIntegration),
    #[serde(rename = "toast_analytics")]
    ToastAnalytics(ToastAnalyticsIntegration),
    #[serde(rename = "openweathermap")]
    OpenWeatherMap(OpenWeatherMapIntegration),
    #[serde(rename = "besttime")]
    BestTime(BestTimeIntegration),
    #[serde(rename = "unifi")]
    Unifi(UnifiIntegration),
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Validate)]
#[garde(context(ValidationContext))]
pub struct OmniIntegration {
    #[garde(custom(validate_env_var))]
    pub api_key_var: String,
    #[garde(length(min = 1))]
    pub base_url: String,
    #[garde(dive)]
    pub topics: Vec<OmniTopic>,
    /// Row count threshold for switching to Arrow format (file path response)
    /// If query result exceeds this threshold, return file_path instead of row arrays
    /// Default: 1000 rows
    #[serde(default = "default_arrow_threshold_rows")]
    #[garde(skip)]
    pub arrow_threshold_rows: usize,
}

fn default_arrow_threshold_rows() -> usize {
    1000
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Validate)]
#[garde(context(ValidationContext))]
pub struct OmniTopic {
    #[garde(length(min = 1))]
    pub name: String,
    #[garde(length(min = 1))]
    pub model_id: String,
}

/// Looker integration configuration.
///
/// Provides connection settings for a Looker instance, including OAuth credentials
/// and the list of explores to expose.
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Validate)]
#[garde(context(ValidationContext))]
pub struct LookerIntegration {
    /// Environment variable containing the Looker client ID
    #[garde(custom(validate_env_var))]
    pub client_id_var: String,
    /// Environment variable containing the Looker client secret
    #[garde(custom(validate_env_var))]
    pub client_secret_var: String,
    /// Base URL for the Looker instance (e.g., https://your.looker.com:19999)
    #[garde(length(min = 1))]
    pub base_url: String,
    /// List of explores to expose from this Looker instance
    #[garde(dive)]
    pub explores: Vec<LookerExplore>,
}

/// Configuration for a Looker explore to expose.
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Validate)]
#[garde(context(ValidationContext))]
pub struct LookerExplore {
    /// The LookML model name containing this explore
    #[garde(length(min = 1))]
    pub model: String,
    /// The explore name within the model
    #[garde(length(min = 1))]
    pub name: String,
    /// Optional description for the explore
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[garde(skip)]
    pub description: Option<String>,
}

/// Toast POS webhook integration.
///
/// The webhook receiver at `POST /api/webhooks/toast/orders?project_id=...`
/// validates incoming payloads with the secret referenced by
/// `webhook_secret_var` (HMAC-SHA256 over body + timestamp).
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Validate)]
#[garde(context(ValidationContext))]
pub struct ToastIntegration {
    /// Environment variable / workspace secret name containing the
    /// Toast webhook signing secret.
    #[garde(custom(validate_env_var))]
    pub webhook_secret_var: String,
    /// Restaurant GUIDs this workspace is authorized to receive events
    /// for. Empty list = accept all (dev convenience).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[garde(skip)]
    pub restaurant_guids: Vec<String>,
}

/// Toast Analytics API integration — the **reconciliation** source.
///
/// Distinct from `ToastIntegration` (webhooks): this carries the credentials and
/// gateway the workspace-health **Reconciliation** dimension uses to call
/// Toast's async Analytics Metrics API. A `reconcile.yml` check binds to it by
/// name via `integration:`. Keeping it separate leaves the webhook integration
/// (managed through the world-model Apps UI) untouched.
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Validate)]
#[garde(context(ValidationContext))]
pub struct ToastAnalyticsIntegration {
    /// Workspace-secret name holding the Toast OAuth client id. Paired with
    /// `client_secret_var` for client-credentials auth; absent ⇒ fall back to
    /// `api_token_var`. (Validation is skipped: in cloud mode the value lives in
    /// the workspace secret store, not the process env.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[garde(skip)]
    pub client_id_var: Option<String>,
    /// Workspace-secret name holding the Toast OAuth client secret. See
    /// `client_id_var`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[garde(skip)]
    pub client_secret_var: Option<String>,
    /// Workspace-secret name holding a static Toast API bearer token, used when
    /// the OAuth client pair is not configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[garde(skip)]
    pub api_token_var: Option<String>,
    /// Toast API gateway base URL (e.g. `https://ws-api.toasttab.com` prod,
    /// `https://ws-sandbox-api.toasttab.com` sandbox). Absent ⇒ fall back to the
    /// `OXY_TOAST_BASE_URL` env var, then the prod gateway default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[garde(skip)]
    pub base_url: Option<String>,
}

/// OpenWeatherMap weather data integration.
///
/// Powers the world-model weather tile proxy and current-weather batch
/// endpoint. Reads `api_key_var` from workspace secrets.
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Validate)]
#[garde(context(ValidationContext))]
pub struct OpenWeatherMapIntegration {
    #[garde(custom(validate_env_var))]
    pub api_key_var: String,
}

/// BestTime foot-traffic integration.
///
/// Powers the world-model foot-traffic and radar endpoints. Reads
/// `api_key_var` from workspace secrets.
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Validate)]
#[garde(context(ValidationContext))]
pub struct BestTimeIntegration {
    #[garde(custom(validate_env_var))]
    pub api_key_var: String,
}

/// UniFi camera integration.
///
/// Powers the world-model camera proxy (UniFi Site Manager devices). Reads
/// `api_key_var` from workspace secrets, mirroring the weather/foot-traffic
/// integrations instead of reading a raw process env var.
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Validate)]
#[garde(context(ValidationContext))]
pub struct UnifiIntegration {
    #[garde(custom(validate_env_var))]
    pub api_key_var: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Validate)]
#[garde(context(ValidationContext))]
pub struct McpConfig {
    /// List of file patterns to expose as MCP tools
    /// Supports glob patterns like "agents/*.agent.yml"
    /// Examples:
    /// - "agents/sql-generator.agent.yml" (specific file)
    /// - "agents/*.agent.yml" (all agents in directory)
    /// - "workflows/**/*.automation.yml" (all automations recursively)
    /// - "semantics/topics/*.topic.yml" (semantic topics)
    /// - "sqls/queries/*.sql" (SQL files)
    #[serde(default)]
    #[garde(skip)]
    pub tools: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug, JsonSchema, ToSchema)]
pub struct SemanticModels {
    pub table: String,
    pub database: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub entities: Vec<Entity>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub dimensions: Vec<Dimension>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub measures: Vec<Measure>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub database_name: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, JsonSchema, ToSchema)]
pub struct Entity {
    pub name: String,
    pub description: String,
    pub sample: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug, JsonSchema, ToSchema)]
pub struct Dimension {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synonyms: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub sample: Vec<String>,
    #[serde(rename = "type", alias = "type")]
    pub data_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_partition_key: Option<bool>,
}

#[derive(Clone, Serialize, Deserialize, Debug, JsonSchema, ToSchema)]
pub struct Measure {
    pub name: String,
    pub sql: String,
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Validate, Default)]
#[garde(context(ValidationContext))]
pub struct Postgres {
    #[serde(default)]
    #[garde(skip)]
    pub host: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub host_var: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub port: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub port_var: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub user: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub user_var: Option<String>,
    #[garde(skip)]
    #[schemars(skip)]
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub password_var: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub database: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub database_var: Option<String>,
}

impl Postgres {
    pub async fn get_password(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.password.as_deref(),
                self.password_var.as_deref(),
                "password",
                None,
            )
            .await
    }

    pub async fn get_host(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.host.as_deref(),
                self.host_var.as_deref(),
                "host",
                Some("localhost"),
            )
            .await
    }

    pub async fn get_port(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.port.as_deref(),
                self.port_var.as_deref(),
                "port",
                Some("5432"),
            )
            .await
    }

    pub async fn get_user(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.user.as_deref(),
                self.user_var.as_deref(),
                "user",
                Some("postgres"),
            )
            .await
    }

    pub async fn get_database(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.database.as_deref(),
                self.database_var.as_deref(),
                "database",
                Some("postgres"),
            )
            .await
    }
}

/// Airhouse: Postgres-wire-protocol warehouse that speaks the DuckDB SQL dialect.
///
/// Connection uses the same pgwire transport as `Postgres`, but queries are written
/// in DuckDB syntax — so `Database::dialect()` returns `"duckdb"` for Airhouse and
/// the connector layer hardcodes the `postgres://` scheme for transport.
#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Validate, Default)]
#[garde(context(ValidationContext))]
pub struct Airhouse {
    #[serde(default)]
    #[garde(skip)]
    pub host: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub host_var: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub port: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub port_var: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub user: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub user_var: Option<String>,
    #[garde(skip)]
    #[schemars(skip)]
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub password_var: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub database: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub database_var: Option<String>,
}

/// Airhouse with managed credentials. Empty by design — host, port, dbname,
/// user, and password are sourced from oxy's per-user `airhouse_users` row.
///
/// **Currently safe only in local (`oxy start`) mode.** Resolution today
/// picks the single active provisioned user, which is unambiguous only when
/// there is exactly one user (the local-mode case). In cloud / multi-user
/// deployments the connector layer does not yet have request-time user
/// context, so a workspace with more than one provisioned Airhouse user will
/// fail to build the connector with a `ConfigurationError`. Use the
/// per-user `host`/`username`/`password_var` fields on `airhouse:` config
/// instead until plumbing the requesting user lands.
#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Default)]
pub struct AirhouseManaged {}

/// Marker for `type: postgres_managed`. Field-free by design: host, database,
/// user, and password all come from the caller's workspace → org →
/// `oltp_tenants` row, so there is nothing for a project to configure — and
/// nothing it can set that would widen the access it receives.
#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Default)]
pub struct PostgresManaged {}

impl Airhouse {
    pub async fn get_password(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.password.as_deref(),
                self.password_var.as_deref(),
                "password",
                Some("airhouse"),
            )
            .await
    }

    pub async fn get_host(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.host.as_deref(),
                self.host_var.as_deref(),
                "host",
                Some("localhost"),
            )
            .await
    }

    pub async fn get_port(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.port.as_deref(),
                self.port_var.as_deref(),
                "port",
                Some("5445"),
            )
            .await
    }

    pub async fn get_user(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.user.as_deref(),
                self.user_var.as_deref(),
                "user",
                Some("admin"),
            )
            .await
    }

    pub async fn get_database(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.database.as_deref(),
                self.database_var.as_deref(),
                "database",
                Some("airhouse"),
            )
            .await
    }
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Validate, Default)]
#[garde(context(ValidationContext))]
pub struct Redshift {
    #[serde(default)]
    #[garde(skip)]
    pub host: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub host_var: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub port: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub port_var: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub user: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub user_var: Option<String>,
    #[garde(skip)]
    #[schemars(skip)]
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub password_var: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub database: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub database_var: Option<String>,
}

impl Redshift {
    pub async fn get_password(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.password.as_deref(),
                self.password_var.as_deref(),
                "password",
                None,
            )
            .await
    }

    pub async fn get_host(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.host.as_deref(),
                self.host_var.as_deref(),
                "host",
                Some("localhost"),
            )
            .await
    }

    pub async fn get_port(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.port.as_deref(),
                self.port_var.as_deref(),
                "port",
                Some("5439"),
            )
            .await
    }

    pub async fn get_user(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.user.as_deref(),
                self.user_var.as_deref(),
                "user",
                Some("awsuser"),
            )
            .await
    }

    pub async fn get_database(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.database.as_deref(),
                self.database_var.as_deref(),
                "database",
                Some("dev"),
            )
            .await
    }
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Validate, Default)]
#[garde(context(ValidationContext))]
pub struct Mysql {
    #[serde(default)]
    #[garde(skip)]
    pub host: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub host_var: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub port: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub port_var: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub user: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub user_var: Option<String>,
    #[garde(skip)]
    #[schemars(skip)]
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub password_var: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub database: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub database_var: Option<String>,
}

impl Mysql {
    pub async fn get_password(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.password.as_deref(),
                self.password_var.as_deref(),
                "password",
                None,
            )
            .await
    }

    pub async fn get_host(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.host.as_deref(),
                self.host_var.as_deref(),
                "host",
                Some("localhost"),
            )
            .await
    }

    pub async fn get_port(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.port.as_deref(),
                self.port_var.as_deref(),
                "port",
                Some("3306"),
            )
            .await
    }

    pub async fn get_user(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.user.as_deref(),
                self.user_var.as_deref(),
                "user",
                Some("root"),
            )
            .await
    }

    pub async fn get_database(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.database.as_deref(),
                self.database_var.as_deref(),
                "database",
                Some("mysql"),
            )
            .await
    }
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Validate, Default)]
#[garde(context(ValidationContext))]
pub struct ClickHouse {
    #[serde(default)]
    #[garde(skip)]
    pub host: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub host_var: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub user: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub user_var: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    #[schemars(skip)]
    pub password: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub password_var: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub database: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub database_var: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub schemas: HashMap<String, Vec<String>>,
    #[serde(default)]
    #[garde(skip)]
    pub role: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub settings_prefix: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub filters: HashMap<String, schemars::schema::SchemaObject>,
}

impl ClickHouse {
    pub async fn get_password(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.password.as_deref(),
                self.password_var.as_deref(),
                "password",
                None,
            )
            .await
    }

    pub async fn get_host(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.host.as_deref(),
                self.host_var.as_deref(),
                "ClickHouse host",
                None,
            )
            .await
    }

    pub async fn get_user(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.user.as_deref(),
                self.user_var.as_deref(),
                "ClickHouse user",
                None,
            )
            .await
    }

    pub async fn get_database(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.database.as_deref(),
                self.database_var.as_deref(),
                "ClickHouse database",
                None,
            )
            .await
    }
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Validate, Default)]
#[garde(context(ValidationContext))]
pub struct DOMO {
    #[garde(length(min = 1))]
    pub instance: String,
    #[garde(length(min = 1))]
    pub developer_token_var: String,
    #[garde(length(min = 1))]
    pub dataset_id: String,
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Validate, Default)]
#[garde(context(ValidationContext))]
pub struct MotherDuck {
    #[garde(custom(validate_env_var))]
    pub token_var: String,
    #[serde(default)]
    #[garde(skip)]
    pub database: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub schemas: HashMap<String, Vec<String>>,
}

impl MotherDuck {
    pub async fn get_token(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(None, Some(&self.token_var), "MotherDuck token", None)
            .await
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Validate)]
#[garde(context(ValidationContext))]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Validate)]
#[garde(context(ValidationContext))]
pub struct ReasoningConfig {
    #[garde(dive)]
    pub effort: ReasoningEffort,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Validate)]
#[garde(context(ValidationContext))]
#[derive(Default)]
pub struct RouteRetrievalConfig {
    /// List of prompts that include this document / route for retrieval
    #[garde(skip)]
    #[serde(default)]
    pub include: Vec<String>,
    /// List of prompts that exclude this document / route for retrieval
    #[garde(skip)]
    #[serde(default)]
    pub exclude: Vec<String>,
}

// These are settings stored as strings derived from the config.yml file's defaults section
#[derive(Debug, Validate, Deserialize, Serialize, Clone, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct Defaults {
    #[garde(length(min = 1))]
    #[garde(custom(|db: &Option<String>, ctx: &ValidationContext| {
        match db {
            Some(database) => validate_database_exists(database.as_str(), ctx),
            None => Ok(()),
        }
    }))]
    pub database: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Validate, Clone, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct BigQuery {
    #[garde(skip)]
    #[serde(default)]
    pub key_path: Option<PathBuf>,
    #[garde(skip)]
    #[serde(default)]
    pub key_path_var: Option<String>,
    #[garde(length(min = 1))]
    pub dataset: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub datasets: HashMap<String, Vec<String>>,
    #[garde(range(min = 1))]
    pub dry_run_limit: Option<u64>,
}

impl BigQuery {
    pub async fn get_key_path(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        if let Some(key_path) = &self.key_path {
            return Ok(key_path.to_string_lossy().to_string());
        }
        if let Some(key_path_var) = &self.key_path_var {
            let value = secret_manager.resolve_secret(key_path_var).await?;
            match value {
                Some(res) => Ok(res),
                None => Err(OxyError::SecretNotFound(Some(key_path_var.clone()))),
            }
        } else {
            Err(OxyError::ConfigurationError(
                "BigQuery key_path or key_path_var must be specified".to_string(),
            ))
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Validate, Clone, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct DuckDB {
    #[serde(flatten)]
    #[garde(dive)]
    pub options: DuckDBOptions,
    /// Compiler-produced (compiled config only). When set, the connector reads
    /// the warehouse data from S3 instead of the local path, so the stateless
    /// fleet can serve a workspace whose DuckDB data lives in the working tree.
    /// Always `None` in `config.yml` — see [`DuckDbS3Mirror`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[garde(skip)]
    pub s3_mirror: Option<DuckDbS3Mirror>,
}

#[derive(Serialize, Deserialize, Debug, Validate, Clone, JsonSchema)]
#[garde(context(ValidationContext))]
#[serde(untagged)] // Consider using tagged enum here if migrations are possible
pub enum SnowflakeAuthType {
    Password {
        #[garde(length(min = 1))]
        password: String,
    },
    PasswordVar {
        #[garde(length(min = 1))]
        password_var: String,
    },
    PrivateKey {
        #[garde(custom(validate_file_path))]
        private_key_path: PathBuf,
    },
    BrowserAuth {
        #[serde(default = "default_snowflake_browser_timeout")]
        #[garde(skip)]
        browser_timeout_secs: u64, // in seconds
        #[garde(skip)]
        cache_dir: Option<PathBuf>,
    },
}

pub fn default_snowflake_browser_timeout() -> u64 {
    120
}

impl SnowflakeAuthType {
    pub fn get_password(&self) -> Option<&String> {
        match self {
            SnowflakeAuthType::Password { password, .. } => Some(password),
            _ => None,
        }
    }

    pub fn get_password_var(&self) -> Option<&String> {
        match self {
            SnowflakeAuthType::PasswordVar { password_var, .. } => Some(password_var),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Validate, Clone, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct Snowflake {
    #[garde(skip)]
    pub account: String,
    #[garde(skip)]
    pub username: String,
    #[garde(skip)]
    pub warehouse: String,
    #[garde(skip)]
    pub database: String,
    #[garde(skip)]
    pub schema: Option<String>,
    #[garde(skip)]
    pub role: Option<String>,
    #[serde(flatten)]
    #[garde(dive)]
    pub auth_type: SnowflakeAuthType,
    #[garde(skip)]
    #[serde(default)]
    pub datasets: HashMap<String, Vec<String>>,
    #[serde(default)]
    #[garde(skip)]
    pub filters: HashMap<String, schemars::schema::SchemaObject>,
}

impl Snowflake {
    pub async fn get_password(&self, secret_manager: &SecretsManager) -> Result<String, OxyError> {
        secret_manager
            .resolve_config_value(
                self.auth_type.get_password().map(|x| x.as_str()),
                self.auth_type.get_password_var().map(|x| x.as_str()),
                "Snowflake password",
                None,
            )
            .await
    }
}

#[derive(Serialize, Deserialize, Debug, Validate, Clone, JsonSchema)]
#[garde(context(ValidationContext))]
#[serde(tag = "type")]
pub enum DatabaseType {
    #[serde(rename = "bigquery")]
    Bigquery(#[garde(dive)] BigQuery),
    #[serde(rename = "duckdb")]
    DuckDB(#[garde(dive)] DuckDB),
    #[serde(rename = "snowflake")]
    Snowflake(#[garde(dive)] Snowflake),
    #[serde(rename = "postgres")]
    Postgres(#[garde(dive)] Postgres),
    #[serde(rename = "airhouse")]
    Airhouse(#[garde(dive)] Airhouse),
    /// Airhouse with credentials managed by oxy. The connector layer resolves
    /// host, port, dbname, user, and password from the caller's
    /// `airhouse_users` row + `org_secrets` entry — populated by the
    /// per-user provisioning flow (Settings → Airhouse). No fields needed
    /// on this variant; everything is sourced from oxy's database.
    #[serde(rename = "airhouse_managed")]
    AirhouseManaged(#[garde(skip)] AirhouseManaged),
    /// Per-org OLTP Postgres with credentials managed by oxy. The connector
    /// resolves host, port, dbname, user, and password from the caller's
    /// workspace → org → `oltp_tenants` row.
    ///
    /// Always resolves the **read-only analyst** role, whatever the caller's
    /// `effective_role` — unlike `airhouse_managed`, which escalates. That
    /// database holds a customer's live business records; writes come only
    /// from published code (an app function or an Airway pipeline), never from
    /// a SQL prompt. No fields: everything is sourced from oxy's database.
    #[serde(rename = "postgres_managed")]
    PostgresManaged(#[garde(skip)] PostgresManaged),
    #[serde(rename = "redshift")]
    Redshift(#[garde(dive)] Redshift),
    #[serde(rename = "mysql")]
    Mysql(#[garde(dive)] Mysql),
    #[serde(rename = "clickhouse")]
    ClickHouse(#[garde(dive)] ClickHouse),
    #[serde(rename = "domo")]
    DOMO(#[garde(dive)] DOMO),
    #[serde(rename = "motherduck")]
    MotherDuck(#[garde(dive)] MotherDuck),
}

impl std::fmt::Display for DatabaseType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DatabaseType::Bigquery(_) => write!(f, "bigquery"),
            DatabaseType::DuckDB(_) => write!(f, "duckdb"),
            DatabaseType::Snowflake(_) => write!(f, "snowflake"),
            DatabaseType::Postgres(_) => write!(f, "postgres"),
            DatabaseType::Airhouse(_) => write!(f, "airhouse"),
            DatabaseType::AirhouseManaged(_) => write!(f, "airhouse_managed"),
            DatabaseType::PostgresManaged(_) => write!(f, "postgres_managed"),
            DatabaseType::Redshift(_) => write!(f, "redshift"),
            DatabaseType::Mysql(_) => write!(f, "mysql"),
            DatabaseType::ClickHouse(_) => write!(f, "clickhouse"),
            DatabaseType::DOMO(_) => write!(f, "domo"),
            DatabaseType::MotherDuck(_) => write!(f, "motherduck"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Validate, Clone, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct Database {
    #[garde(length(min = 1))]
    pub name: String,

    #[serde(flatten)]
    #[garde(dive)]
    pub database_type: DatabaseType,
}

impl Database {
    pub fn database_type_name(&self) -> &'static str {
        match &self.database_type {
            DatabaseType::Bigquery(_) => "bigquery",
            DatabaseType::DuckDB(_) => "duckdb",
            DatabaseType::Snowflake(_) => "snowflake",
            DatabaseType::Postgres(_) => "postgres",
            DatabaseType::Redshift(_) => "redshift",
            DatabaseType::Mysql(_) => "mysql",
            DatabaseType::ClickHouse(_) => "clickhouse",
            DatabaseType::DOMO(_) => "domo",
            DatabaseType::MotherDuck(_) => "motherduck",
            DatabaseType::Airhouse(_) => "airhouse",
            DatabaseType::AirhouseManaged(_) => "airhouse_managed",
            DatabaseType::PostgresManaged(_) => "postgres_managed",
        }
    }

    pub fn dialect(&self) -> String {
        match &self.database_type {
            DatabaseType::Bigquery(_) => "bigquery".to_owned(),
            DatabaseType::DuckDB(_) => "duckdb".to_owned(),
            DatabaseType::Postgres(_) => "postgres".to_owned(),
            DatabaseType::Airhouse(_) => "duckdb".to_owned(),
            DatabaseType::AirhouseManaged(_) => "duckdb".to_owned(),
            // Real Postgres, unlike airhouse's DuckDB-over-pgwire.
            DatabaseType::PostgresManaged(_) => "postgres".to_owned(),
            DatabaseType::Redshift(_) => "postgres".to_owned(),
            DatabaseType::Mysql(_) => "mysql".to_owned(),
            DatabaseType::ClickHouse(_) => "clickhouse".to_string(),
            DatabaseType::Snowflake(_) => "snowflake".to_string(),
            DatabaseType::DOMO(_) => "domo".to_string(),
            DatabaseType::MotherDuck(_) => "duckdb".to_string(),
        }
    }

    pub fn datasets(&self) -> HashMap<String, Vec<String>> {
        match &self.database_type {
            DatabaseType::Bigquery(bq) => match (bq.dataset.is_some(), bq.datasets.is_empty()) {
                (true, _) => HashMap::from_iter([(
                    bq.dataset.clone().unwrap().to_string(),
                    vec!["*".to_string()],
                )]),
                (false, false) => bq.datasets.clone(),
                (false, true) => {
                    HashMap::from_iter([("`region-us`".to_string(), vec!["*".to_string()])])
                }
            },
            DatabaseType::ClickHouse(ch) => {
                if ch.schemas.is_empty() {
                    HashMap::from_iter([(String::default(), vec!["*".to_string()])])
                } else {
                    ch.schemas.clone()
                }
            }
            DatabaseType::Snowflake(sf) => {
                if sf.datasets.is_empty() {
                    // Empty key signals "all user schemas" — see Snowflake branch in loader::get_schemas_queries
                    HashMap::from_iter([("".to_string(), vec!["*".to_string()])])
                } else {
                    sf.datasets.clone()
                }
            }
            DatabaseType::MotherDuck(md) => {
                if md.schemas.is_empty() {
                    HashMap::from_iter([(String::default(), vec!["*".to_string()])])
                } else {
                    md.schemas.clone()
                }
            }
            _ => Default::default(),
        }
    }

    pub fn with_datasets(self, datasets: Vec<String>) -> Self {
        if datasets.is_empty() {
            return self;
        }

        match &self.database_type {
            DatabaseType::Bigquery(bq) => {
                let mut datasets_map = HashMap::new();
                for dataset in datasets {
                    let tables = bq.datasets.get(&dataset).cloned();
                    datasets_map.insert(dataset, tables.unwrap_or(vec!["*".to_string()]));
                }
                Database {
                    database_type: DatabaseType::Bigquery(BigQuery {
                        datasets: datasets_map,
                        ..bq.clone()
                    }),
                    ..self
                }
            }
            DatabaseType::ClickHouse(ch) => {
                let mut datasets_map = HashMap::new();
                for dataset in datasets {
                    let tables = ch.schemas.get(&dataset).cloned();
                    datasets_map.insert(dataset, tables.unwrap_or(vec!["*".to_string()]));
                }
                Database {
                    database_type: DatabaseType::ClickHouse(ClickHouse {
                        schemas: datasets_map,
                        ..ch.clone()
                    }),
                    ..self
                }
            }
            _ => self,
        }
    }

    /// Filter sync to specific tables within their schemas.
    /// Accepts a map of schema name → table names.
    pub fn with_schema_tables(self, schema_tables: HashMap<String, Vec<String>>) -> Self {
        if schema_tables.is_empty() {
            return self;
        }

        match &self.database_type {
            DatabaseType::Bigquery(bq) => Database {
                database_type: DatabaseType::Bigquery(BigQuery {
                    datasets: schema_tables,
                    ..bq.clone()
                }),
                ..self
            },
            DatabaseType::ClickHouse(ch) => Database {
                database_type: DatabaseType::ClickHouse(ClickHouse {
                    schemas: schema_tables,
                    ..ch.clone()
                }),
                ..self
            },
            DatabaseType::Snowflake(sf) => Database {
                database_type: DatabaseType::Snowflake(Snowflake {
                    datasets: schema_tables,
                    ..sf.clone()
                }),
                ..self
            },
            DatabaseType::MotherDuck(md) => Database {
                database_type: DatabaseType::MotherDuck(MotherDuck {
                    schemas: schema_tables,
                    ..md.clone()
                }),
                ..self
            },
            _ => self,
        }
    }
}

/// ClickHouse-specific connection override parameters
///
/// Allows overriding ClickHouse connection parameters at request time.
/// Used primarily by third-party API consumers to dynamically modify connection settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct ClickHouseConnectionOverride {
    /// Override the ClickHouse host/URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,

    /// Override the database name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
}

/// Snowflake-specific connection override parameters
///
/// Allows overriding Snowflake connection parameters at request time.
/// Used primarily by third-party API consumers to dynamically modify connection settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct SnowflakeConnectionOverride {
    /// Override the database name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,

    /// Override the schema
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    /// Override the warehouse
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warehouse: Option<String>,

    /// Override the account identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// Database-specific connection override
///
/// Different databases support different override parameters.
/// The connector will deserialize to the appropriate variant based on the database type.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(untagged)]
pub enum ConnectionOverride {
    ClickHouse(ClickHouseConnectionOverride),
    Snowflake(SnowflakeConnectionOverride),
}

impl TryFrom<ConnectionOverride> for ClickHouseConnectionOverride {
    type Error = oxy_shared::errors::OxyError;
    fn try_from(ovr: ConnectionOverride) -> Result<Self, Self::Error> {
        let ConnectionOverride::ClickHouse(ch) = ovr else {
            return Err(oxy_shared::errors::OxyError::ConfigurationError(
                "Invalid override type for ClickHouse".into(),
            ));
        };
        Ok(ch)
    }
}

impl TryFrom<ConnectionOverride> for SnowflakeConnectionOverride {
    type Error = oxy_shared::errors::OxyError;
    fn try_from(ovr: ConnectionOverride) -> Result<Self, Self::Error> {
        let ConnectionOverride::Snowflake(sf) = ovr else {
            return Err(oxy_shared::errors::OxyError::ConfigurationError(
                "Invalid override type for Snowflake".into(),
            ));
        };
        Ok(sf)
    }
}

/// Map of database name to connection overrides
///
/// Keys should match database names defined in config.yml under the `databases` section.
/// This allows API requests to override connection parameters for specific databases
/// without modifying the base configuration.
///
/// The override structure depends on the database type - the connector will automatically
/// deserialize to the correct variant based on the database configuration.
pub type ConnectionOverrides = HashMap<String, ConnectionOverride>;

fn validate_models(models: &Vec<Model>, ctx: &ValidationContext) -> garde::Result {
    for (i, model) in models.iter().enumerate() {
        match model {
            Model::OpenAI { config } => {
                if config.name.is_empty() {
                    return Err(garde::Error::new(format!(
                        "models[{}].name: length is lower than 1",
                        i
                    )));
                }
                if config.model_ref.is_empty() {
                    return Err(garde::Error::new(format!(
                        "models[{}].model_ref: length is lower than 1",
                        i
                    )));
                }
                validate_env_var(&config.key_var, ctx)
                    .map_err(|e| garde::Error::new(format!("models[{}].key_var: {}", i, e)))?;
            }
            Model::Google { config } => {
                if config.name.is_empty() {
                    return Err(garde::Error::new(format!(
                        "models[{}].name: length is lower than 1",
                        i
                    )));
                }
                if config.model_ref.is_empty() {
                    return Err(garde::Error::new(format!(
                        "models[{}].model_ref: length is lower than 1",
                        i
                    )));
                }
                validate_env_var(&config.key_var, ctx)
                    .map_err(|e| garde::Error::new(format!("models[{}].key_var: {}", i, e)))?;
            }
            Model::Ollama { config } => {
                if config.name.is_empty() {
                    return Err(garde::Error::new(format!(
                        "models[{}].name: length is lower than 1",
                        i
                    )));
                }
                if config.model_ref.is_empty() {
                    return Err(garde::Error::new(format!(
                        "models[{}].model_ref: length is lower than 1",
                        i
                    )));
                }
                if config.api_key.is_empty() {
                    return Err(garde::Error::new(format!(
                        "models[{}].api_key: length is lower than 1",
                        i
                    )));
                }
                if config.api_url.is_empty() {
                    return Err(garde::Error::new(format!(
                        "models[{}].api_url: length is lower than 1",
                        i
                    )));
                }
            }
            Model::Anthropic { config } => {
                if config.name.is_empty() {
                    return Err(garde::Error::new(format!(
                        "models[{}].name: length is lower than 1",
                        i
                    )));
                }
                if config.model_ref.is_empty() {
                    return Err(garde::Error::new(format!(
                        "models[{}].model_ref: length is lower than 1",
                        i
                    )));
                }
                validate_env_var(&config.key_var, ctx)
                    .map_err(|e| garde::Error::new(format!("models[{}].key_var: {}", i, e)))?;
            }
            Model::OpenAICompat { config } => {
                if config.name.is_empty() {
                    return Err(garde::Error::new(format!(
                        "models[{}].name: length is lower than 1",
                        i
                    )));
                }
                if config.model_ref.is_empty() {
                    return Err(garde::Error::new(format!(
                        "models[{}].model_ref: length is lower than 1",
                        i
                    )));
                }
                validate_env_var(&config.key_var, ctx)
                    .map_err(|e| garde::Error::new(format!("models[{}].key_var: {}", i, e)))?;
                // Unlike `openai`, there is no sensible default host for a
                // compat gateway — the whole point is that it lives elsewhere.
                // `api_url` defaults to OpenAI's own base via serde, which would
                // silently send traffic to api.openai.com, so require it.
                match config.api_url.as_deref() {
                    Some(url) if url != OPENAI_API_URL && !url.is_empty() => {}
                    _ => {
                        return Err(garde::Error::new(format!(
                            "models[{}].api_url: openai_compat requires an explicit api_url \
                             (e.g. https://api.langdock.com/openai/eu/v1); use vendor: openai \
                             to talk to api.openai.com",
                            i
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone)]
#[serde(tag = "type")]
pub enum AnonymizerConfig {
    #[serde(rename = "flash_text")]
    FlashText {
        #[serde(flatten)]
        source: FlashTextSourceType,
        #[serde(default = "default_anonymizer_pluralize")]
        pluralize: bool,
        #[serde(default = "default_case_sensitive")]
        case_sensitive: bool,
    },
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone)]
#[serde(untagged)]
pub enum FlashTextSourceType {
    Keywords {
        keywords_file: PathBuf,
        #[serde(default = "default_anonymizer_replacement")]
        replacement: String,
    },
    Mapping {
        mapping_file: PathBuf,
        #[serde(default = "default_delimiter")]
        delimiter: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, JsonSchema)]
pub enum FileFormat {
    #[serde(rename = "json")]
    Json,
    #[serde(rename = "markdown")]
    #[default]
    Markdown,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct AgentTask {
    #[garde(length(min = 1))]
    pub prompt: String,
    #[garde(skip)]
    pub agent_ref: String,
    #[serde(default = "default_retry")]
    #[garde(skip)]
    pub retry: usize,

    #[serde(default = "default_consistency_run")]
    #[garde(skip)]
    pub consistency_run: usize,

    #[garde(skip)]
    pub variables: Option<HashMap<String, Value>>,

    /// Custom consistency evaluation prompt for this specific task
    /// Overrides automation-level consistency_prompt if specified
    #[garde(custom(validate_consistency_prompt))]
    pub consistency_prompt: Option<String>,

    #[garde(dive)]
    pub export: Option<TaskExport>,
}

impl Hash for AgentTask {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.agent_ref.hash(state);
        self.prompt.hash(state);
        if let Some(ref vars) = self.variables {
            for (key, value) in vars.iter().sorted_by_cached_key(|(key, _)| *key) {
                key.hash(state);
                value.hash(state);
            }
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Clone, Debug, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub enum ExportFormat {
    #[serde(rename = "sql")]
    SQL,
    #[serde(rename = "csv")]
    CSV,
    #[serde(rename = "json")]
    JSON,
    #[serde(rename = "txt")]
    TXT,
    #[serde(rename = "docx")]
    DOCX,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct TaskExport {
    #[garde(length(min = 1))]
    pub path: String,
    #[garde(dive)]
    pub format: ExportFormat,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct TaskCache {
    #[serde(default = "default_cache_enabled")]
    #[garde(skip)]
    pub enabled: bool,
    #[garde(length(min = 1))]
    pub path: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema, Hash)]
#[garde(context(ValidationContext))]
#[serde(untagged)]
pub enum SQL {
    File {
        #[garde(length(min = 1))]
        sql_file: String,
    },
    Query {
        #[garde(length(min = 1))]
        sql_query: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct ExecuteSQLTask {
    #[garde(custom(validate_database_exists))]
    pub database: String,
    #[garde(dive)]
    #[serde(flatten)]
    pub sql: SQL,
    #[serde(default)]
    #[garde(skip)]
    pub variables: Option<HashMap<String, String>>,

    #[garde(dive)]
    pub export: Option<TaskExport>,

    #[garde(range(min = 1))]
    pub dry_run_limit: Option<u64>,
}

impl Hash for ExecuteSQLTask {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.database.hash(state);
        self.sql.hash(state);
        if let Some(ref vars) = self.variables {
            for (key, value) in vars.iter().sorted() {
                key.hash(state);
                value.hash(state);
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct SemanticQueryTask {
    // TODO: validate
    #[garde(skip)]
    #[serde(flatten)]
    pub query: SemanticQueryParams,

    // Optional export configuration (reuses existing task export logic)
    #[garde(dive)]
    pub export: Option<TaskExport>,

    // Optional variables for semantic model expressions
    #[garde(skip)]
    #[serde(default)]
    pub variables: Option<HashMap<String, Value>>,
}

impl Hash for SemanticQueryTask {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.query.hash(state);
        // Variables affect query results, so include them in hash
        if let Some(variables) = &self.variables {
            for (key, value) in variables {
                key.hash(state);
                value.to_string().hash(state); // Hash the JSON string representation
            }
        }
        // Export options don't affect semantic equivalence for caching
    }
}

// Supporting Enums & Structs
#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema, Hash, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SemanticOrderDirection {
    Asc,
    Desc,
}

// Custom schema functions for JSON values to ensure OpenAI compatibility
fn json_value_schema(_gen: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
    use schemars::schema::{InstanceType, Schema, SchemaObject};
    Schema::Object(SchemaObject {
        instance_type: Some(
            vec![
                InstanceType::String,
                InstanceType::Number,
                InstanceType::Boolean,
                InstanceType::Null,
            ]
            .into(),
        ),
        ..Default::default()
    })
}

fn json_value_array_schema(
    _gen: &mut schemars::r#gen::SchemaGenerator,
) -> schemars::schema::Schema {
    use schemars::schema::{ArrayValidation, InstanceType, Schema, SchemaObject, SingleOrVec};
    Schema::Object(SchemaObject {
        instance_type: Some(vec![InstanceType::Array].into()),
        array: Some(Box::new(ArrayValidation {
            items: Some(SingleOrVec::Single(Box::new(Schema::Object(
                SchemaObject {
                    instance_type: Some(
                        vec![
                            InstanceType::String,
                            InstanceType::Number,
                            InstanceType::Boolean,
                            InstanceType::Null,
                        ]
                        .into(),
                    ),
                    ..Default::default()
                },
            )))),
            ..Default::default()
        })),
        ..Default::default()
    })
}

/// Scalar comparison filter (eq, neq, gt, gte, lt, lte)
#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema, ToSchema)]
#[garde(context(ValidationContext))]
pub struct ScalarFilter {
    #[garde(skip)]
    #[schemars(
        schema_with = "json_value_schema",
        description = "The value to compare. Can be a string, number, boolean, or null."
    )]
    pub value: Value,
}

/// Array-based filter (in, not_in)
#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema, ToSchema)]
#[garde(context(ValidationContext))]
pub struct ArrayFilter {
    #[garde(skip)]
    #[schemars(
        schema_with = "json_value_array_schema",
        description = "Array of values to filter by. Each value can be a string, number, boolean, or null."
    )]
    pub values: Vec<Value>,
}

/// Date range filter (in_date_range, not_in_date_range)
#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema, ToSchema)]
#[garde(context(ValidationContext))]
pub struct DateRangeFilter {
    #[garde(skip)]
    #[schemars(
        schema_with = "json_value_schema",
        description = "Start of the date range. Can be a string (ISO date or relative like 'today', '7 days ago'), number (timestamp)."
    )]
    pub from: Value,
    #[garde(skip)]
    #[schemars(
        schema_with = "json_value_schema",
        description = "End of the date range. Can be a string (ISO date or relative like 'today', '7 days ago'), number (timestamp)."
    )]
    pub to: Value,
}

impl DateRangeFilter {
    #[allow(dead_code)]
    fn resolve_date_value(value: &Value) -> Result<Value, oxy_shared::errors::OxyError> {
        match value {
            Value::String(s) => {
                let resolved = Self::parse_relative_date(s)?;
                Ok(Value::String(resolved))
            }
            other => Ok(other.clone()),
        }
    }

    #[allow(dead_code)]
    fn parse_relative_date(expr: &str) -> Result<String, oxy_shared::errors::OxyError> {
        use chrono::Utc;

        // Try parsing with chrono-english for natural language dates
        match chrono_english::parse_date_string(expr, Utc::now(), chrono_english::Dialect::Us) {
            Ok(datetime) => Ok(datetime.to_rfc3339()),
            Err(_) => {
                // If chrono-english can't parse it, assume it's already a valid datetime string
                // and return it as-is
                Ok(expr.to_string())
            }
        }
    }
}

/// Enum representing different filter types with their appropriate value types
#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema, ToSchema)]
#[garde(context(ValidationContext))]
#[serde(tag = "op")]
pub enum SemanticFilterType {
    #[serde(rename = "eq")]
    Eq(#[garde(dive)] ScalarFilter),
    #[serde(rename = "neq")]
    Neq(#[garde(dive)] ScalarFilter),
    #[serde(rename = "gt")]
    Gt(#[garde(dive)] ScalarFilter),
    #[serde(rename = "gte")]
    Gte(#[garde(dive)] ScalarFilter),
    #[serde(rename = "lt")]
    Lt(#[garde(dive)] ScalarFilter),
    #[serde(rename = "lte")]
    Lte(#[garde(dive)] ScalarFilter),
    #[serde(rename = "in")]
    In(#[garde(dive)] ArrayFilter),
    #[serde(rename = "not_in")]
    NotIn(#[garde(dive)] ArrayFilter),
    #[serde(rename = "in_date_range")]
    InDateRange(#[garde(dive)] DateRangeFilter),
    #[serde(rename = "not_in_date_range")]
    NotInDateRange(#[garde(dive)] DateRangeFilter),
}

impl Hash for SemanticFilterType {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        // Then hash the value(s)
        match self {
            SemanticFilterType::Eq(f)
            | SemanticFilterType::Neq(f)
            | SemanticFilterType::Gt(f)
            | SemanticFilterType::Gte(f)
            | SemanticFilterType::Lt(f)
            | SemanticFilterType::Lte(f) => {
                if let Ok(s) = serde_json::to_string(&f.value) {
                    s.hash(state);
                }
            }
            SemanticFilterType::In(f) | SemanticFilterType::NotIn(f) => {
                for v in &f.values {
                    if let Ok(s) = serde_json::to_string(v) {
                        s.hash(state);
                    }
                }
            }
            SemanticFilterType::InDateRange(f) | SemanticFilterType::NotInDateRange(f) => {
                if let Ok(s) = serde_json::to_string(&f.from) {
                    s.hash(state);
                }
                if let Ok(s) = serde_json::to_string(&f.to) {
                    s.hash(state);
                }
            }
        }
    }
}

impl PartialEq for SemanticFilterType {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (SemanticFilterType::Eq(a), SemanticFilterType::Eq(b))
            | (SemanticFilterType::Neq(a), SemanticFilterType::Neq(b))
            | (SemanticFilterType::Gt(a), SemanticFilterType::Gt(b))
            | (SemanticFilterType::Gte(a), SemanticFilterType::Gte(b))
            | (SemanticFilterType::Lt(a), SemanticFilterType::Lt(b))
            | (SemanticFilterType::Lte(a), SemanticFilterType::Lte(b)) => {
                serde_json::to_string(&a.value).ok() == serde_json::to_string(&b.value).ok()
            }
            (SemanticFilterType::In(a), SemanticFilterType::In(b))
            | (SemanticFilterType::NotIn(a), SemanticFilterType::NotIn(b)) => {
                a.values.len() == b.values.len()
                    && a.values.iter().zip(b.values.iter()).all(|(x, y)| {
                        serde_json::to_string(x).ok() == serde_json::to_string(y).ok()
                    })
            }
            (SemanticFilterType::InDateRange(a), SemanticFilterType::InDateRange(b))
            | (SemanticFilterType::NotInDateRange(a), SemanticFilterType::NotInDateRange(b)) => {
                serde_json::to_string(&a.from).ok() == serde_json::to_string(&b.from).ok()
                    && serde_json::to_string(&a.to).ok() == serde_json::to_string(&b.to).ok()
            }
            _ => false,
        }
    }
}

impl Eq for SemanticFilterType {}

impl SemanticFilterType {
    /// Get the filter values as a `Vec<Value>`
    pub fn values(&self) -> Vec<Value> {
        match self {
            SemanticFilterType::Eq(f)
            | SemanticFilterType::Neq(f)
            | SemanticFilterType::Gt(f)
            | SemanticFilterType::Gte(f)
            | SemanticFilterType::Lt(f)
            | SemanticFilterType::Lte(f) => vec![f.value.clone()],
            SemanticFilterType::In(f) | SemanticFilterType::NotIn(f) => f.values.clone(),
            SemanticFilterType::InDateRange(f) | SemanticFilterType::NotInDateRange(f) => {
                vec![f.from.clone(), f.to.clone()]
            }
        }
    }

    /// Check if this filter type requires array values
    pub fn requires_array(&self) -> bool {
        matches!(
            self,
            SemanticFilterType::In(_)
                | SemanticFilterType::NotIn(_)
                | SemanticFilterType::InDateRange(_)
                | SemanticFilterType::NotInDateRange(_)
        )
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, ToSchema)]
#[garde(context(ValidationContext))]
pub struct SemanticFilter {
    #[garde(length(min = 1))]
    pub field: String,
    #[serde(flatten)]
    #[garde(dive)]
    pub filter_type: SemanticFilterType,
}

// Custom JSON schema implementation to flatten filter_type variants
// That produce JSON schema compatible with OpenAI
impl JsonSchema for SemanticFilter {
    fn schema_name() -> String {
        "SemanticFilter".to_string()
    }

    fn json_schema(r#gen: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        use schemars::schema::{InstanceType, Metadata, Schema, SchemaObject, SubschemaValidation};

        // Generate the full schema for SemanticFilterType first
        // This will add it to the definitions and return a reference
        let _filter_ref = r#gen.subschema_for::<SemanticFilterType>();

        let definitions = r#gen.definitions();
        let filter_type_schema = definitions.get("SemanticFilterType").cloned();

        // Use anyOf to combine the field with filter_type variants
        // Since filter_type is flattened, we need to merge it at the top level
        let mut subschemas = Vec::new();

        // Extract the oneOf variants from SemanticFilterType and convert to anyOf
        if let Some(Schema::Object(filter_obj)) = filter_type_schema
            && let Some(subschema_validation) = &filter_obj.subschemas
            && let Some(one_of) = &subschema_validation.one_of
        {
            // For each variant in oneOf, create an anyOf schema that includes the field property
            for variant in one_of {
                let mut combined = SchemaObject::default();
                combined.instance_type = Some(InstanceType::Object.into());

                // Add field property to each variant
                let mut field_schema_clone = SchemaObject::default();
                field_schema_clone.instance_type = Some(InstanceType::String.into());
                field_schema_clone.metadata = Some(Box::new(Metadata {
                            description: Some("The measure/dimension to apply the filter on. Must by full name: <view_name>.<field_name>".to_string()),
                            ..Default::default()
                        }));

                combined
                    .object()
                    .properties
                    .insert("field".to_string(), Schema::Object(field_schema_clone));
                combined.object().required.insert("field".to_string());

                // Merge the filter_type variant properties
                if let Schema::Object(variant_obj) = variant
                    && let Some(props) = &variant_obj.object
                {
                    for (key, value) in &props.properties {
                        combined
                            .object()
                            .properties
                            .insert(key.clone(), value.clone());
                    }
                    for req in &props.required {
                        combined.object().required.insert(req.clone());
                    }
                }

                subschemas.push(Schema::Object(combined));
            }
        }

        // Return a schema with anyOf at the top level
        let mut schema = SchemaObject::default();
        schema.subschemas = Some(Box::new(SubschemaValidation {
            any_of: Some(subschemas),
            ..Default::default()
        }));

        Schema::Object(schema)
    }
}

impl Hash for SemanticFilter {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.field.hash(state);
        self.filter_type.hash(state);
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct SemanticOrder {
    #[garde(length(min = 1))]
    pub field: String,
    #[serde(default = "default_order_direction")]
    #[garde(skip)]
    pub direction: SemanticOrderDirection,
}

impl Hash for SemanticOrder {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.field.hash(state);
        self.direction.hash(state);
    }
}

fn default_order_direction() -> SemanticOrderDirection {
    SemanticOrderDirection::Asc
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct FormatterTask {
    #[garde(length(min = 1))]
    pub template: String,
    #[garde(dive)]
    pub export: Option<TaskExport>,
}

impl Hash for FormatterTask {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.template.hash(state);
    }
}

fn default_http_method() -> String {
    "POST".to_string()
}

fn default_http_timeout_secs() -> u64 {
    30
}

/// Persist a field of an `http_request` JSON response back into a project secret.
///
/// This is the rotating-OAuth-token path: the QuickBooks token-refresh response
/// returns a new `refresh_token`, which we write back to the secret store so the
/// next run starts from the latest value (mirrors the Airway Intuit source).
#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema, Hash)]
#[garde(context(ValidationContext))]
pub struct PersistToSecret {
    /// JSON pointer into the parsed response body, e.g. `/refresh_token`.
    #[garde(length(min = 1))]
    pub from: String,
    /// Project secret name to upsert with the extracted value.
    #[garde(length(min = 1))]
    pub name: String,
}

/// `type: http_request` — make an outbound HTTP request from a workflow.
///
/// The reusable primitive behind the QuickBooks JE-posting automation (and the
/// stop-gap for Oxy Functions' `ctx.fetch`). Templated fields render against the
/// prior-step render context; `{{ secrets.NAME }}` resolves declared `secrets`.
/// Egress is constrained (private/loopback/link-local hosts are always denied;
/// `allow_hosts`, when set, is an additional allowlist).
#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct HttpRequestTask {
    /// Request URL (Jinja-templated).
    #[garde(length(min = 1))]
    pub url: String,
    /// HTTP method (GET/POST/PUT/PATCH/DELETE). Defaults to POST.
    #[serde(default = "default_http_method")]
    #[garde(skip)]
    pub method: String,
    /// Request headers; values are Jinja-templated and may reference `{{ secrets.X }}`.
    #[serde(default)]
    #[garde(skip)]
    pub headers: HashMap<String, String>,
    /// Raw request body (Jinja-templated). Mutually exclusive with `form`.
    #[serde(default)]
    #[garde(skip)]
    pub body: Option<String>,
    /// Form fields serialized as `application/x-www-form-urlencoded` (values templated).
    #[serde(default)]
    #[garde(skip)]
    pub form: Option<HashMap<String, String>>,
    /// Project secret names to resolve into the `{{ secrets.NAME }}` template scope.
    #[serde(default)]
    #[garde(skip)]
    pub secrets: Vec<String>,
    /// Request timeout in seconds (1–120). Defaults to 30.
    #[serde(default = "default_http_timeout_secs")]
    #[garde(range(min = 1, max = 120))]
    pub timeout_secs: u64,
    /// Accepted HTTP status codes. Empty → any 2xx is accepted; anything else fails the task.
    #[serde(default)]
    #[garde(skip)]
    pub expected_status: Vec<u16>,
    /// Allowed target hostnames (in addition to the always-on private-IP deny).
    /// Empty means "any public host"; set it to lock a task to specific APIs.
    #[serde(default)]
    #[garde(skip)]
    pub allow_hosts: Vec<String>,
    /// Optionally write a field of the JSON response back into a project secret.
    #[serde(default)]
    #[garde(dive)]
    pub persist_to_secret: Option<PersistToSecret>,
}

impl Hash for HttpRequestTask {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.url.hash(state);
        self.method.hash(state);
        for (k, v) in self.headers.iter().sorted_by_cached_key(|(k, _)| *k) {
            k.hash(state);
            v.hash(state);
        }
        self.body.hash(state);
        if let Some(ref form) = self.form {
            for (k, v) in form.iter().sorted_by_cached_key(|(k, _)| *k) {
                k.hash(state);
                v.hash(state);
            }
        }
        self.secrets.hash(state);
        self.timeout_secs.hash(state);
        self.expected_status.hash(state);
        self.allow_hosts.hash(state);
        self.persist_to_secret.hash(state);
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct SubAutomationTask {
    #[garde(skip)]
    pub src: PathBuf,
    #[garde(skip)]
    pub variables: Option<HashMap<String, Value>>,
    #[garde(dive)]
    pub export: Option<TaskExport>,
}

/// Back-compat alias: the sub-automation task was historically named `WorkflowTask`.
pub type WorkflowTask = SubAutomationTask;

impl Hash for SubAutomationTask {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.src.hash(state);
        if let Some(ref vars) = self.variables {
            for (key, value) in vars.iter().sorted_by_cached_key(|(key, _)| *key) {
                key.hash(state);
                value.hash(state);
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct OmniQueryTask {
    #[garde(custom(validate_omni_integration_exists))]
    pub integration: String,
    #[garde(length(min = 1))]
    pub topic: String,
    #[serde(flatten)]
    #[garde(skip)]
    pub query: crate::types::tool_params::OmniQueryParams,
    #[garde(dive)]
    pub export: Option<TaskExport>,
}

impl Hash for OmniQueryTask {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.integration.hash(state);
        self.topic.hash(state);
        self.query.hash(state);
    }
}

/// Task configuration for executing a Looker query within an automation.
#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct LookerQueryTask {
    /// Name of the Looker integration to use
    #[garde(custom(validate_looker_integration_exists))]
    pub integration: String,
    /// The LookML model name
    #[garde(length(min = 1))]
    pub model: String,
    /// The explore name within the model
    #[garde(length(min = 1))]
    pub explore: String,
    /// Query parameters
    #[serde(flatten)]
    #[garde(skip)]
    pub query: LookerQueryParams,
    /// Optional export configuration for query results
    #[garde(dive)]
    pub export: Option<TaskExport>,
}

impl Hash for LookerQueryTask {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.integration.hash(state);
        self.model.hash(state);
        self.explore.hash(state);
        self.query.hash(state);
    }
}

fn default_sort_direction() -> String {
    "asc".to_string()
}

/// A sort field for a Looker query with explicit field name and direction.
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, PartialEq, Eq, Hash)]
pub struct LookerSortField {
    pub field: String,
    #[serde(default = "default_sort_direction")]
    pub direction: String,
}

impl LookerSortField {
    pub fn to_looker_string(&self) -> String {
        if self.direction.eq_ignore_ascii_case("desc") {
            format!("{} desc", self.field)
        } else {
            format!("{} asc", self.field)
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, PartialEq, Eq)]
pub struct LookerQueryParams {
    /// List of field names to include in the results (e.g., "orders.id", "orders.total")
    #[schemars(
        description = "Fields to select. Field name must be full name format {view}.{field_name}."
    )]
    pub fields: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Filter conditions as field name to Looker filter expression mappings."
    )]
    pub filters: Option<HashMap<String, String>>,
    /// Looker filter expression for complex OR conditions
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Looker filter expression for complex conditions.")]
    pub filter_expression: Option<String>,
    /// List of fields to sort by as `field asc` or `field desc`.
    /// Input accepts objects with `field` and `direction` ("asc"/"desc").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Fields to sort by using Looker syntax: 'view.field asc' or 'view.field desc'."
    )]
    pub sorts: Option<Vec<LookerSortField>>,
    /// Maximum number of rows to return (-1 for unlimited)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Maximum number of rows to return.")]
    pub limit: Option<i64>,
}

impl Hash for LookerQueryParams {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.fields.hash(state);
        // Sort the hashmap keys to ensure consistent hashing
        if let Some(filters) = &self.filters {
            let mut sorted: Vec<_> = filters.iter().collect();
            sorted.sort_by_key(|(k, _)| *k);
            for (k, v) in sorted {
                k.hash(state);
                v.hash(state);
            }
        }
        self.filter_expression.hash(state);
        self.sorts.hash(state);
        self.limit.hash(state);
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Hash)]
#[serde(untagged)]
pub enum LoopValues {
    Template(String),
    Array(Vec<serde_json::Value>),
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct LoopSequentialTask {
    #[garde(skip)]
    pub values: LoopValues,
    #[garde(dive)]
    pub tasks: Vec<Task>,
    #[garde(skip)]
    #[serde(default = "default_loop_concurrency")]
    pub concurrency: usize,
}

impl Hash for LoopSequentialTask {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.values.hash(state);
        for task in &self.tasks {
            task.hash(state);
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema, Hash)]
#[garde(context(ValidationContext))]
pub struct Condition {
    #[garde(length(min = 1))]
    #[serde(rename = "if")]
    pub if_expr: String,
    #[garde(dive)]
    pub tasks: Vec<Task>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct ConditionalTask {
    #[garde(length(min = 1))]
    pub conditions: Vec<Condition>,
    #[garde(skip)]
    #[serde(default, rename = "else")]
    pub else_tasks: Option<Vec<Task>>,
}

impl Hash for ConditionalTask {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        for condition in &self.conditions {
            condition.if_expr.hash(state);
            for task in &condition.tasks {
                task.hash(state);
            }
        }
        if let Some(ref else_tasks) = self.else_tasks {
            for task in else_tasks {
                task.hash(state);
            }
        }
    }
}

/// Run an airway ELT pipeline as an automation step.
///
/// Unlike the other I/O task types this is **not** executed by
/// `step_executor` — it is delegated as a `TaskSpec::Airway`, reusing the
/// existing airway run path (secret resolution, Airhouse credential minting,
/// backfill windowing, run-scoped state). See `StepKind::Airway`.
///
/// A step completes only once the pipeline's end-of-load fold has committed,
/// so a following `execute_sql` step reads a queryable table rather than a
/// half-folded one.
#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema, Hash)]
#[garde(context(ValidationContext))]
pub struct AirwayTask {
    /// Workspace-relative path to the `.airway.yml` pipeline spec.
    #[garde(length(min = 1))]
    pub pipeline: String,

    /// Explicit subset of the spec's resources (tables) to run. Omitted or
    /// empty runs the whole pipeline.
    #[serde(default)]
    #[garde(skip)]
    pub resources: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema, Hash)]
#[garde(context(ValidationContext))]
#[serde(tag = "type")]
pub enum TaskType {
    #[serde(rename = "agent")]
    Agent(#[garde(dive)] AgentTask),
    #[serde(rename = "execute_sql")]
    ExecuteSQL(#[garde(dive)] ExecuteSQLTask),
    #[serde(rename = "semantic_query")]
    SemanticQuery(#[garde(dive)] SemanticQueryTask),
    #[serde(rename = "omni_query")]
    OmniQuery(#[garde(dive)] OmniQueryTask),
    #[serde(rename = "looker_query")]
    LookerQuery(#[garde(dive)] LookerQueryTask),
    #[serde(rename = "loop_sequential")]
    LoopSequential(#[garde(dive)] LoopSequentialTask),
    #[serde(rename = "formatter")]
    Formatter(#[garde(dive)] FormatterTask),
    // Wire tag stays `workflow` (sub-automation step); variant renamed to Automation term.
    #[serde(rename = "workflow")]
    SubAutomation(#[garde(dive)] SubAutomationTask),
    #[serde(rename = "conditional")]
    Conditional(#[garde(dive)] ConditionalTask),
    #[serde(rename = "http_request")]
    HttpRequest(#[garde(dive)] HttpRequestTask),
    #[serde(rename = "airway")]
    Airway(#[garde(dive)] AirwayTask),
    #[serde(other)]
    Unknown,
}

/// Where a task is executed when used inside an app with interactive controls.
/// `client` (default) — the frontend re-runs the SQL directly in DuckDB WASM on
/// every control change; no server round-trip required.
/// `server` — the server executes the task on every control change (required for
/// tasks that query external databases like Snowflake or BigQuery).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AppTaskMode {
    #[default]
    Client,
    Server,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct Task {
    #[schemars(
        description = "Unique name for the task within the workflow. Format: alphanumeric and underscores only, starting with a letter."
    )]
    #[garde(length(min = 1))]
    pub name: String,
    #[serde(flatten)]
    #[garde(dive)]
    #[garde(custom(validate_task))]
    pub task_type: TaskType,
    #[garde(dive)]
    pub cache: Option<TaskCache>,
    /// Execution mode when this task is used inside a data app. Defaults to `client`.
    #[serde(default)]
    #[garde(skip)]
    pub mode: AppTaskMode,
}

impl Task {
    pub fn kind(&self) -> &str {
        match &self.task_type {
            TaskType::Agent(_) => "agent",
            TaskType::ExecuteSQL(_) => "execute_sql",
            TaskType::SemanticQuery(_) => "semantic_query",
            TaskType::OmniQuery(_) => "omni_query",
            TaskType::LookerQuery(_) => "looker_query",
            TaskType::LoopSequential(_) => "loop",
            TaskType::Formatter(_) => "formatter",
            TaskType::SubAutomation(_) => "sub_workflow",
            TaskType::Conditional(_) => "conditional",
            TaskType::HttpRequest(_) => "http_request",
            TaskType::Airway(_) => "airway",
            TaskType::Unknown => "unknown",
        }
    }
}

impl Hash for Task {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.task_type.hash(state);
    }
}

#[derive(Serialize, Deserialize, Debug, Validate, JsonSchema, Clone)]
#[garde(context(ValidationContext))]
pub struct EvalConfig {
    #[garde(dive)]
    #[serde(flatten)]
    pub kind: EvalKind,
    #[garde(dive)]
    #[serde(default = "default_solvers")]
    pub metrics: Vec<SolverKind>,
    #[garde(skip)]
    #[serde(default = "default_consistency_concurrency")]
    pub concurrency: usize,
    #[garde(skip)]
    pub task_ref: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Validate, JsonSchema, Clone)]
#[serde(tag = "type")]
#[garde(context(ValidationContext))]
pub enum EvalKind {
    #[serde(rename = "consistency")]
    Consistency(#[garde(dive)] Consistency),
    #[serde(rename = "custom")]
    Custom(#[garde(dive)] Custom),
    #[serde(rename = "test_case")]
    TestCase(#[garde(skip)] TestCaseEval),
}

#[derive(Serialize, Deserialize, Debug, Validate, JsonSchema, Clone)]
#[garde(context(ValidationContext))]
pub struct Consistency {
    #[garde(skip)]
    #[serde(default = "default_n")]
    pub n: usize,
    #[garde(length(min = 1))]
    pub task_description: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Validate, JsonSchema, Clone)]
#[garde(context(ValidationContext))]
pub struct Custom {
    #[garde(length(min = 1))]
    pub dataset: String,
    #[garde(length(min = 1))]
    pub workflow_variable_name: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub is_context_id: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
pub struct TestCaseEval {
    pub cases: Vec<super::test_config::TestCase>,
    pub runs: usize,
    pub judge_model: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SolverKind {
    ContextRecall(#[garde(dive)] ContextRecallSolver),
    Similarity(#[garde(dive)] SimilaritySolver),
    Correctness(#[garde(dive)] CorrectnessSolver),
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[serde(tag = "distance")]
#[garde(context(ValidationContext))]
#[derive(Default)]
pub enum DistanceMethod {
    #[default]
    Levenshtein,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct ContextRecallSolver {
    #[serde(default)]
    #[garde(dive)]
    pub distance: DistanceMethod,
    #[garde(range(min = 0 as f32, max = 1_f32))]
    #[serde(default = "default_threshold")]
    pub threshold: f32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct SimilaritySolver {
    #[garde(length(min = 1))]
    #[serde(default = "default_consistency_prompt")]
    pub prompt: String,
    #[garde(length(min = 1))]
    pub model_ref: Option<String>,
    #[garde(skip)]
    #[serde(default = "default_scores")]
    pub scores: HashMap<String, f32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct CorrectnessSolver {
    #[garde(length(min = 1))]
    #[serde(default = "default_correctness_prompt")]
    pub prompt: String,
    #[garde(length(min = 1))]
    pub model_ref: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
#[garde(context(ValidationContext))]
pub struct Automation {
    /// Automation name. Accepted in YAML for documentation but always overwritten
    /// by the filename (e.g., `foo.automation.yml` -> name = "foo").
    #[serde(default)]
    #[schemars(skip)]
    #[garde(skip)]
    pub name: String,
    #[garde(length(min = 1))]
    #[garde(dive)]
    pub tasks: Vec<Task>,
    #[serde(flatten)]
    #[garde(skip)]
    pub variables: Option<Variables>,
    #[garde(skip)]
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    #[garde(dive)]
    pub retrieval: Option<RouteRetrievalConfig>,
    /// Global consistency evaluation prompt for all agent tasks in this automation.
    /// This can be overridden per-task via AgentTask.consistency_prompt
    #[garde(custom(validate_consistency_prompt))]
    pub consistency_prompt: Option<String>,
}

/// Back-compat alias: an automation was historically named `Workflow` (and before
/// that, a Procedure). The canonical type is now [`Automation`].
pub type Workflow = Automation;

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone)]
pub struct MarkdownDisplay {
    pub content: String,
}

/// How a numeric value is formatted for display.
///
/// Used on chart axes / tooltips and on individual table columns. When unset,
/// the renderer falls back to its default numeric formatting (trailing zeros
/// stripped for integers, two decimals for floats).
#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Copy, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DisplayFormat {
    /// Monetary values — e.g. `$301,397,792.46`. Uses USD by default.
    Currency,
    /// Percentage values — e.g. `12.5%`. Input is already a percentage
    /// (0–100), not a ratio.
    Percent,
    /// Plain number with thousands separators — e.g. `1,234,567`.
    Number,
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Validate, ToSchema)]
#[garde(context(ValidationContext))]
pub struct LineChartDisplay {
    #[garde(length(min = 1))]
    pub x: String,
    #[garde(length(min = 1))]
    pub y: String,
    #[garde(skip)]
    pub x_axis_label: Option<String>,
    #[garde(skip)]
    pub y_axis_label: Option<String>,
    #[garde(length(min = 1))]
    #[garde(custom(validate_task_data_reference))]
    #[schemars(description = "reference data output from a table using table name")]
    pub data: String,
    #[garde(skip)]
    pub series: Option<String>,
    #[garde(skip)]
    pub title: Option<String>,
    /// Optional formatting applied to the y-axis labels and tooltip values.
    /// Use `currency` for monetary measures. Unset renders raw numbers.
    #[garde(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_format: Option<DisplayFormat>,
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Validate, ToSchema)]
#[garde(context(ValidationContext))]
pub struct BarChartDisplay {
    #[garde(length(min = 1))]
    pub x: String,
    #[garde(length(min = 1))]
    pub y: String,
    #[garde(skip)]
    pub title: Option<String>,
    #[garde(length(min = 1))]
    #[garde(custom(validate_task_data_reference))]
    #[schemars(description = "reference data output from a table using table name")]
    pub data: String,
    #[garde(skip)]
    pub series: Option<String>,
    /// Optional formatting applied to the y-axis labels and tooltip values.
    /// Use `currency` for monetary measures. Unset renders raw numbers.
    #[garde(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_format: Option<DisplayFormat>,
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Validate, ToSchema)]
#[garde(context(ValidationContext))]
pub struct PieChartDisplay {
    #[garde(length(min = 1))]
    pub name: String,
    #[garde(length(min = 1))]
    pub value: String,
    #[garde(skip)]
    pub title: Option<String>,
    #[garde(length(min = 1))]
    #[garde(custom(validate_task_data_reference))]
    #[schemars(description = "reference data output from a table using table name")]
    pub data: String,
    /// Optional formatting applied to the slice value in the tooltip.
    /// Use `currency` for monetary measures. Unset renders raw numbers.
    #[garde(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_format: Option<DisplayFormat>,
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Validate)]
#[garde(context(ValidationContext))]
pub struct TableDisplay {
    #[garde(length(min = 1))]
    #[garde(custom(validate_task_data_reference))]
    pub data: String,
    #[garde(skip)]
    pub title: Option<String>,
    /// Optional per-column formatting. Keys are the output column names as
    /// they appear in the task result (e.g. `oxymart__total_weekly_sales`
    /// for semantic_query tasks, which join view + field with `__`). Columns
    /// omitted from the map fall back to the default numeric formatter.
    #[garde(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formats: Option<HashMap<String, DisplayFormat>>,
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Validate)]
#[serde(deny_unknown_fields)]
#[garde(context(ValidationContext))]
pub struct RowDisplay {
    /// Number of equal-width columns; defaults to the number of children.
    #[serde(default)]
    #[garde(custom(validate_row_columns))]
    pub columns: Option<u8>,
    /// Child display blocks rendered side-by-side in a grid row.
    #[garde(dive)]
    #[garde(length(min = 1))]
    pub children: Vec<Display>,
}

fn validate_row_columns(columns: &Option<u8>, _ctx: &ValidationContext) -> garde::Result {
    if columns == &Some(0) {
        return Err(garde::Error::new("columns must be at least 1"));
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Validate)]
#[serde(tag = "type")]
#[garde(context(ValidationContext))]
pub enum Display {
    #[serde(rename = "markdown")]
    Markdown(#[garde(skip)] MarkdownDisplay),
    #[serde(rename = "line_chart")]
    LineChart(#[garde(dive)] LineChartDisplay),
    #[serde(rename = "pie_chart")]
    PieChart(#[garde(dive)] PieChartDisplay),
    #[serde(rename = "bar_chart")]
    BarChart(#[garde(dive)] BarChartDisplay),
    #[serde(rename = "table")]
    Table(#[garde(dive)] TableDisplay),
    #[serde(rename = "row")]
    Row(#[garde(dive)] RowDisplay),
    /// A group of controls defined inline in the display list. The backend
    /// extracts these items into the controls array before sending to clients.
    #[serde(rename = "controls")]
    Controls(#[garde(skip)] ControlsDisplay),
    /// A single control defined inline in the display list. Because the Display
    /// enum uses "type" as its discriminant, the control kind is expressed as
    /// "control_type" instead of "type" to avoid a duplicate-key conflict:
    ///
    ///   display:
    ///     - type: control
    ///       name: region
    ///       control_type: select
    ///       label: Region
    ///       options: [All, North, South]
    #[serde(rename = "control")]
    Control(#[garde(skip)] SingleControlDisplay),
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone)]
pub struct ControlsDisplay {
    /// Interactive controls defined inline in the display list.
    pub items: Vec<ControlConfig>,
}

/// A single control defined inline in a `display:` list.
/// Uses `control_type` instead of `type` for the control kind because
/// the `type` key is already consumed by the Display enum discriminant.
#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone)]
pub struct SingleControlDisplay {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    pub control_type: ControlType,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub options: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

impl From<SingleControlDisplay> for ControlConfig {
    fn from(c: SingleControlDisplay) -> Self {
        ControlConfig {
            name: c.name,
            label: c.label,
            control_type: c.control_type,
            source: c.source,
            options: c.options,
            default: c.default,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone)]
#[serde(rename_all = "snake_case")]
pub enum ControlType {
    Select,
    Toggle,
    Date,
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Validate)]
#[garde(context(ValidationContext))]
pub struct ControlConfig {
    #[garde(length(min = 1))]
    pub name: String,
    #[serde(default)]
    #[garde(skip)]
    pub label: Option<String>,
    #[serde(rename = "type")]
    #[garde(skip)]
    pub control_type: ControlType,
    /// Task name whose first column populates dropdown options dynamically.
    #[serde(default)]
    #[garde(skip)]
    pub source: Option<String>,
    /// Static list of options (used when source is not set).
    #[serde(default)]
    #[garde(skip)]
    pub options: Option<Vec<serde_json::Value>>,
    /// Default value injected into Jinja context on initial load.
    #[serde(default)]
    #[garde(skip)]
    pub default: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Validate, Default)]
#[serde(deny_unknown_fields)]
#[garde(context(ValidationContext))]
pub struct AppConfig {
    /// App name. Accepted in YAML for documentation but the authoritative name
    /// is derived from the filename (e.g., `foo.app.yml` -> name = "foo").
    #[serde(default)]
    #[schemars(skip)]
    #[garde(skip)]
    pub name: String,
    /// Human-friendly title shown in dashboard listings. When unset, callers
    /// fall back to a humanized form of the filename-derived name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[garde(skip)]
    pub title: Option<String>,
    /// Optional description of the app.
    #[serde(default)]
    #[garde(skip)]
    pub description: String,
    /// Interactive controls (dropdowns, toggles, date pickers) whose values are
    /// injected into task Jinja templates as `{{ controls.<name> }}`.
    #[serde(default)]
    #[garde(dive)]
    pub controls: Vec<ControlConfig>,
    #[schemars(description = "tasks to prepare the data for the app")]
    #[garde(dive)]
    #[garde(length(min = 1))]
    pub tasks: Vec<Task>,
    #[schemars(description = "display blocks to render the app")]
    #[garde(length(min = 1))]
    #[garde(dive)]
    pub display: Vec<Display>,
    /// When false (default), the app is treated as a draft and hidden from the
    /// left sidebar. Authors can still open, edit, and run it from the IDE.
    /// Flip to true to make the app visible to consumers.
    #[serde(default)]
    #[schemars(default)]
    #[garde(skip)]
    pub published: bool,
}

fn default_anonymizer_replacement() -> String {
    "FLASH".to_string()
}

fn default_delimiter() -> String {
    ",".to_string()
}

fn default_anonymizer_pluralize() -> bool {
    false
}

fn default_case_sensitive() -> bool {
    false
}

fn default_retry() -> usize {
    1
}

fn default_consistency_run() -> usize {
    1
}

fn default_cache_enabled() -> bool {
    false
}

fn default_scores() -> HashMap<String, f32> {
    HashMap::from_iter([("A".to_string(), 1.0), ("B".to_string(), 0.0)])
}

fn default_n() -> usize {
    10
}

fn default_threshold() -> f32 {
    0.5
}

fn default_solvers() -> Vec<SolverKind> {
    vec![SolverKind::Similarity(SimilaritySolver {
        prompt: default_consistency_prompt(),
        model_ref: None,
        scores: default_scores(),
    })]
}

fn default_consistency_prompt() -> String {
    indoc! {"
    You are comparing a pair of submitted answers on a given question. Here is the data:
    [BEGIN DATA]
    ************
    [Question]: {{ task_description }}
    ************
    [Submission 1]: {{submission_1}}
    ************
    [Submission 2]: {{submission_2}}
    ************
    [END DATA]

    Compare the factual content of the submitted answers. Ignore any differences in style, grammar, punctuation. Answer the question by selecting one of the following options:
    A. The submitted answers are either a superset or contains each other and is fully consistent with it.
    B. There is a disagreement between the submitted answers.

    - First, highlight the disagreements between the two submissions.
    Following is the syntax to highlight the differences:

    (1) <factual_content>
    +++ <submission_1_factual_content_diff>
    --- <submission_2_factual_content_diff>

    [BEGIN EXAMPLE]
    Here are the key differences between the two submissions:
    (1) Capital of France
    +++ Paris
    --- France
    [END EXAMPLE]

    - Then reason about the highlighted differences. The submitted answers may either be a subset or superset of each other, or it may conflict. Determine which case applies.
    - At the end, print only a single choice from AB (without quotes or brackets or punctuation) on its own line corresponding to the correct answer. e.g A

    Reasoning:
    "}.to_string()
}

pub fn default_correctness_prompt() -> String {
    indoc! {"
    You are evaluating an AI agent's response to a business question.

    [Question]: {{ prompt }}
    [Expected Answer]: {{ expected }}
    [Agent's Answer]: {{ actual }}

    Think step by step:
    1. What are the key facts/claims in the expected answer?
    2. Does the agent's answer contain these facts?
    3. Are there any contradictions or significant omissions?

    Ignore differences in formatting, style, phrasing, and level of detail.
    The agent may include additional correct information beyond the expected answer — this is fine.
    Mark as PASS if the agent's answer contains the core factual content of the expected answer.

    Reasoning:

    Verdict: PASS or FAIL
    "}
    .to_string()
}

fn default_loop_concurrency() -> usize {
    1
}

fn default_consistency_concurrency() -> usize {
    10
}

#[cfg(test)]
mod tests {
    use schemars::schema_for;

    /// A `config.yml` that declares neither models nor databases still parses.
    ///
    /// Both were REQUIRED keys until this test existed, and `Config` is
    /// `deny_unknown_fields`, so their absence failed the whole document —
    /// `storage.rs` then continued with an empty `Config`, silently ignoring
    /// every integration the file DID declare, and the workspace resurfaced
    /// downstream as "not configured". Production reported it as
    /// "config.yml is present but does not parse": missing field `models`.
    #[test]
    fn a_config_without_models_or_databases_parses() {
        use super::Config;
        let yaml = r#"
integrations:
  - name: toast
    type: toast
    webhook_secret_var: TOAST_WEBHOOK_SECRET
"#;
        let config: Config =
            serde_yaml::from_str(yaml).expect("a config declaring no models must still parse");
        assert!(config.models.is_empty());
        assert!(config.databases.is_empty());
        // The whole point: what the file did declare survives.
        assert_eq!(config.integrations.len(), 1);
    }

    /// The webhook `toast` integration keeps its original shape — adding the
    /// separate `toast_analytics` type must not have touched it.
    #[test]
    fn webhook_toast_integration_unchanged() {
        use super::{Integration, IntegrationType};
        let yaml = r#"
name: toast
type: toast
webhook_secret_var: TOAST_WEBHOOK_SECRET
"#;
        let integration: Integration = serde_yaml::from_str(yaml).unwrap();
        let IntegrationType::Toast(t) = integration.integration_type else {
            panic!("expected toast integration");
        };
        assert_eq!(t.webhook_secret_var, "TOAST_WEBHOOK_SECRET");
        assert!(t.restaurant_guids.is_empty());
    }

    /// The new reconciliation source parses as its own `toast_analytics` kind,
    /// independent of any webhook `toast` integration.
    #[test]
    fn toast_analytics_integration_parses() {
        use super::{Integration, IntegrationType};
        let yaml = r#"
name: toast_reconcile
type: toast_analytics
client_id_var: TOAST_CLIENT_ID
client_secret_var: TOAST_CLIENT_SECRET
base_url: https://ws-api.toasttab.com
"#;
        let integration: Integration = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(integration.name, "toast_reconcile");
        let IntegrationType::ToastAnalytics(t) = integration.integration_type else {
            panic!("expected toast_analytics integration");
        };
        assert_eq!(t.client_id_var.as_deref(), Some("TOAST_CLIENT_ID"));
        assert_eq!(t.client_secret_var.as_deref(), Some("TOAST_CLIENT_SECRET"));
        assert_eq!(t.api_token_var, None);
        assert_eq!(t.base_url.as_deref(), Some("https://ws-api.toasttab.com"));
    }

    /// An OpenAI-compatible gateway (LangDock and similar EU/self-hosted
    /// proxies): same config shape as `openai` — crucially including `key_var`,
    /// so the credential resolves through the managed workspace secret store —
    /// but the agentic pipeline routes it to `/chat/completions`.
    #[test]
    fn openai_compat_model_parses_with_key_var() {
        use super::Model;
        let yaml = r#"
name: o4-mini
vendor: openai_compat
model_ref: o4-mini
key_var: LANGDOCK_API_KEY
api_url: https://api.langdock.com/openai/eu/v1
"#;
        let model: Model = serde_yaml::from_str(yaml).unwrap();
        let Model::OpenAICompat { config } = &model else {
            panic!("expected an openai_compat model, got {model:?}");
        };
        assert_eq!(config.name, "o4-mini");
        assert_eq!(config.model_ref, "o4-mini");
        assert_eq!(
            config.api_url.as_deref(),
            Some("https://api.langdock.com/openai/eu/v1")
        );
        // The differentiator from `ollama`, which also speaks Chat Completions
        // but carries its key inline and so cannot bind a managed secret.
        assert_eq!(model.key_var(), Some("LANGDOCK_API_KEY"));
    }

    /// `rename_all` isn't in play on this enum (variants are explicitly
    /// renamed), but accept the snake_case spelling too so the value matches
    /// `llm.vendor:` in `.agentic.yml`, where serde does derive it.
    #[test]
    fn openai_compat_accepts_snake_case_alias() {
        use super::Model;
        let yaml = "name: m\nvendor: open_ai_compat\nmodel_ref: m\nkey_var: K\napi_url: https://gw.example/v1\n";
        let model: Model = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(model, Model::OpenAICompat { .. }));
    }

    /// `key_var` is validated by `validate_env_var`, which requires the
    /// variable to actually be set — so the fixture borrows `PATH` rather than
    /// mutating the environment, which would make these order-dependent.
    fn config_with_compat_model(api_url_line: &str) -> super::Config {
        let yaml = format!(
            "defaults:\n  database: local\n\
             databases:\n  - name: local\n    type: duckdb\n    dataset: ./data\n\
             models:\n  - name: gw\n    vendor: openai_compat\n    model_ref: m\n\
             \x20   key_var: PATH\n{api_url_line}"
        );
        serde_yaml::from_str(&yaml).expect("config parses")
    }

    /// `api_url` is `#[serde(default)]`-ed to OpenAI's own host, so an omitted
    /// value doesn't arrive as `None` — it arrives as api.openai.com. Without
    /// this rule an `openai_compat` model would silently ship traffic to
    /// OpenAI, which defeats the point of naming a gateway (and breaks
    /// data-residency setups).
    #[test]
    fn openai_compat_rejects_missing_api_url() {
        let config = config_with_compat_model("");
        let err = config
            .validate_config()
            .expect_err("a compat model without api_url must be rejected");
        assert!(
            err.to_string().contains("api_url"),
            "error should name the offending field, got: {err}"
        );
    }

    /// Explicitly pinning OpenAI's host is the same hole spelled out longhand.
    #[test]
    fn openai_compat_rejects_explicit_openai_host() {
        let config = config_with_compat_model(&format!("    api_url: {}\n", super::OPENAI_API_URL));
        assert!(
            config.validate_config().is_err(),
            "pinning api.openai.com on a compat model must be rejected too"
        );
    }

    #[test]
    fn openai_compat_accepts_a_real_gateway_url() {
        let config =
            config_with_compat_model("    api_url: https://api.langdock.com/openai/eu/v1\n");
        config
            .validate_config()
            .expect("a compat model pointing at a gateway is valid");
    }

    #[test]
    fn test_semantic_query_params_schema() {
        use crate::service::types::SemanticQueryParams;
        let schema = schema_for!(SemanticQueryParams);
        let json = serde_json::to_string_pretty(&schema).unwrap();
        println!("\n{}\n", json);

        // Verify that the schema doesn't have "items": true or "value": true
        assert!(
            !json.contains(r#""items": true"#),
            "Schema should not contain 'items': true"
        );
        assert!(
            !json.contains(r#""value": true"#),
            "Schema should not contain 'value': true"
        );
        assert!(
            !json.contains(r#""from": true"#),
            "Schema should not contain 'from': true"
        );
        assert!(
            !json.contains(r#""to": true"#),
            "Schema should not contain 'to': true"
        );
    }
}
