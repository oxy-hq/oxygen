use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Result returned by all three onboarding endpoints.
#[derive(Serialize)]
pub struct OnboardingResult {
    pub workspace_type: String,
    /// The UUID of the newly created workspace. The caller is responsible for
    /// activating it if desired (no auto-activation on the backend).
    pub workspace_id: Uuid,
}

#[derive(Deserialize, Default)]
pub struct DemoSetupRequest {
    /// Project name (slug) — used as directory name inside the projects root.
    /// Ignored in single-project mode (when PROJECT_DIR was provided to oxy serve).
    pub name: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct NewSetupRequest {
    /// Project name (slug) — used as directory name inside the projects root.
    /// Ignored in single-project mode (when PROJECT_DIR was provided to oxy serve).
    pub name: Option<String>,
}

#[derive(Deserialize)]
pub struct GitHubSetupRequest {
    pub namespace_id: Uuid,
    pub repo_id: i64,
    pub branch: String,
    /// Project name (slug) — used as directory name inside the projects root.
    /// Ignored in single-project mode.
    pub name: Option<String>,
    /// Optional subdirectory inside the repository to use as the Oxy project root.
    /// For example, `"analytics"` or `"data/oxy"` for a monorepo layout.
    /// The full repository is still cloned; only the registered project path changes.
    pub subdir: Option<String>,
}

/// Response from the onboarding readiness check.
#[derive(Serialize)]
pub struct ReadinessResponse {
    /// True if at least one LLM API key is set in the environment.
    pub has_llm_key: bool,
    /// Names of LLM API keys that are present in the environment.
    pub llm_keys_present: Vec<String>,
    /// Names of LLM API keys that are absent from the environment.
    pub llm_keys_missing: Vec<String>,
}

/// Request for `POST /{workspace_id}/onboarding/test-llm-key`.
#[derive(Deserialize)]
pub struct TestLlmKeyRequest {
    /// `"anthropic"` or `"openai"`.
    pub provider: String,
    /// The raw API key to validate. Never logged, never persisted by this
    /// endpoint — it is only used to make a single live call to the provider.
    pub api_key: String,
}

/// Response from `test_llm_key`. `success: false` carries an actionable
/// message — typically "invalid API key" but it can also surface network
/// errors so the user understands why the test didn't pass.
#[derive(Serialize)]
pub struct TestLlmKeyResponse {
    pub success: bool,
    pub message: Option<String>,
}

/// Response for `GET /{workspace_id}/onboarding/github-setup`.
///
/// Describes the setup work a GitHub-imported workspace still needs before the
/// user can start asking questions: which LLM API keys are referenced by the
/// repo's `config.yml` but don't yet have a secret set, and which warehouses
/// declare `*_var` references that don't yet resolve.
///
/// The shape mirrors how the frontend presents the prompts:
/// - one `secure_input` per `missing_llm_key_vars` entry
/// - one `credential_form` per `warehouses` entry (containing a field for each
///   `missing_vars` entry)
#[derive(Serialize, Default, Debug, Clone)]
pub struct GithubSetupResponse {
    /// `key_var` names from the repo's configured models (`openai`, `anthropic`,
    /// etc.) that do not yet have a workspace secret. Already-set keys are
    /// omitted so the user isn't asked to re-enter them.
    pub missing_llm_key_vars: Vec<GithubSetupKeyVar>,
    /// Warehouses declared in `config.yml` that still need at least one secret
    /// value before the connection can be tested. Warehouses whose `*_var`
    /// references all resolve are omitted.
    pub warehouses: Vec<GithubSetupWarehouse>,
    /// Every model with its `key_var` — lets the frontend resolve an agent's
    /// chosen `model` to a specific key_var rather than treating any-of-many
    /// as the gap signal.
    pub models: Vec<GithubSetupModel>,
}

#[derive(Serialize, Default, Debug, Clone)]
pub struct GithubSetupModel {
    pub name: String,
    /// `None` for keyless vendors (Ollama).
    pub key_var: Option<String>,
}

#[derive(Serialize, Default, Debug, Clone)]
pub struct GithubSetupKeyVar {
    /// Env-var name to store the secret under (e.g. `ANTHROPIC_API_KEY`).
    pub var_name: String,
    /// User-facing vendor label derived from the model's `vendor` field — used
    /// to personalise the prompt ("Enter your Anthropic API key").
    pub vendor: String,
    /// One sample model name using this key, purely informational.
    pub sample_model_name: Option<String>,
}

#[derive(Serialize, Default, Debug, Clone)]
pub struct GithubSetupWarehouse {
    /// Warehouse `name` as declared in `config.yml`.
    pub name: String,
    /// Dialect string (`postgres`, `snowflake`, …) from `Database::dialect`.
    pub dialect: String,
    /// `*_var` fields on this warehouse that don't yet resolve to a secret.
    pub missing_vars: Vec<GithubSetupMissingVar>,
}

#[derive(Serialize, Default, Debug, Clone)]
pub struct GithubSetupMissingVar {
    /// Field this var refers to (`password`, `user`, `host`, `port`,
    /// `database`, `key_path`, `token`, `developer_token`).
    pub field: String,
    /// Env-var name declared in config.yml (e.g. `SNOWFLAKE_PASSWORD`).
    pub var_name: String,
    /// True when the corresponding plain value isn't also declared in the
    /// config. False means the config already has an inline value and the
    /// `*_var` is only used as a fallback — these entries can be treated as
    /// optional by the UI.
    pub required: bool,
}

/// Manifest of onboarding side-effects to revert.
///
/// Each list is handled idempotently — missing entries are silently skipped so
/// the client can send a best-effort manifest derived from its local state.
#[derive(Deserialize, Default)]
pub struct OnboardingResetRequest {
    /// Secret names to delete (e.g. `ANTHROPIC_API_KEY`).
    #[serde(default)]
    pub secret_names: Vec<String>,
    /// Database names to remove from `config.yml`. For each database, the
    /// associated password secret (via `password_var`) is also deleted.
    #[serde(default)]
    pub database_names: Vec<String>,
    /// Model names to remove from `config.yml`. For each model, the associated
    /// API key secret (via `key_var`) is also deleted.
    #[serde(default)]
    pub model_names: Vec<String>,
    /// File paths (relative to the workspace root) to delete.
    #[serde(default)]
    pub file_paths: Vec<String>,
    /// Directory paths (relative to the workspace root) to recursively delete.
    /// Used for wiping generated trees such as `.databases/<warehouse>/`.
    #[serde(default)]
    pub directory_paths: Vec<String>,
}

#[derive(Serialize, Default)]
pub struct OnboardingResetResponse {
    pub secrets_deleted: Vec<String>,
    pub databases_removed: Vec<String>,
    pub models_removed: Vec<String>,
    pub files_deleted: Vec<String>,
    pub directories_deleted: Vec<String>,
    /// Human-readable warnings for individual entries that could not be
    /// reverted — the overall request still returns 200 to stay idempotent.
    pub warnings: Vec<String>,
}

#[derive(Serialize)]
pub struct SkippedUpload {
    pub name: String,
    pub reason: String,
}

#[derive(Serialize)]
pub struct UploadWarehouseFilesResponse {
    /// The resolved subdir, relative to the workspace root (e.g. ".db").
    pub subdir: String,
    /// Paths of files successfully written, relative to the workspace root.
    pub files: Vec<String>,
    /// Files that were rejected (unsupported extension, oversize, etc.).
    pub skipped: Vec<SkippedUpload>,
}
