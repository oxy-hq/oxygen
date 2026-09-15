use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::Parser;
use clap::builder::ValueParser;
use minijinja::{Environment, Value};
use uuid::Uuid;

use ::oxy::adapters::secrets::SecretsManager;
use ::oxy::adapters::workspace::builder::WorkspaceBuilder;
use ::oxy::config::{ConfigBuilder, ConfigManager, resolve_local_workspace_path};
use ::oxy::connector::Connector;
use ::oxy::exec_types::utils::record_batches_to_table;
use ::oxy::sentry_config;
use ::oxy::utils::print_colored_sql;
use oxy::config::WorkingCopy;
use oxy_shared::errors::OxyError;

type Variable = (String, String);

fn parse_variable(env: &str) -> Result<Variable, OxyError> {
    if let Some((var, value)) = env.split_once('=') {
        Ok((var.to_owned(), value.to_owned()))
    } else {
        Err(OxyError::ArgumentError(
            "Invalid variable format. Must be in the form of VAR=VALUE".to_string(),
        ))
    }
}

#[derive(Parser, Debug)]
pub struct RunArgs {
    /// Path to the file to execute (.sql, .automation.yml, or .procedure.yml)
    pub(super) file: String,

    /// Database connection to use for SQL execution
    ///
    /// Specify which database from your config.yml to use.
    /// If not provided, uses the default database from configuration.
    #[clap(long)]
    pub(super) database: Option<String>,

    /// Template variables in the format VAR=VALUE
    ///
    /// Pass variables to SQL templates using Jinja2 syntax.
    /// Example: --variables user_id=123 --variables status=active
    #[clap(long, short = 'v', value_parser=ValueParser::new(parse_variable), num_args = 1..)]
    pub(super) variables: Vec<(String, String)>,

    /// Reserved for future agentic CLI integration.
    pub(super) question: Option<String>,

    /// Retry failed operations automatically
    ///
    /// Enable automatic retry logic for transient failures
    /// during automation or query execution.
    #[clap(long, default_value_t = false, group = "named")]
    pub(super) retry: bool,

    /// Retry from a specific step in the automation
    #[clap(long, group = "unnamed", conflicts_with = "named")]
    pub(super) retry_from: Option<String>,

    /// Preview SQL without executing against the database
    ///
    /// Validate and display the generated SQL query without
    /// actually running it against your database.
    #[clap(long, default_value_t = false)]
    pub(super) dry_run: bool,
}

#[derive(Clone)]
pub struct RunOptions {
    pub database: Option<String>,
    pub variables: Option<Vec<(String, String)>>,
    pub question: Option<String>,
    pub retry: bool,
    pub dry_run: bool,
}

impl RunArgs {
    pub fn new(file: String, options: Option<RunOptions>) -> Self {
        match options {
            Some(options) => Self {
                file,
                database: options.database,
                variables: options.variables.unwrap_or(vec![]),
                question: options.question,
                retry: options.retry,
                dry_run: options.dry_run,
                retry_from: None,
            },
            None => Self {
                file,
                database: None,
                variables: vec![],
                question: None,
                retry: false,
                dry_run: false,
                retry_from: None,
            },
        }
    }
}

pub enum RunResult {
    Automation,
    Sql(String),
}

pub async fn handle_run_command(run_args: RunArgs) -> Result<RunResult, OxyError> {
    let file = &run_args.file;

    let current_dir = std::env::current_dir()
        .map_err(|e| OxyError::RuntimeError(format!("Could not get current directory: {e}")))?;

    let file_path = current_dir.join(file);
    if !file_path.exists() {
        return Err(OxyError::ConfigurationError(format!(
            "File not found: {file_path:?}"
        )));
    }

    let extension = file_path.extension().and_then(std::ffi::OsStr::to_str);

    // Extract the compound extension (the part before the final `.yml`/`.yaml`/`.sql`).
    // For example, `my.automation.yml` → outer_ext = "yml", stem_ext = "automation".
    let stem_ext = file_path
        .file_stem()
        .and_then(|stem| std::path::Path::new(stem).extension())
        .and_then(std::ffi::OsStr::to_str);

    match (extension, stem_ext) {
        (Some("yml") | Some("yaml"), Some("procedure" | "automation")) => {
            handle_automation_file(&file_path, run_args.retry, run_args.retry_from).await?;
            Ok(RunResult::Automation)
        }
        (Some("yml") | Some("yaml"), _) => Err(OxyError::ArgumentError(
            "Invalid YAML file. Must be either *.automation.yml or *.procedure.yml".into(),
        )),
        (Some("sql"), _) => {
            let config = ConfigBuilder::new()
                .with_workspace_path(&resolve_local_workspace_path()?)?
                .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Fail)
                .await?;
            let database = run_args
                .database
                .or_else(|| config.default_database_ref().cloned());

            if database.is_none() {
                return Err(OxyError::ArgumentError(
                    "Database is required for running SQL file. Please provide the database using --database or set a default database in config.yml".into(),
                ));
            }
            let sql_result = handle_sql_file(
                &file_path,
                database.unwrap(),
                &config,
                &run_args.variables,
                run_args.dry_run,
            )
            .await?;
            Ok(RunResult::Sql(sql_result))
        }
        _ => Err(OxyError::ArgumentError(
            "Invalid file extension. Must be .automation.yml, .procedure.yml, or .sql".into(),
        )),
    }
}

async fn handle_automation_file(
    automation_path: &PathBuf,
    _retry: bool,
    _retry_from: Option<String>,
) -> Result<(), OxyError> {
    use std::sync::Arc;

    use crate::agentic_wiring::OxyProjectContext;
    use ::oxy::theme::StyledText;

    let workspace_path = resolve_local_workspace_path()?;
    let automation_name_str = automation_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    sentry_config::add_automation_context(automation_name_str, None);

    // CLI runs are stateless: nil ids and a noop runs manager so the user
    // doesn't need OXY_DATABASE_URL set just to execute an automation.
    let workspace_manager = WorkspaceBuilder::new(Uuid::nil())
        .with_working_copy(&workspace_path, None, oxy::config::OnMissing::Fail)
        .await?
        .build()
        .await
        .map_err(|e| OxyError::from(anyhow::anyhow!("Failed to create project: {e}")))?;

    // Read + parse the automation YAML directly from disk. We bypass
    // `WorkspaceContext::resolve_automation_yaml` (which expects relative
    // paths) because the user gave us an absolute filesystem path.
    let yaml = tokio::fs::read_to_string(automation_path)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("read {}: {e}", automation_path.display())))?;
    let automation: agentic_automation::AutomationConfig = serde_yaml::from_str(&yaml)
        .map_err(|e| OxyError::RuntimeError(format!("parse automation YAML: {e}")))?;

    let project_ctx = Arc::new(OxyProjectContext::new(workspace_manager));
    let workspace: Arc<dyn agentic_automation::WorkspaceContext> = project_ctx;

    let results = agentic_pipeline::automation_run::run_inline_automation_with(
        workspace.as_ref(),
        automation,
        None,
        None,
    )
    .await
    .map_err(|e| OxyError::RuntimeError(format!("inline automation: {e}")))?;

    // Render results in automation-task order — the HashMap key set is
    // the same, but the iteration order isn't deterministic.
    let automation_again: agentic_automation::AutomationConfig =
        serde_yaml::from_str(&yaml).expect("re-parse automation YAML (already validated above)");
    println!("{}", format!("✓ {}", automation_name_str).success());
    for task in &automation_again.tasks {
        if let Some(value) = results.get(&task.name) {
            println!("{}", format!("  {}", task.name).primary());
            let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
            for line in pretty.lines() {
                println!("    {line}");
            }
        }
    }
    Ok(())
}

async fn handle_sql_file(
    file_path: &PathBuf,
    database: String,
    config: &ConfigManager<WorkingCopy>,
    variables: &[(String, String)],
    dry_run: bool,
) -> Result<String, OxyError> {
    sentry_config::add_database_context(&database, Some("sql_file"));
    sentry_config::add_operation_context("sql", Some(&file_path.to_string_lossy()));

    let content = std::fs::read_to_string(file_path)
        .map_err(|e| OxyError::RuntimeError(format!("Failed to read SQL file: {e}")))?;
    let mut env = Environment::new();
    let mut query = content.clone();

    // Handle variable templating if variables are provided
    if !variables.is_empty() {
        env.add_template("query", &query)
            .map_err(|e| OxyError::RuntimeError(format!("Failed to parse SQL template: {e}")))?;
        let tmpl = env.get_template("query").unwrap();
        let ctx = Value::from({
            let mut m = BTreeMap::new();
            for var in variables {
                m.insert(var.0.clone(), var.1.clone());
            }
            m
        });
        query = tmpl
            .render(ctx)
            .map_err(|e| OxyError::RuntimeError(format!("Failed to render SQL template: {e}")))?
    }

    print_colored_sql(&query);
    let secrets_manager = SecretsManager::from_environment()?;
    // CLI runs as the local guest with no per-user identity. Catch the
    // airhouse_managed case here so the user sees a single friendly
    // message instead of the verbose ConfigurationError out of `from_db`.
    oxy::connector::reject_airhouse_managed_for_system_path(
        config,
        &database,
        "Running queries from the CLI against",
    )?;
    let connector = Connector::from_database(
        &database,
        config,
        &secrets_manager,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await?;
    let (datasets, schema) = match dry_run {
        false => connector.run_query_and_load(&query).await,
        true => connector.dry_run(&query).await,
    }?;
    let batches_display = record_batches_to_table(&datasets, &schema)
        .map_err(|e| OxyError::RuntimeError(format!("Failed to display query results: {e}")))?;
    println!("\n\x1b[1;32mResults:\x1b[0m");
    println!("{batches_display}");

    Ok(batches_display.to_string())
}
