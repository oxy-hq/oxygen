//! `BuilderProjectValidator` implementation backed by oxy's ConfigManager
//! and airlayer for semantic file validation.

use std::path::{Path, PathBuf};

use agentic_builder::validator::{BuilderProjectValidator, ValidatedFile, ValidationReport};
use agentic_core::tools::ToolError;
use async_trait::async_trait;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::WorkingCopy;

/// Bridges builder project validation to oxy's config validation (for agents,
/// automations, apps) and airlayer parsing (for semantic views/topics).
pub struct OxyBuilderProjectValidator {
    workspace_manager: WorkspaceManager<WorkingCopy>,
}

impl OxyBuilderProjectValidator {
    pub fn new(workspace_manager: WorkspaceManager<WorkingCopy>) -> Self {
        Self { workspace_manager }
    }
}

#[async_trait]
impl BuilderProjectValidator for OxyBuilderProjectValidator {
    async fn validate_file(&self, abs_path: &Path) -> Result<(), String> {
        let config: oxy::config::ConfigManager<WorkingCopy> = oxy::config::ConfigBuilder::new()
            .with_workspace_path(self.workspace_manager.config_manager.workspace_path())
            .map_err(|e| format!("config error: {e}"))?
            .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Fail)
            .await
            .map_err(|e| format!("config error: {e}"))?;

        let cfg = config.get_config();
        let file_name = abs_path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        validate_single_file(abs_path, file_name, cfg)
    }

    async fn validate_all(&self) -> Result<ValidationReport, ToolError> {
        let project_path = self
            .workspace_manager
            .config_manager
            .workspace_path()
            .to_path_buf();

        let config: oxy::config::ConfigManager<WorkingCopy> = oxy::config::ConfigBuilder::new()
            .with_workspace_path(&project_path)
            .map_err(|e| ToolError::Execution(format!("config error: {e}")))?
            .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Fail)
            .await
            .map_err(|e| ToolError::Execution(format!("config error: {e}")))?;

        let cfg = config.get_config();
        let mut errors: Vec<ValidatedFile> = Vec::new();
        let mut valid_count: usize = 0;

        for path in cfg.list_workflows(&cfg.workspace_path) {
            let rel = path
                .strip_prefix(&project_path)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            match cfg
                .load_workflow(&path)
                .and_then(|w| cfg.validate_workflow(&w).map_err(Into::into))
            {
                Ok(()) => valid_count += 1,
                Err(e) => errors.push(ValidatedFile {
                    relative_path: rel,
                    error: Some(e.to_string()),
                }),
            }
        }

        for path in list_semantic_files(&cfg.workspace_path) {
            let rel = path
                .strip_prefix(&project_path)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            match validate_semantic_file(&path) {
                Ok(()) => valid_count += 1,
                Err(e) => errors.push(ValidatedFile {
                    relative_path: rel,
                    error: Some(e),
                }),
            }
        }

        Ok(ValidationReport {
            valid_count,
            errors,
        })
    }
}

fn validate_single_file(
    abs: &Path,
    file_name: &str,
    cfg: &oxy::config::model::Config,
) -> Result<(), String> {
    if file_name.ends_with(".procedure.yml") || file_name.ends_with(".automation.yml") {
        let w = cfg
            .load_workflow(&abs.to_path_buf())
            .map_err(|e| e.to_string())?;
        cfg.validate_workflow(&w).map_err(|e| e.to_string())
    } else if file_name.ends_with(".view.yml") || file_name.ends_with(".topic.yml") {
        validate_semantic_file(abs)
    } else {
        Err(format!(
            "unsupported file type: {file_name}. Expected .procedure.yml, \
             .automation.yml, .view.yml, or .topic.yml"
        ))
    }
}

// ── Semantic file validation ────────────────────────────────────────────────

/// Validate a semantic file by parsing it through the canonical
/// `oxy-airlayer-compat` shim — the same parser analytics, the automation
/// bridge, and `oxy validate` use, so the builder validator cannot accept a
/// file analytics would reject. See
/// `internal-docs/semantic-validation-standardization.md`.
fn validate_semantic_file(abs: &Path) -> Result<(), String> {
    let content = std::fs::read_to_string(abs).map_err(|e| e.to_string())?;
    let file_name = abs.file_name().and_then(|n| n.to_str()).unwrap_or("");

    if file_name.ends_with(".view.yml") {
        oxy_airlayer_compat::parse_view_yaml(&content)
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else {
        oxy_airlayer_compat::parse_topic_yaml(&content)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

fn list_semantic_files(project_path: &Path) -> Vec<PathBuf> {
    let semantics_dir = project_path.join("semantics");
    let mut files = Vec::new();
    for sub in &["views", "topics"] {
        let dir = semantics_dir.join(sub);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str())
                    && (name.ends_with(".view.yml") || name.ends_with(".topic.yml"))
                {
                    files.push(path);
                }
            }
        }
    }
    files
}
