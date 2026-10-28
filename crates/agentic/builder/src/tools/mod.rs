//! Built-in tools for the builder copilot.
//!
//! - `search_files`          — glob pattern search across the project directory
//! - `read_file`             — read file content (optionally by line range)
//! - `search_text`           — grep-like text search across the codebase
//! - `write_file`            — create a new file or fully overwrite an existing one (HITL-gated)
//! - `edit_file`             — exact-string replacement in an existing file (HITL-gated)
//! - `delete_file`           — delete an existing file (HITL-gated)
//! - `manage_directory`      — create, delete, or rename a directory with user confirmation
//! - `validate_project`      — validate project files against schema using the oxy config validator
//! - `lookup_reference`      — load a domain reference card (semantic-layer/agentic-builder)
//! - `run_tests`             — run one or more `.test.yml` files using the oxy eval pipeline
//! - `execute_sql`           — run a SQL query against a configured database
//! - `semantic_query`        — compile and execute a semantic model query
//! - `list_dbt_projects`     — list all airform/dbt projects in the workspace
//! - `list_dbt_nodes`        — list models, seeds, tests, and sources in a project
//! - `compile_dbt_model`     — compile one or all dbt models to SQL
//! - `run_dbt_models`        — execute dbt models and write Parquet outputs
//! - `test_dbt_models`       — run dbt data-quality tests
//! - `get_dbt_lineage`       — return the model-level dependency DAG
//! - `analyze_dbt_project`   — analyze SQL correctness, schemas, and contract violations
//! - `get_dbt_column_lineage`— return the column-level lineage DAG
//! - `parse_dbt_project`     — parse the manifest and validate the DAG
//! - `seed_dbt_project`      — load seed CSV files into the execution context
//! - `debug_dbt_project`     — health-check the project config and compilation
//! - `clean_dbt_project`     — remove target/ and other clean-target directories
//! - `docs_generate_dbt`     — write manifest.json documentation artifact
//! - `format_dbt_sql`        — uppercase SQL keywords in model files
//! - `init_dbt_project`      — scaffold a new dbt project under modeling/
//!
//! All file-system operations are sandboxed to the project root.

mod airform;
mod delete_file;
mod edit_file;
mod execute_sql;
mod lookup_reference;
mod lookup_schema;
mod manage_directory;
mod read_file;
mod run_tests;
mod search_files;
mod search_text;
mod semantic_query;
mod utils;
mod validate_project;
mod write_file;

use agentic_core::tools::ToolDef;

use crate::schema_provider::BuilderSchemaProvider;
pub use airform::{
    execute_analyze_dbt_project, execute_clean_dbt_project, execute_compile_dbt_model_all,
    execute_compile_dbt_model_single, execute_debug_dbt_project, execute_docs_generate_dbt,
    execute_format_dbt_sql, execute_get_dbt_column_lineage, execute_get_dbt_lineage,
    execute_init_dbt_project, execute_list_dbt_nodes, execute_list_dbt_projects,
    execute_parse_dbt_project, execute_run_dbt_models, execute_seed_dbt_project,
    execute_test_dbt_models,
};

pub use delete_file::execute_delete_file;
pub use edit_file::{apply_edit, execute_edit_file};
pub use execute_sql::execute_execute_sql;
pub use lookup_reference::execute_lookup_reference;
pub use lookup_schema::execute_lookup_schema;
pub use manage_directory::execute_manage_directory;
pub use read_file::execute_read_file;
pub use run_tests::execute_run_tests;
pub use search_files::execute_search_files;
pub use search_text::execute_search_text;
pub use semantic_query::execute_semantic_query;
pub use utils::{hitl_confirm, remove_file, safe_path, write_file_content};
pub use validate_project::execute_validate_project;
pub use write_file::execute_write_file;
pub(crate) use write_file::is_data_app_path;

/// All tools available to the builder copilot.
pub fn all_tools(schema_provider: &dyn BuilderSchemaProvider) -> Vec<ToolDef> {
    vec![
        search_files::search_files_def(),
        read_file::read_file_def(),
        search_text::search_text_def(),
        write_file::write_file_def(),
        edit_file::edit_file_def(),
        delete_file::delete_file_def(),
        validate_project::validate_project_def(),
        lookup_reference::lookup_reference_def(),
        lookup_schema::lookup_schema_def(schema_provider),
        run_tests::run_tests_def(),
        execute_sql::execute_sql_def(),
        semantic_query::semantic_query_def(),
        airform::list_dbt_projects_def(),
        airform::list_dbt_nodes_def(),
        airform::compile_dbt_model_def(),
        airform::run_dbt_models_def(),
        airform::test_dbt_models_def(),
        airform::get_dbt_lineage_def(),
        airform::analyze_dbt_project_def(),
        airform::get_dbt_column_lineage_def(),
        airform::parse_dbt_project_def(),
        airform::seed_dbt_project_def(),
        airform::debug_dbt_project_def(),
        airform::clean_dbt_project_def(),
        airform::docs_generate_dbt_def(),
        airform::format_dbt_sql_def(),
        airform::init_dbt_project_def(),
        manage_directory::manage_directory_def(),
        agentic_core::tools::ask_user_tool_def(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema_provider::EmptySchemaProvider;

    #[test]
    fn no_tool_runs_or_builds_a_data_app() {
        let names: Vec<&str> = all_tools(&EmptySchemaProvider)
            .iter()
            .map(|t| t.name)
            .collect();
        assert!(!names.contains(&"run_app"), "{names:?}");
    }
}
