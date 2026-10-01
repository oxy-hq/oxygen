use crate::config::model::AppConfig;

use super::model::{Config, ExportFormat, TaskExport, TaskType};
use std::{env, fmt::Display, path::PathBuf};

const FILE_NOT_FOUND_ERROR: &str = "File does not exist";
const FILE_SAME_DIR_ERROR: &str = "File must be in the same directory as the config file";
const DIR_NOT_FOUND_ERROR: &str = "Directory does not exist";
const ENV_VAR_NOT_FOUND_ERROR: &str = "Env var not set";
const SQL_FILE_NOT_FOUND_ERROR: &str = "Sql file not found";
const DATABASE_NOT_FOUND_ERROR: &str = "Database not found";
const INVALID_EXPORT_FORMAT_ERROR: &str = "Invalid export format";

fn format_error_message(error_message: &str, value: impl Display) -> garde::Error {
    garde::Error::new(format!("{error_message} ({value})"))
}

pub fn validate_file_path(path: &PathBuf, context: &ValidationContext) -> garde::Result {
    if path.is_absolute() || path.components().count() > 1 {
        return Err(format_error_message(
            FILE_SAME_DIR_ERROR,
            path.to_string_lossy(),
        ));
    }

    let file_path = context.config.workspace_path.join(path);

    if !file_path.exists() {
        return Err(format_error_message(
            FILE_NOT_FOUND_ERROR,
            file_path.to_string_lossy(),
        ));
    }
    Ok(())
}

pub fn validate_optional_file_path(
    path: &Option<PathBuf>,
    context: &ValidationContext,
) -> garde::Result {
    if let Some(path) = path {
        validate_file_path(path, context)
    } else {
        Ok(())
    }
}

pub fn validate_optional_private_key_path(
    path: &Option<PathBuf>,
    _context: &ValidationContext,
) -> garde::Result {
    if let Some(path) = path {
        // For private keys, allow absolute paths since they're often stored in secure locations
        // Just check that the file exists
        if !path.exists() {
            return Err(format_error_message(
                FILE_NOT_FOUND_ERROR,
                path.to_string_lossy(),
            ));
        }
    }
    Ok(())
}

pub fn validation_directory_path(path: &PathBuf, _: &ValidationContext) -> garde::Result {
    if !path.is_dir() {
        return Err(format_error_message(
            DIR_NOT_FOUND_ERROR,
            path.as_path().to_string_lossy(),
        ));
    }
    Ok(())
}

pub fn validate_env_var(env_var: &str, _: &ValidationContext) -> garde::Result {
    match env::var(env_var) {
        Ok(_) => Ok(()),
        Err(_) => Err(format_error_message(ENV_VAR_NOT_FOUND_ERROR, env_var)),
    }
}

pub struct DataAppValidationContext {
    pub app_config: AppConfig,
}

pub enum ValidationContextMetadata {
    DataApp(DataAppValidationContext),
}

pub struct ValidationContext {
    pub config: Config,
    pub metadata: Option<ValidationContextMetadata>,
}

/// The prefix a workspace-preview run scopes every side-effecting name with
/// (`preview:<run_id>:<name>`; `agentic_automation::preview_names`, which this
/// platform crate may not import). A database named with it would let a preview
/// run's scoped name resolve on a pod that does not know previews — the fence
/// the prefix exists to be.
pub const RESERVED_DATABASE_PREFIX: &str = "preview:";

pub fn validate_database_name(name: &str, _: &ValidationContext) -> garde::Result {
    if name.starts_with(RESERVED_DATABASE_PREFIX) {
        return Err(garde::Error::new(format!(
            "database name `{name}` must not start with `{RESERVED_DATABASE_PREFIX}`: \
             the prefix is reserved for workspace previews"
        )));
    }
    Ok(())
}

pub fn validate_database_exists(database_name: &str, context: &ValidationContext) -> garde::Result {
    let database = context.config.find_database(database_name);
    match database {
        Ok(_) => Ok(()),
        Err(_) => Err(format_error_message(
            DATABASE_NOT_FOUND_ERROR,
            database_name,
        )),
    }
}

pub fn validate_sql_file(sql_file: &str, context: &ValidationContext) -> garde::Result {
    let path = &context.config.workspace_path.join(sql_file);
    if !path.exists() {
        return Err(format_error_message(
            SQL_FILE_NOT_FOUND_ERROR,
            path.as_path().to_string_lossy(),
        ));
    }
    Ok(())
}

pub fn validate_omni_integration_exists(
    integration_name: &str,
    context: &ValidationContext,
) -> garde::Result {
    let integration_exists = context.config.integrations.iter().any(|integration| {
        integration.name == integration_name
            && matches!(
                &integration.integration_type,
                crate::config::model::IntegrationType::Omni(_)
            )
    });

    if integration_exists {
        Ok(())
    } else {
        Err(format_error_message(
            "Integration not found",
            integration_name,
        ))
    }
}

pub fn validate_looker_integration_exists(
    integration_name: &str,
    context: &ValidationContext,
) -> garde::Result {
    let integration_exists = context.config.integrations.iter().any(|integration| {
        integration.name == integration_name
            && matches!(
                &integration.integration_type,
                crate::config::model::IntegrationType::Looker(_)
            )
    });

    if integration_exists {
        Ok(())
    } else {
        Err(format_error_message(
            "Looker integration not found",
            integration_name,
        ))
    }
}

pub fn validate_task(task_type: &TaskType, _context: &ValidationContext) -> garde::Result {
    match task_type {
        TaskType::Agent(task) => validate_export(
            task.export.as_ref(),
            &[ExportFormat::JSON, ExportFormat::CSV, ExportFormat::SQL],
            "agent",
        ),
        TaskType::ExecuteSQL(task) => validate_export(
            task.export.as_ref(),
            &[ExportFormat::JSON, ExportFormat::CSV, ExportFormat::SQL],
            "ExecuteSQL",
        ),
        TaskType::Formatter(task) => validate_export(
            task.export.as_ref(),
            &[ExportFormat::TXT, ExportFormat::DOCX],
            "Formatter",
        ),
        TaskType::SemanticQuery(task) => validate_export(
            task.export.as_ref(),
            &[ExportFormat::JSON, ExportFormat::CSV, ExportFormat::SQL],
            "SemanticQuery",
        ),
        TaskType::OmniQuery(task) => validate_export(
            task.export.as_ref(),
            &[ExportFormat::JSON, ExportFormat::CSV, ExportFormat::SQL],
            "OmniQuery",
        ),
        TaskType::LookerQuery(task) => validate_export(
            task.export.as_ref(),
            &[ExportFormat::JSON, ExportFormat::CSV, ExportFormat::SQL],
            "LookerQuery",
        ),
        TaskType::SubAutomation(_)
        | TaskType::LoopSequential(_)
        | TaskType::HttpRequest(_)
        // AirwayTask has no `export:` — its output is rows in the warehouse,
        // not a file artifact.
        | TaskType::Airway(_)
        | TaskType::Unknown => Ok(()),
        TaskType::Conditional(_) => Ok(()),
    }
}

fn validate_export(
    export: Option<&TaskExport>,
    allowed_formats: &[ExportFormat],
    task_name: &str,
) -> garde::Result {
    if let Some(export) = export
        && !allowed_formats.contains(&export.format)
    {
        return Err(garde::Error::new(format!(
            "{}: {:?}, only supports {:?} for {} task",
            INVALID_EXPORT_FORMAT_ERROR, export.format, allowed_formats, task_name
        )));
    }
    Ok(())
}

pub fn validate_task_data_reference(data_ref: &String, ctx: &ValidationContext) -> garde::Result {
    if let Some(ValidationContextMetadata::DataApp(data_app_ctx)) = &ctx.metadata {
        let task_names: std::collections::HashSet<String> = data_app_ctx
            .app_config
            .tasks
            .iter()
            .map(|t| t.name.clone())
            .collect();

        // For dot notation references like "workflow.task", validate the first part (automation name)
        // The nested task part can't be validated statically since it requires loading sub-automations
        let task_to_check = if let Some(dot_pos) = data_ref.find('.') {
            &data_ref[..dot_pos]
        } else {
            data_ref.as_str()
        };

        if !task_names.contains(task_to_check) {
            return Err(garde::Error::new(format!(
                "Display block references task '{task_to_check}' which does not exist in the app config"
            )));
        }
    }
    Ok(())
}

pub fn validate_consistency_prompt(
    prompt: &Option<String>,
    _context: &ValidationContext,
) -> garde::Result {
    if let Some(prompt_str) = prompt {
        match minijinja::Environment::new()
            .add_template_owned("test".to_string(), prompt_str.to_string())
        {
            Ok(_) => Ok(()),
            Err(e) => Err(garde::Error::new(format!(
                "Invalid consistency prompt template syntax: {}",
                e
            ))),
        }
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_context(project_path: PathBuf) -> ValidationContext {
        ValidationContext {
            config: Config {
                defaults: None,
                models: vec![],
                databases: vec![],
                builder_agent: None,
                timezone: None,
                workspace_path: project_path,
                repositories: vec![],
                integrations: vec![],
                slack_legacy: None,
                mcp: None,
                protected_branches: None,
                base_branch: None,
                admins: vec![],
                health_check: None,
                pre_aggregations: None,
            },
            metadata: None,
        }
    }

    /// `preview:` names a workspace-preview run's scoped database; a real
    /// database may not start with it (see `RESERVED_DATABASE_PREFIX`).
    #[test]
    fn a_database_name_cannot_start_with_preview() {
        let ctx = create_test_context(PathBuf::new());
        for bad in ["preview:", "preview:run:clickhouse"] {
            assert!(validate_database_name(bad, &ctx).is_err(), "{bad}");
        }
        for ok in ["clickhouse", "preview", "previews", "my_preview:db"] {
            assert!(validate_database_name(ok, &ctx).is_ok(), "{ok}");
        }
    }

    mod validate_file_path_tests {
        use super::*;

        #[test]
        fn test_existing_file_in_same_dir_succeeds() {
            let temp_dir = TempDir::new().unwrap();
            let file_path = temp_dir.path().join("test.txt");
            std::fs::write(&file_path, "test content").unwrap();

            let context = create_test_context(temp_dir.path().to_path_buf());
            let path = PathBuf::from("test.txt");

            let result = validate_file_path(&path, &context);
            assert!(result.is_ok());
        }

        #[test]
        fn test_non_existing_file_fails() {
            let temp_dir = TempDir::new().unwrap();
            let context = create_test_context(temp_dir.path().to_path_buf());
            let path = PathBuf::from("nonexistent.txt");

            let result = validate_file_path(&path, &context);
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("File does not exist"));
        }

        #[test]
        fn test_absolute_path_fails() {
            let temp_dir = TempDir::new().unwrap();
            let context = create_test_context(temp_dir.path().to_path_buf());
            let path = PathBuf::from("/absolute/path/file.txt");

            let result = validate_file_path(&path, &context);
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("same directory"));
        }

        #[test]
        fn test_nested_path_fails() {
            let temp_dir = TempDir::new().unwrap();
            let nested_dir = temp_dir.path().join("subdir");
            std::fs::create_dir(&nested_dir).unwrap();
            std::fs::write(nested_dir.join("file.txt"), "content").unwrap();

            let context = create_test_context(temp_dir.path().to_path_buf());
            let path = PathBuf::from("subdir/file.txt");

            let result = validate_file_path(&path, &context);
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("same directory"));
        }
    }

    mod validate_optional_file_path_tests {
        use super::*;

        #[test]
        fn test_none_succeeds() {
            let temp_dir = TempDir::new().unwrap();
            let context = create_test_context(temp_dir.path().to_path_buf());

            let result = validate_optional_file_path(&None, &context);
            assert!(result.is_ok());
        }

        #[test]
        fn test_some_existing_file_succeeds() {
            let temp_dir = TempDir::new().unwrap();
            let file_path = temp_dir.path().join("test.txt");
            std::fs::write(&file_path, "test").unwrap();

            let context = create_test_context(temp_dir.path().to_path_buf());
            let path = Some(PathBuf::from("test.txt"));

            let result = validate_optional_file_path(&path, &context);
            assert!(result.is_ok());
        }

        #[test]
        fn test_some_non_existing_file_fails() {
            let temp_dir = TempDir::new().unwrap();
            let context = create_test_context(temp_dir.path().to_path_buf());
            let path = Some(PathBuf::from("missing.txt"));

            let result = validate_optional_file_path(&path, &context);
            assert!(result.is_err());
        }
    }

    mod validate_env_var_tests {
        use super::*;

        #[test]
        fn test_existing_env_var_succeeds() {
            // PATH is almost always set
            let temp_dir = TempDir::new().unwrap();
            let context = create_test_context(temp_dir.path().to_path_buf());

            let result = validate_env_var("PATH", &context);
            assert!(result.is_ok());
        }

        #[test]
        fn test_non_existing_env_var_fails() {
            let temp_dir = TempDir::new().unwrap();
            let context = create_test_context(temp_dir.path().to_path_buf());

            let result = validate_env_var("DEFINITELY_NOT_SET_VAR_12345", &context);
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("Env var not set"));
        }
    }

    mod validation_directory_path_tests {
        use super::*;

        #[test]
        fn test_existing_directory_succeeds() {
            let temp_dir = TempDir::new().unwrap();
            let context = create_test_context(temp_dir.path().to_path_buf());
            let path = temp_dir.path().to_path_buf();

            let result = validation_directory_path(&path, &context);
            assert!(result.is_ok());
        }

        #[test]
        fn test_non_existing_directory_fails() {
            let temp_dir = TempDir::new().unwrap();
            let context = create_test_context(temp_dir.path().to_path_buf());
            let path = temp_dir.path().join("nonexistent_dir");

            let result = validation_directory_path(&path, &context);
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("Directory does not exist"));
        }

        #[test]
        fn test_file_path_as_directory_fails() {
            let temp_dir = TempDir::new().unwrap();
            let file_path = temp_dir.path().join("file.txt");
            std::fs::write(&file_path, "content").unwrap();

            let context = create_test_context(temp_dir.path().to_path_buf());

            let result = validation_directory_path(&file_path, &context);
            assert!(result.is_err());
        }
    }

    mod validate_consistency_prompt_tests {
        use super::*;

        #[test]
        fn test_none_succeeds() {
            let temp_dir = TempDir::new().unwrap();
            let context = create_test_context(temp_dir.path().to_path_buf());

            let result = validate_consistency_prompt(&None, &context);
            assert!(result.is_ok());
        }

        #[test]
        fn test_valid_template_succeeds() {
            let temp_dir = TempDir::new().unwrap();
            let context = create_test_context(temp_dir.path().to_path_buf());
            let prompt = Some("Hello {{ name }}, welcome!".to_string());

            let result = validate_consistency_prompt(&prompt, &context);
            assert!(result.is_ok());
        }

        #[test]
        fn test_plain_text_succeeds() {
            let temp_dir = TempDir::new().unwrap();
            let context = create_test_context(temp_dir.path().to_path_buf());
            let prompt = Some("Just plain text without variables".to_string());

            let result = validate_consistency_prompt(&prompt, &context);
            assert!(result.is_ok());
        }

        #[test]
        fn test_invalid_template_syntax_fails() {
            let temp_dir = TempDir::new().unwrap();
            let context = create_test_context(temp_dir.path().to_path_buf());
            // Unclosed braces
            let prompt = Some("Hello {{ name".to_string());

            let result = validate_consistency_prompt(&prompt, &context);
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("Invalid consistency prompt"));
        }

        #[test]
        fn test_complex_valid_template_succeeds() {
            let temp_dir = TempDir::new().unwrap();
            let context = create_test_context(temp_dir.path().to_path_buf());
            let prompt = Some("{% for item in items %}{{ item.name }}{% endfor %}".to_string());

            let result = validate_consistency_prompt(&prompt, &context);
            assert!(result.is_ok());
        }
    }

    mod validate_sql_file_tests {
        use super::*;

        #[test]
        fn test_existing_sql_file_succeeds() {
            let temp_dir = TempDir::new().unwrap();
            let sql_path = temp_dir.path().join("query.sql");
            std::fs::write(&sql_path, "SELECT * FROM users").unwrap();

            let context = create_test_context(temp_dir.path().to_path_buf());

            let result = validate_sql_file("query.sql", &context);
            assert!(result.is_ok());
        }

        #[test]
        fn test_non_existing_sql_file_fails() {
            let temp_dir = TempDir::new().unwrap();
            let context = create_test_context(temp_dir.path().to_path_buf());

            let result = validate_sql_file("missing.sql", &context);
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("Sql file not found"));
        }
    }

    mod validate_export_tests {
        use super::*;

        #[test]
        fn test_none_export_succeeds() {
            let result = validate_export(None, &[ExportFormat::JSON], "test");
            assert!(result.is_ok());
        }

        #[test]
        fn test_valid_format_succeeds() {
            let export = TaskExport {
                format: ExportFormat::JSON,
                path: "output.json".to_string(),
            };
            let result = validate_export(
                Some(&export),
                &[ExportFormat::JSON, ExportFormat::CSV],
                "test",
            );
            assert!(result.is_ok());
        }

        #[test]
        fn test_invalid_format_fails() {
            let export = TaskExport {
                format: ExportFormat::TXT,
                path: "output.txt".to_string(),
            };
            let result = validate_export(
                Some(&export),
                &[ExportFormat::JSON, ExportFormat::CSV],
                "TestTask",
            );
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("Invalid export format"));
            assert!(err.to_string().contains("TestTask"));
        }
    }

    mod validate_optional_private_key_path_tests {
        use super::*;

        #[test]
        fn test_none_succeeds() {
            let temp_dir = TempDir::new().unwrap();
            let context = create_test_context(temp_dir.path().to_path_buf());

            let result = validate_optional_private_key_path(&None, &context);
            assert!(result.is_ok());
        }

        #[test]
        fn test_existing_absolute_path_succeeds() {
            let temp_dir = TempDir::new().unwrap();
            let key_path = temp_dir.path().join("private.key");
            std::fs::write(&key_path, "PRIVATE KEY").unwrap();

            let context = create_test_context(temp_dir.path().to_path_buf());

            let result = validate_optional_private_key_path(&Some(key_path), &context);
            assert!(result.is_ok());
        }

        #[test]
        fn test_non_existing_path_fails() {
            let temp_dir = TempDir::new().unwrap();
            let context = create_test_context(temp_dir.path().to_path_buf());
            let key_path = temp_dir.path().join("nonexistent.key");

            let result = validate_optional_private_key_path(&Some(key_path), &context);
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("File does not exist"));
        }
    }
}
