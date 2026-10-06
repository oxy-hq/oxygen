use std::path::PathBuf;
pub mod apps_helpers;
pub mod auth;
pub mod health_check;
pub mod model;
mod parser;
pub mod preagg_check;
pub mod schema_type_converter;
pub mod test_config;
pub mod validate;
use garde::Validate;
mod artifacts;
mod builder;
mod compiled;
pub mod constants;
mod context_documents;
mod manager;
mod naming;
pub mod oxy;
pub mod scan;
mod storage;

use anyhow;
use model::{AppConfig, Automation, Config, Database, Model, SemanticModels};

use parser::{parse_automation_config, parse_semantic_model_config};
use std::{fs, io};
use validate::{DataAppValidationContext, ValidationContext, ValidationContextMetadata};

use oxy_shared::errors::OxyError;

pub use artifacts::{
    AgentEntry, AppEntry, ArtifactError, AutomationEntry, CompiledArtifact, ContextDocument,
    ContextDocuments, PipelineEntry, SimulationEntry, VerifiedQueryEntry, pipeline_source_kind,
};
pub use builder::{ConfigBuilder, OnMissing};
pub use manager::{ConfigManager, DiskSlot, Origin, ReadOnly, ResolveWorkspaceFile, WorkingCopy};
pub use naming::artifact_name;
pub use scan::{ScanDir, SemanticEntity};

impl Config {
    pub fn validate_config(&self) -> anyhow::Result<()> {
        let context = ValidationContext {
            config: self.clone(),
            metadata: None,
        };
        if let Err(e) = self.validate_with(&context) {
            anyhow::bail!(OxyError::ConfigurationError(format!(
                "Invalid configuration: {e}"
            )));
        }

        Ok(())
    }

    pub fn validate_workflow(&self, automation: &Automation) -> anyhow::Result<()> {
        let context = ValidationContext {
            config: self.clone(),
            metadata: None,
        };
        match automation.validate_with(&context) {
            Ok(_) => Ok(()),
            Err(e) => anyhow::bail!(OxyError::ConfigurationError(format!(
                "Invalid automation: {} \n{}",
                automation.name, e
            ))),
        }
    }

    pub fn validate_app(&self, app: &AppConfig) -> anyhow::Result<()> {
        let context = ValidationContext {
            config: self.clone(),
            metadata: Some(ValidationContextMetadata::DataApp(
                DataAppValidationContext {
                    app_config: app.clone(),
                },
            )),
        };
        match app.validate_with(&context) {
            Ok(_) => Ok(()),
            Err(e) => anyhow::bail!(OxyError::ConfigurationError(format!(
                "Invalid app: {} \n{}",
                app.name, e
            ))),
        }
    }

    pub fn validate_workflows(&self) -> anyhow::Result<()> {
        for automation_file in self.list_workflows(&self.workspace_path) {
            let automation = self.load_workflow(&automation_file)?;
            self.validate_workflow(&automation)?;
        }
        Ok(())
    }

    pub fn validate_apps(&self) -> anyhow::Result<()> {
        for app_file in self.list_apps(&self.workspace_path) {
            let app = self.load_app(&app_file)?;
            self.validate_app(&app)?;
        }
        Ok(())
    }

    pub fn load_app(&self, app_file: &PathBuf) -> Result<AppConfig, OxyError> {
        if !app_file.exists() {
            return Err(OxyError::ConfigurationError(format!(
                "App configuration file not found: {app_file:?}"
            )));
        }

        let app_content = fs::read_to_string(app_file).map_err(|e| {
            OxyError::ArgumentError(format!(
                "Couldn't read app file {}: {e}",
                app_file.display()
            ))
        })?;

        let mut app: AppConfig = serde_yaml::from_str(&app_content).map_err(|e| {
            OxyError::ConfigurationError(format!(
                "Couldn't parse app file {}: {e}",
                app_file.display()
            ))
        })?;

        // Derive name from filename if not set
        let app_name = app_file.file_stem().unwrap().to_str().unwrap();
        let app_name = app_name.strip_suffix(".app").unwrap_or(app_name);
        app.name = app_name.to_string();

        Ok(app)
    }

    fn list_by_sub_extension(&self, dir: &PathBuf, sub_extension: &str) -> Vec<PathBuf> {
        let mut files = Vec::new();

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    files.extend(self.list_by_sub_extension(&path, sub_extension));
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

    pub fn list_workflows(&self, dir: &PathBuf) -> Vec<PathBuf> {
        let mut automations = self.list_by_sub_extension(dir, "procedure");
        automations.extend(self.list_by_sub_extension(dir, "workflow"));
        automations.extend(self.list_by_sub_extension(dir, "automation"));
        automations
    }

    pub fn list_apps(&self, dir: &PathBuf) -> Vec<PathBuf> {
        self.list_by_sub_extension(dir, "app")
    }

    pub fn list_agentic_agents(&self, dir: &PathBuf) -> Vec<PathBuf> {
        self.list_by_sub_extension(dir, "agentic")
    }

    pub fn load_workflow(&self, automation_path: &PathBuf) -> Result<Automation, OxyError> {
        if !automation_path.exists() {
            return Err(OxyError::ArgumentError(format!(
                "Automation configuration file not found: {automation_path:?}"
            )));
        }

        let automation_name = automation_path.file_stem().unwrap().to_str().unwrap();
        let automation_name = automation_name
            .strip_suffix(".procedure")
            .or_else(|| automation_name.strip_suffix(".automation"))
            .unwrap_or(automation_name);

        let automation_config =
            parse_automation_config(automation_name, &automation_path.to_string_lossy())?;

        Ok(automation_config)
    }

    pub fn load_semantic_model(
        &self,
        semantic_model_path: &PathBuf,
    ) -> anyhow::Result<SemanticModels> {
        if !semantic_model_path.exists() {
            anyhow::bail!(OxyError::ConfigurationError(format!(
                "Semantic model file not found: {semantic_model_path:?}"
            )));
        }

        let semantic_model = parse_semantic_model_config(&semantic_model_path.to_string_lossy())?;

        Ok(semantic_model)
    }

    pub fn default_model(&self) -> Option<String> {
        self.models.first().map(|m| m.name().to_string())
    }

    pub fn find_model(&self, model_name: &str) -> anyhow::Result<Model> {
        self.models
            .iter()
            .find(|m| m.name() == model_name)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Model not found").into())
    }

    pub fn find_database(&self, database_name: &str) -> anyhow::Result<Database> {
        self.databases
            .iter()
            .find(|w| w.name == database_name)
            .cloned()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("Database {database_name} not found"),
                )
                .into()
            })
    }
}

pub fn parse_config(config_path: &PathBuf, project_path: PathBuf) -> Result<Config, OxyError> {
    let config_str = fs::read_to_string(config_path)
        .map_err(|_e| OxyError::ConfigurationError("Unable to read config file".into()))?;

    let result = serde_yaml::from_str::<Config>(&config_str);
    match result {
        Ok(mut config) => {
            if config.slack_legacy.is_some() {
                tracing::warn!(
                    "config.yml contains a `slack:` section which is no longer read. \
                    Slack is now configured per-org via OAuth. \
                    Please remove the `slack:` section from your config.yml."
                );
            }
            config.workspace_path = project_path;
            let context = ValidationContext {
                config: config.clone(),
                metadata: None,
            };
            let validation_result = config
                .validate_with(&context)
                .map_err(|e| OxyError::ConfigurationError(e.to_string()));
            match validation_result {
                Ok(_) => Ok(config),
                Err(e) => Err(e),
            }
        }
        Err(e) => {
            let mut raw_error = e.to_string();
            raw_error = raw_error.replace("usize", "unsigned integer");
            Err(OxyError::ConfigurationError(format!(
                "Failed to parse config file:\n{raw_error}"
            )))
        }
    }
}

pub fn resolve_local_workspace_path() -> Result<PathBuf, OxyError> {
    let mut current_dir = std::env::current_dir().expect("Could not get current directory");

    for _ in 0..10 {
        let config_path = current_dir.join("config.yml");
        if config_path.exists() {
            return Ok(current_dir);
        }

        if !current_dir.pop() {
            break;
        }
    }

    Err(OxyError::RuntimeError(
        "Could not find config.yml".to_string(),
    ))
}
