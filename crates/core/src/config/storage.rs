use std::path::{Path, PathBuf};
use tokio::fs;

use crate::state_dir::resolve_state_dir_with_fallback;
use oxy_compile::context_documents::{ContextPatterns, DocumentFile};
use oxy_shared::errors::OxyError;

use super::artifacts::{AgentEntry, AppEntry, AutomationEntry, PipelineEntry, SimulationEntry};
use super::model::{AppConfig, Automation, AutomationWithRawVariables, Config};
use super::naming::artifact_name;
use super::test_config::TestFileConfig;

/// `name` / `title` / `published` off the YAML root mapping, parsed leniently so
/// a validation error deeper in the file cannot make an app vanish from the
/// listing or read as unpublished. The three defaults must match
/// `oxy_compile::compile::compile_app`, which is what fills the same columns on
/// the compiled arm — `crates/app/tests/platform/artifact_naming_agrees.rs` pins `name`.
/// The `name:` field, or the path rule the compiler falls back to. Shared so a
/// second entity kind cannot invent a third spelling.
async fn yaml_name(path: &Path, relative: &str) -> String {
    yaml_root(path)
        .await
        .and_then(|m| {
            m.get(serde_yaml::Value::String("name".to_string()))
                .and_then(|v| v.as_str().map(str::to_string))
        })
        .unwrap_or_else(|| artifact_name(relative))
}

async fn yaml_root(path: &Path) -> Option<serde_yaml::Mapping> {
    fs::read_to_string(path)
        .await
        .ok()
        .and_then(|yaml| serde_yaml::from_str::<serde_yaml::Value>(&yaml).ok())
        .and_then(|value| value.as_mapping().cloned())
}

async fn read_app_entry(path: &Path, relative: String) -> AppEntry {
    let mapping = yaml_root(path).await;
    let field = |key: &str| {
        mapping
            .as_ref()
            .and_then(|m| m.get(serde_yaml::Value::String(key.to_string())).cloned())
    };

    AppEntry {
        name: field("name")
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| artifact_name(&relative)),
        title: field("title").and_then(|v| v.as_str().map(str::to_string)),
        published: field("published")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        file_path: relative,
    }
}

const DEFAULT_CONFIG_PATH: &str = "config.yml";
const AUTOMATION_EXTENSION: &str = ".automation";
const PROCEDURE_EXTENSION: &str = ".procedure";
#[allow(dead_code)]
const TEST_EXTENSION: &str = ".test";

pub(super) trait ConfigStorage {
    async fn load_config(&self) -> Result<Config, OxyError>;
    async fn load_config_with_fallback(&self) -> Config;
    async fn write_config(&self, config: &Config) -> Result<(), OxyError>;
    async fn load_automation_config<P: AsRef<Path>>(
        &self,
        automation_ref: P,
    ) -> Result<Automation, OxyError>;
    async fn load_automation_config_with_raw_variables<P: AsRef<Path>>(
        &self,
        automation_ref: P,
    ) -> Result<AutomationWithRawVariables, OxyError>;
    async fn fs_link<P: AsRef<Path>>(&self, file_ref: P) -> Result<String, OxyError>;
    async fn resolve_state_dir(&self) -> Result<PathBuf, OxyError>;
    /// The four listers below all report `file_path` **workspace-relative**,
    /// `/`-separated — NOT absolute, which is what they used to return.
    ///
    /// It is the form the compiled arm carries: `automation_definitions` is
    /// keyed by exactly this string, so the two sources only agree in this
    /// shape. It is also what the downstream contract wants — an absolute
    /// `workflow_ref` is rejected as a `..`-traversal guard, so every caller
    /// was undoing an absolute path anyway.
    ///
    /// Join with `project_path()` if you need an absolute one; do not read
    /// `file_path` off the filesystem directly.
    async fn list_analytics_agents(&self) -> Result<Vec<AgentEntry>, OxyError>;
    async fn list_apps(&self) -> Result<Vec<AppEntry>, OxyError>;
    async fn list_workflows(&self) -> Result<Vec<AutomationEntry>, OxyError>;
    async fn list_pipelines(&self) -> Result<Vec<PipelineEntry>, OxyError>;
    async fn list_simulations(&self) -> Result<Vec<SimulationEntry>, OxyError>;
    /// The markdown files `patterns` reach. Paths only: the caller reads them,
    /// so that an unreadable file is classified where the other read faults
    /// are.
    async fn list_context_documents(
        &self,
        patterns: &ContextPatterns,
    ) -> Result<Vec<DocumentFile>, OxyError>;
    async fn load_app_config<P: AsRef<Path>>(&self, app_path: P) -> Result<AppConfig, OxyError>;
    async fn get_charts_dir(&self) -> Result<PathBuf, OxyError>;
    async fn get_results_dir(&self) -> Result<PathBuf, OxyError>;
    async fn get_exported_chart_dir(&self) -> Result<PathBuf, OxyError>;
    async fn get_app_results_dir(&self) -> Result<PathBuf, OxyError>;
    async fn load_test_config<P: AsRef<Path>>(
        &self,
        test_ref: P,
    ) -> Result<TestFileConfig, OxyError>;
    async fn list_tests(&self) -> Result<Vec<PathBuf>, OxyError>;
}

#[derive(Debug)]
pub(super) struct FsStorage {
    project_path: PathBuf,
    config_path: String,
}

impl FsStorage {
    pub fn new<P: AsRef<Path>>(project_path: P) -> Result<Self, OxyError> {
        Ok(FsStorage {
            project_path: project_path.as_ref().to_path_buf(),
            config_path: DEFAULT_CONFIG_PATH.to_string(),
        })
    }

    pub(super) fn project_path(&self) -> &Path {
        &self.project_path
    }

    fn get_stem_by_extension(&self, path: &PathBuf, extension: &str) -> Result<String, OxyError> {
        let file_stem = path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| {
            OxyError::ConfigurationError(format!(
                "Invalid file path (no file stem or non-UTF-8): {}",
                path.display()
            ))
        })?;
        Ok(file_stem
            .strip_suffix(extension)
            .unwrap_or(file_stem)
            .to_string())
    }

    /// A workspace root that is not on this disk is not an empty workspace.
    ///
    /// `read_dir` on a missing directory yields nothing, so every lister below
    /// used to answer "this workspace has no agents" when the truth was "this
    /// process holds no working copy for it". That is the shape behind both
    /// shipped incidents. Every lister passes through here, so an eighth one
    /// cannot lose the distinction by forgetting to check.
    fn require_root(&self) -> Result<(), OxyError> {
        if self.project_path.is_dir() {
            return Ok(());
        }
        Err(OxyError::ConfigurationError(format!(
            "workspace directory not found: {}",
            self.project_path.display()
        )))
    }

    fn list_by_sub_extension(&self, dir: Option<&PathBuf>, sub_extension: &str) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let dir = dir.unwrap_or(&self.project_path);
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // `oxy_compile::walker::is_skipped` is the one definition of
                    // "a path the workspace does not enumerate" — see its doc
                    // comment. Pruning here (rather than filtering after the
                    // walk) keeps this arm from descending into e.g. a huge
                    // `node_modules/`.
                    let skipped = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(oxy_compile::walker::is_skipped)
                        .unwrap_or(false);
                    if skipped {
                        // A pruned directory must leave a trace, the way the
                        // compile walker's own drops do. Discovery is the only
                        // thing between a file on disk and a listing, so a
                        // silent prune reads to the user as "my agent
                        // disappeared" with nothing to grep for. DEBUG rather
                        // than WARN because the common case is a
                        // `node_modules/` — one line per directory, not per
                        // file, since this arm prunes before it descends.
                        tracing::debug!(
                            dir = %path.display(),
                            sub_extension,
                            "workspace listing: pruning a skipped directory"
                        );
                    } else {
                        files.extend(self.list_by_sub_extension(Some(&path), sub_extension));
                    }
                } else if path.is_file()
                    && path.extension().and_then(|s| s.to_str()) == Some("yml")
                    && path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .map(|s| s.ends_with(format!(".{sub_extension}.yml").as_str()))
                        .unwrap_or(false)
                {
                    files.push(path);
                }
            }
        }
        files
    }

    /// [`Self::list_by_sub_extension`] as the ENTITY listers want it: minus the
    /// test files that mirror an entity extension.
    ///
    /// `oxy_compile::walker` drops any path whose FILE NAME contains `.test.`,
    /// and this is the working-copy half of that one rule. Without it the two
    /// workspace enumerations disagree on exactly the names that end in a real
    /// entity extension but are fixtures — `baseline.test.simulation.yml` ends
    /// `.simulation.yml`, so the extension match below claims it while the
    /// walker drops it, and the IDE resolves a world the fleet 404s.
    ///
    /// The rule is scoped to the file name, not the whole path, for the same
    /// reason it is on the walker: a fixtures DIRECTORY like
    /// `worlds/q3.test.grid/` is not a build dir and its real entities must
    /// survive. `is_skipped` already owns the directory question.
    ///
    /// **Not folded into `list_by_sub_extension`**, which
    /// [`ConfigStorage::list_tests`] calls with `sub_extension = "test"` —
    /// every path it wants contains `.test.`, so a blanket filter one level
    /// down would return nothing at all.
    fn list_entity_files(&self, sub_extension: &str) -> Vec<PathBuf> {
        self.list_by_sub_extension(None, sub_extension)
            .into_iter()
            .filter(|path| {
                let is_fixture = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains(".test."))
                    .unwrap_or(false);
                if is_fixture {
                    tracing::debug!(
                        path = %path.display(),
                        sub_extension,
                        "workspace listing: dropping a test file mirroring an entity extension"
                    );
                }
                !is_fixture
            })
            .collect()
    }

    fn try_ensure_dir_exists(&self, path: &Path) -> Result<(), OxyError> {
        if !path.exists() {
            std::fs::create_dir_all(path).map_err(|e| {
                OxyError::ConfigurationError(format!(
                    "Could not create directory {}: {e}",
                    path.display()
                ))
            })?;
        }
        Ok(())
    }

    fn validate_path_within_project<P: AsRef<Path>>(
        &self,
        file_ref: P,
    ) -> Result<PathBuf, OxyError> {
        let resolved_path = self.project_path.join(file_ref);
        let canonical_project = self
            .project_path
            .canonicalize()
            .map_err(|e| OxyError::IOError(format!("Failed to canonicalize project path: {e}")))?;

        let canonical_resolved = if resolved_path.exists() {
            resolved_path.canonicalize().map_err(|e| {
                OxyError::IOError(format!("Failed to canonicalize resolved path: {e}"))
            })?
        } else {
            let parent = resolved_path.parent().ok_or_else(|| {
                OxyError::IOError("Invalid path: no parent directory".to_string())
            })?;
            let filename = resolved_path
                .file_name()
                .ok_or_else(|| OxyError::IOError("Invalid path: no filename".to_string()))?;

            if parent.exists() {
                parent
                    .canonicalize()
                    .map_err(|e| {
                        OxyError::IOError(format!("Failed to canonicalize parent path: {e}"))
                    })?
                    .join(filename)
            } else {
                let normalized = self.normalize_path(&resolved_path);
                let normalized_project = self.normalize_path(&self.project_path);
                if !normalized.starts_with(&normalized_project) {
                    return Err(OxyError::IOError(
                        "Path traversal detected: resolved path is outside project directory"
                            .to_string(),
                    ));
                }
                return Ok(resolved_path);
            }
        };

        if !canonical_resolved.starts_with(&canonical_project) {
            return Err(OxyError::IOError(
                "Path traversal detected: resolved path is outside project directory".to_string(),
            ));
        }

        Ok(resolved_path)
    }

    fn normalize_path(&self, path: &Path) -> PathBuf {
        let mut components = Vec::new();
        for component in path.components() {
            match component {
                std::path::Component::Normal(name) => components.push(name),
                std::path::Component::ParentDir => {
                    components.pop();
                }
                std::path::Component::CurDir => {}
                other => components.push(other.as_os_str()),
            }
        }
        components.iter().collect()
    }
}

impl FsStorage {
    /// The "nothing configured" value. Explicit rather than `Default` because
    /// `Config` deliberately does not derive it — an empty config is a fallback,
    /// never a thing you construct on purpose.
    fn empty_config(&self) -> Config {
        Config {
            defaults: None,
            workspace_path: self.project_path.clone(),
            models: [].to_vec(),
            databases: [].to_vec(),
            builder_agent: None,
            timezone: None,
            integrations: vec![],
            slack_legacy: None,
            mcp: None,
            protected_branches: None,
            base_branch: None,
            repositories: vec![],
            admins: vec![],
            health_check: None,
            pre_aggregations: None,
        }
    }
}

impl ConfigStorage for FsStorage {
    async fn load_config(&self) -> Result<Config, OxyError> {
        let resolved_path = PathBuf::from(&self.project_path).join(&self.config_path);
        let config_yml = fs::read_to_string(resolved_path).await.map_err(|e| {
            OxyError::ConfigurationError(format!(
                "Failed to read config from file: {e}, project_path: {}",
                self.project_path.display()
            ))
        })?;
        let mut config: Config = serde_yaml::from_str(&config_yml).map_err(|e| {
            OxyError::ConfigurationError(format!(
                "Failed to deserialize config: {e}, project_path: {}",
                self.project_path.display()
            ))
        })?;
        if config.slack_legacy.is_some() {
            tracing::warn!(
                "config.yml contains a `slack:` section which is no longer read. \
                Slack is now configured per-org via OAuth. \
                Please remove the `slack:` section from your config.yml."
            );
        }
        config.workspace_path = self.project_path.clone();
        Ok(config)
    }

    async fn load_config_with_fallback(&self) -> Config {
        let resolved_path = PathBuf::from(&self.project_path).join(&self.config_path);

        // Three outcomes, only one of which is legitimate. They used to collapse
        // into the same empty `Config`, which is how a platform-side miss came
        // to be reported as the customer's configuration:
        //
        //   POST /api/webhooks/toast/orders  ->  no config.yml on this replica
        //     -> empty Config -> no `integrations:` -> resolve_toast == Ok(None)
        //     -> 401 "toast integration not configured for this workspace"
        //
        // 2,072 of those an hour for four days (#2816), with nothing in the logs,
        // because "file absent" and "customer configured nothing" were the same
        // value by the time anything looked. Absent is still tolerated — that is
        // the point of this method, and a workspace mid-onboarding has no
        // config.yml yet — but unreadable and unparseable now say so.
        let config_yml = match std::fs::read_to_string(&resolved_path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(
                    path = %resolved_path.display(),
                    "no config.yml; continuing with an empty config"
                );
                String::new()
            }
            Err(e) => {
                // Permissions, a broken mount, an I/O fault. The file is
                // supposed to be here and we could not read it — that is a fault
                // on this node, not a workspace without configuration.
                tracing::error!(
                    path = %resolved_path.display(),
                    error = %e,
                    "config.yml exists but could not be read; continuing with an \
                     empty config. Anything that reads `integrations:`, \
                     `databases:` or `models:` will behave as if the workspace \
                     were unconfigured — treat a downstream \"not configured\" as \
                     THIS, not as the customer's doing."
                );
                String::new()
            }
        };

        let mut config: Config = match serde_yaml::from_str(&config_yml) {
            Ok(config) => config,
            Err(e) => {
                if !config_yml.trim().is_empty() {
                    tracing::error!(
                        path = %resolved_path.display(),
                        error = %e,
                        "config.yml is present but does not parse; continuing with \
                         an empty config. Every integration, database and model \
                         declared in it is being ignored."
                    );
                }
                self.empty_config()
            }
        };
        config.workspace_path = self.project_path.clone();
        config
    }

    async fn write_config(&self, config: &Config) -> Result<(), OxyError> {
        let resolved_path = PathBuf::from(&self.project_path).join(&self.config_path);
        let config_yml = serde_yaml::to_string(config).map_err(|e| {
            OxyError::ConfigurationError(format!("Failed to serialize config: {e}"))
        })?;
        fs::write(&resolved_path, config_yml).await.map_err(|e| {
            OxyError::IOError(format!(
                "Failed to write config to file {}: {e}",
                resolved_path.display()
            ))
        })?;
        Ok(())
    }

    async fn load_automation_config<P: AsRef<Path>>(
        &self,
        automation_ref: P,
    ) -> Result<Automation, OxyError> {
        let resolved_path = self.validate_path_within_project(automation_ref)?;
        let automation_yml = fs::read_to_string(&resolved_path).await.map_err(|e| {
            OxyError::ConfigurationError(format!("Failed to read automation config from file: {e}"))
        })?;
        let mut automation_config: Automation =
            serde_yaml::from_str(&automation_yml).map_err(|e| {
                OxyError::ConfigurationError(format!(
                    "Failed to deserialize automation config: {e}"
                ))
            })?;
        let file_name = resolved_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let extension = if file_name.contains(AUTOMATION_EXTENSION) {
            AUTOMATION_EXTENSION
        } else if file_name.contains(PROCEDURE_EXTENSION) {
            PROCEDURE_EXTENSION
        } else {
            AUTOMATION_EXTENSION
        };
        automation_config.name = self.get_stem_by_extension(&resolved_path, extension)?;
        Ok(automation_config)
    }

    async fn load_automation_config_with_raw_variables<P: AsRef<Path>>(
        &self,
        automation_ref: P,
    ) -> Result<AutomationWithRawVariables, OxyError> {
        let resolved_path = self.validate_path_within_project(automation_ref)?;
        let automation_yml = fs::read_to_string(&resolved_path).await.map_err(|e| {
            OxyError::ConfigurationError(format!("Failed to read automation config from file: {e}"))
        })?;
        let mut temp_automation: AutomationWithRawVariables = serde_yaml::from_str(&automation_yml)
            .map_err(|e| {
                OxyError::ConfigurationError(format!(
                    "Failed to deserialize automation config: {e}"
                ))
            })?;
        let file_name = resolved_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let extension = if file_name.contains(AUTOMATION_EXTENSION) {
            AUTOMATION_EXTENSION
        } else if file_name.contains(PROCEDURE_EXTENSION) {
            PROCEDURE_EXTENSION
        } else {
            AUTOMATION_EXTENSION
        };
        temp_automation.name = self.get_stem_by_extension(&resolved_path, extension)?;
        Ok(temp_automation)
    }

    async fn fs_link<P: AsRef<Path>>(&self, file_ref: P) -> Result<String, OxyError> {
        let resolved_path = self.validate_path_within_project(file_ref)?;
        Ok(resolved_path.display().to_string())
    }

    /// The state dir, created on the way out.
    ///
    /// The guard covers the *fallback* only. `OXY_STATE_DIR` points outside the
    /// workspace and wins, so there is nothing to protect on that path — but the
    /// fallback is `<root>/.oxy_state`, and creating it on a node with no
    /// working copy manufactures the workspace root. That is exactly what
    /// `WorkingCopy::new` was stopped from doing; fixing the constructor and leaving this
    /// accessor would have reopened it through the back door. The creating
    /// resolver calls `std::process::exit(1)` on failure, so the fallout would
    /// not even be a catchable error.
    async fn resolve_state_dir(&self) -> Result<PathBuf, OxyError> {
        let fallback = PathBuf::from(&self.project_path).join(".oxy_state");
        if std::env::var("OXY_STATE_DIR").is_err() {
            self.require_root()?;
        }
        Ok(resolve_state_dir_with_fallback(Some(fallback)))
    }

    async fn list_analytics_agents(&self) -> Result<Vec<AgentEntry>, OxyError> {
        self.require_root()?;
        let mut out = Vec::new();
        for path in self.list_entity_files("agentic") {
            let Ok(relative) = path.strip_prefix(&self.project_path) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            let root = yaml_root(&path).await;
            let field = |key: &str| {
                root.as_ref()
                    .and_then(|m| m.get(serde_yaml::Value::String(key.to_string())).cloned())
            };
            out.push(AgentEntry {
                name: field("name")
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_else(|| artifact_name(&relative)),
                model_ref: field("llm").and_then(|llm| {
                    llm.as_mapping()?
                        .get(serde_yaml::Value::String("ref".to_string()))?
                        .as_str()
                        .map(str::to_string)
                }),
                timezone: field("timezone").and_then(|v| v.as_str().map(str::to_string)),
                file_path: relative,
            });
        }
        Ok(out)
    }

    async fn list_workflows(&self) -> Result<Vec<AutomationEntry>, OxyError> {
        self.require_root()?;
        let mut out = Vec::new();
        for (sub_extension, extension) in [
            ("procedure", ".procedure.yml"),
            ("workflow", ".workflow.yml"),
            ("automation", ".automation.yml"),
        ] {
            for path in self.list_entity_files(sub_extension) {
                let Ok(relative) = path.strip_prefix(&self.project_path) else {
                    continue;
                };
                let relative = relative.to_string_lossy().replace('\\', "/");
                out.push(AutomationEntry {
                    name: yaml_name(&path, &relative).await,
                    extension: extension.to_string(),
                    file_path: relative,
                });
            }
        }
        Ok(out)
    }

    async fn list_pipelines(&self) -> Result<Vec<PipelineEntry>, OxyError> {
        self.require_root()?;
        let mut out = Vec::new();
        for path in self.list_entity_files("airway") {
            let Ok(relative) = path.strip_prefix(&self.project_path) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            // Read the kind here too, not just on the compiled arm. Local mode
            // never promotes a revision, so a kind supplied only by the
            // boundary would be absent for every pipeline, forever, and any UI
            // gated on it could never appear locally. A file that will not
            // parse yields `None` rather than failing the listing: a broken
            // pipeline should still be visible so it can be fixed.
            let source_kind = fs::read_to_string(&path)
                .await
                .ok()
                .and_then(|text| serde_yaml::from_str::<serde_json::Value>(&text).ok())
                .as_ref()
                .and_then(crate::config::artifacts::pipeline_source_kind);
            out.push(PipelineEntry {
                name: yaml_name(&path, &relative).await,
                file_path: relative,
                source_kind,
            });
        }
        Ok(out)
    }

    /// Every declared world (`*.simulation.yml`), body included.
    ///
    /// It carries the parsed `definition`, which is what
    /// `simulation_definitions.definition` holds — the grid renders off it and
    /// a run reads its seed out of it. That's the one thing this does that its
    /// siblings do not; the skip set (`target/`, `node_modules/`, `dist/`,
    /// `build/`, hidden dirs, all at any depth) comes from
    /// [`Self::list_by_sub_extension`] pruning via `oxy_compile::walker::is_skipped`
    /// — the same function the compile walker uses, so a stray copy under
    /// `build/` or a nested `sub/target/` can't list on one arm and not the
    /// other. See `crates/core/src/config/manager.rs::list_simulations`.
    async fn list_simulations(&self) -> Result<Vec<SimulationEntry>, OxyError> {
        self.require_root()?;
        let mut out = Vec::new();
        for path in self.list_entity_files("simulation") {
            let Ok(relative) = path.strip_prefix(&self.project_path) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            let source = match fs::read_to_string(&path).await {
                Ok(source) => source,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "skip unreadable world");
                    continue;
                }
            };
            // A malformed world is skipped with a warning rather than failing
            // the listing: one broken file must not hide every other world on
            // the page.
            let definition: serde_json::Value = match serde_yaml::from_str(&source) {
                Ok(definition) => definition,
                Err(e) => {
                    tracing::warn!(path = %relative, error = %e, "skip unparseable world");
                    continue;
                }
            };
            let name = definition
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| artifact_name(&relative));
            out.push(SimulationEntry {
                name,
                file_path: relative,
                definition,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// The working-copy half of `context_document_definitions`.
    ///
    /// It does not walk the tree itself. `oxy_compile::context_documents` owns
    /// the walk and both skip rules, and the compile walker calls the same
    /// function, so this arm and the compiled one enumerate the same files by
    /// construction rather than by two lists kept in step.
    async fn list_context_documents(
        &self,
        patterns: &ContextPatterns,
    ) -> Result<Vec<DocumentFile>, OxyError> {
        self.require_root()?;
        oxy_compile::context_documents::discover(&self.project_path, patterns).map_err(|e| {
            OxyError::IOError(format!(
                "could not list context documents under {}: {e}",
                self.project_path.display()
            ))
        })
    }

    async fn list_apps(&self) -> Result<Vec<AppEntry>, OxyError> {
        self.require_root()?;
        let mut out = Vec::new();
        for path in self.list_entity_files("app") {
            let Ok(relative) = path.strip_prefix(&self.project_path) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            out.push(read_app_entry(&path, relative).await);
        }
        Ok(out)
    }

    async fn load_app_config<P: AsRef<Path>>(&self, app_path: P) -> Result<AppConfig, OxyError> {
        let resolved_path = self.validate_path_within_project(app_path)?;
        let agent_yml = fs::read_to_string(&resolved_path).await.map_err(|e| {
            OxyError::ConfigurationError(format!("Failed to read agent config from file: {e}"))
        })?;
        let app_config: AppConfig = serde_yaml::from_str(&agent_yml).map_err(|e| {
            OxyError::ConfigurationError(format!("Failed to deserialize agent config: {e}"))
        })?;

        Ok(app_config)
    }

    async fn get_charts_dir(&self) -> Result<PathBuf, OxyError> {
        let charts_dir = self.resolve_state_dir().await?.join("charts");
        self.try_ensure_dir_exists(&charts_dir)?;
        Ok(charts_dir)
    }

    async fn get_exported_chart_dir(&self) -> Result<PathBuf, OxyError> {
        let charts_dir = self.resolve_state_dir().await?.join("exported-charts");
        self.try_ensure_dir_exists(&charts_dir)?;
        Ok(charts_dir)
    }

    async fn get_results_dir(&self) -> Result<PathBuf, OxyError> {
        let results_dir = self.resolve_state_dir().await?.join("results");
        self.try_ensure_dir_exists(&results_dir)?;
        Ok(results_dir)
    }

    async fn get_app_results_dir(&self) -> Result<PathBuf, OxyError> {
        let dir = self.resolve_state_dir().await?.join("apps").join("results");
        self.try_ensure_dir_exists(&dir)?;
        Ok(dir)
    }

    async fn load_test_config<P: AsRef<Path>>(
        &self,
        test_ref: P,
    ) -> Result<TestFileConfig, OxyError> {
        let resolved_path = self.validate_path_within_project(&test_ref)?;
        let test_yml = fs::read_to_string(&resolved_path).await.map_err(|e| {
            OxyError::ConfigurationError(format!("Failed to read test config from file: {e}"))
        })?;
        let mut test_config: TestFileConfig = serde_yaml::from_str(&test_yml).map_err(|e| {
            OxyError::ConfigurationError(format!(
                "Failed to deserialize test config {}: {e}",
                resolved_path.display()
            ))
        })?;
        // Infer target from filename if not specified, e.g.
        // "sales.agentic.test.yml" -> "sales.agentic.yml".
        if test_config.target.is_none() {
            let file_name = resolved_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if let Some(base) = file_name.strip_suffix(".test.yml") {
                let target_name = format!("{base}.yml");
                if let Some(parent) = resolved_path.parent() {
                    let target_path = parent.join(&target_name);
                    if let Ok(relative) = target_path.strip_prefix(&self.project_path) {
                        test_config.target = Some(relative.display().to_string());
                    } else {
                        test_config.target = Some(target_name);
                    }
                } else {
                    test_config.target = Some(target_name);
                }
            }
        }
        Ok(test_config)
    }

    async fn list_tests(&self) -> Result<Vec<PathBuf>, OxyError> {
        self.require_root()?;
        let candidates = self.list_by_sub_extension(None, "test");
        let mut test_files = Vec::new();
        for path in candidates {
            let content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(_) => continue,
            };
            if serde_yaml::from_str::<TestFileConfig>(&content).is_ok() {
                test_files.push(path);
            }
        }
        Ok(test_files)
    }
}
