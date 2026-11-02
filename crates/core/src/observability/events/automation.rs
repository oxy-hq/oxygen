//! Automation execution observability events
//!
//! This module provides utilities for logging automation and task execution events
//! with consistent span creation and field recording.

use tracing::{Level, Span, event};

/// Service-level events for automation entry points (run_workflow, run_workflow_v2)
/// Note: No metrics recording here - that happens in launcher::launch
pub mod run_workflow {
    use super::*;

    pub static NAME: &str = "workflow.run_workflow";
    pub static TYPE: &str = "workflow";
    pub static INPUT: &str = "workflow.run_workflow.input";
    pub static OUTPUT: &str = "workflow.run_workflow.output";

    pub fn input(workflow_ref: &str, retry_strategy: &str) {
        event!(
            Level::INFO,
            name = INPUT,
            is_visible = true,
            workflow_ref = %workflow_ref,
            retry_strategy = %retry_strategy
        );
    }

    /// Record automation output event (service layer - no metrics, just tracing).
    /// DEBUG level: the output payload is verbose and floods the console;
    /// observability backends capture it independently.
    pub fn output(output: &crate::exec_types::OutputContainer) {
        event!(
            Level::DEBUG,
            name = OUTPUT,
            is_visible = true,
            status = "success",
            output = %serde_json::to_string(output).unwrap_or_default()
        );
    }

    /// Record automation error event (service layer - no metrics, just tracing)
    pub fn error(error: &str) {
        event!(
            Level::ERROR,
            name = OUTPUT,
            is_visible = true,
            status = "error",
            error = %error
        );
    }
}

/// Constants and logging functions for automation launcher spans
pub mod launcher {
    use super::*;

    pub mod with_workspace {
        use super::*;

        pub static NAME: &str = "workflow.launcher.with_workspace";
        pub static TYPE: &str = "workflow";
        pub static INPUT: &str = "workflow.launcher.with_workspace.input";
        pub static OUTPUT: &str = "workflow.launcher.with_workspace.output";

        pub fn input(project_path: &str) {
            event!(
                Level::INFO,
                name = INPUT,
                is_visible = true,
                project_path = %project_path
            );
        }

        pub fn output() {
            event!(
                Level::INFO,
                name = OUTPUT,
                is_visible = true,
                status = "success"
            );
        }
    }

    pub mod get_global_context {
        use super::*;

        pub static NAME: &str = "workflow.launcher.get_global_context";
        pub static TYPE: &str = "workflow";
        pub static INPUT: &str = "workflow.launcher.get_global_context.input";
        pub static OUTPUT: &str = "workflow.launcher.get_global_context.output";

        pub fn input() {
            event!(Level::INFO, name = INPUT, is_visible = true);
        }

        pub fn output(has_models: bool, has_dimensions: bool) {
            event!(
                Level::INFO,
                name = OUTPUT,
                is_visible = true,
                status = "success",
                has_models = has_models,
                has_dimensions = has_dimensions
            );
        }
    }
}

/// Constants and logging functions for task execution spans
pub mod task {
    use super::*;

    pub mod execute {
        use super::*;

        pub static NAME: &str = "workflow.task.execute";
        pub static TYPE: &str = "task";
        pub static INPUT: &str = "workflow.task.execute.input";
        pub static OUTPUT: &str = "workflow.task.execute.output";

        pub fn input(task_name: &str, task_type: &str) {
            event!(
                Level::INFO,
                name = INPUT,
                is_visible = true,
                task_name = %task_name,
                task_type = %task_type
            );
        }

        pub fn output(task_name: &str, output: &crate::exec_types::OutputContainer) {
            event!(
                Level::DEBUG,
                name = OUTPUT,
                is_visible = true,
                status = "success",
                task_name = %task_name,
                output = %serde_json::to_string(output).unwrap_or_default()
            );
        }

        pub fn error(task_name: &str, error: &str) {
            event!(
                Level::ERROR,
                name = OUTPUT,
                is_visible = true,
                status = "error",
                task_name = %task_name,
                error = %error
            );
        }
    }

    pub mod agent {
        use super::*;

        pub static NAME: &str = "workflow.task.agent.execute";
        pub static TYPE: &str = "agent";
        pub static INPUT: &str = "workflow.task.agent.input";
        pub static OUTPUT: &str = "workflow.task.agent.output";

        pub fn input(agent_ref: &str, prompt: &str, consistency_run: usize) {
            event!(
                Level::DEBUG,
                name = INPUT,
                is_visible = true,
                agent_ref = %agent_ref,
                prompt = %prompt,
                consistency_run = consistency_run
            );
        }

        pub fn output(agent_ref: &str, output: &crate::exec_types::OutputContainer) {
            event!(
                Level::DEBUG,
                name = OUTPUT,
                is_visible = true,
                status = "success",
                agent_ref = %agent_ref,
                output = %serde_json::to_string(output).unwrap_or_default()
            );
        }
    }

    pub mod semantic_query {
        use super::*;

        pub static NAME_RENDER: &str = "workflow.task.semantic_query.render";
        pub static NAME_MAP: &str = "workflow.task.semantic_query.map";
        pub static NAME_COMPILE: &str = "workflow.task.semantic_query.compile";
        pub static NAME_EXECUTE: &str = "workflow.task.semantic_query.execute";
        pub static NAME_EXECUTE_SQL: &str = "workflow.task.semantic_query.execute_sql";
        pub static TYPE: &str = "semantic_query";

        pub static INPUT_RENDER: &str = "workflow.task.semantic_query.render.input";
        pub static OUTPUT_RENDER: &str = "workflow.task.semantic_query.render.output";
        pub static INPUT_MAP: &str = "workflow.task.semantic_query.map.input";
        pub static OUTPUT_MAP: &str = "workflow.task.semantic_query.map.output";
        pub static INPUT_COMPILE: &str = "workflow.task.semantic_query.compile.input";
        pub static OUTPUT_COMPILE: &str = "workflow.task.semantic_query.compile.output";
        pub static INPUT_EXECUTE: &str = "workflow.task.semantic_query.execute.input";
        pub static OUTPUT_EXECUTE: &str = "workflow.task.semantic_query.execute.output";
        pub static INPUT_EXECUTE_SQL: &str = "workflow.task.semantic_query.execute_sql.input";
        pub static OUTPUT_EXECUTE_SQL: &str = "workflow.task.semantic_query.execute_sql.output";

        pub fn render_input(task: &crate::config::model::SemanticQueryTask) {
            event!(
                Level::DEBUG,
                name = INPUT_RENDER,
                is_visible = true,
                task = %serde_json::to_string(task).unwrap_or_default()
            );
        }

        pub fn render_output(rendered_task: &crate::config::model::SemanticQueryTask) {
            event!(
                Level::DEBUG,
                name = OUTPUT_RENDER,
                is_visible = true,
                status = "success",
                rendered_task = %serde_json::to_string(rendered_task).unwrap_or_default()
            );
        }

        pub fn map_input(task: &crate::config::model::SemanticQueryTask) {
            event!(
                Level::DEBUG,
                name = INPUT_MAP,
                is_visible = true,
                task = %serde_json::to_string(task).unwrap_or_default()
            );
        }

        pub fn map_output(topic: &str, dimensions_count: usize, measures_count: usize) {
            event!(
                Level::INFO,
                name = OUTPUT_MAP,
                is_visible = true,
                status = "success",
                topic = %topic,
                dimensions_count = dimensions_count,
                measures_count = measures_count
            );
        }

        pub fn compile_input(topic: &str, query: &crate::service::types::SemanticQueryParams) {
            event!(
                Level::DEBUG,
                name = INPUT_COMPILE,
                is_visible = true,
                topic = %topic,
                query = %serde_json::to_string(query).unwrap_or_default()
            );
        }

        pub fn compile_output(sql: &str) {
            event!(
                Level::DEBUG,
                name = OUTPUT_COMPILE,
                is_visible = true,
                status = "success",
                sql = %sql
            );
        }

        pub fn execute_input(topic: &str, dimensions: &[String], measures: &[String]) {
            event!(
                Level::INFO,
                name = INPUT_EXECUTE,
                is_visible = true,
                topic = %topic,
                dimensions = %serde_json::to_string(dimensions).unwrap_or_default(),
                measures = %serde_json::to_string(measures).unwrap_or_default()
            );
        }

        pub fn execute_output(output: &crate::exec_types::Output) {
            event!(
                Level::DEBUG,
                name = OUTPUT_EXECUTE,
                is_visible = true,
                status = "success",
                output = %serde_json::to_string(output).unwrap_or_default()
            );
        }

        pub fn execute_sql_input(database_ref: &str, sql: &str) {
            event!(
                Level::DEBUG,
                name = INPUT_EXECUTE_SQL,
                is_visible = true,
                database_ref = %database_ref,
                sql = %sql
            );
        }

        pub fn execute_sql_output(file_path: &str) {
            event!(
                Level::INFO,
                name = OUTPUT_EXECUTE_SQL,
                is_visible = true,
                status = "success",
                file_path = %file_path
            );
        }
    }

    pub mod execute_sql {
        use super::*;

        pub static NAME_MAP: &str = "workflow.task.execute_sql.map";
        pub static NAME_EXECUTE: &str = "workflow.task.execute_sql.execute";
        pub static TYPE: &str = "execute_sql";

        pub static INPUT_MAP: &str = "workflow.task.execute_sql.map.input";
        pub static OUTPUT_MAP: &str = "workflow.task.execute_sql.map.output";

        pub fn map_input(task: &crate::config::model::ExecuteSQLTask) {
            event!(
                Level::DEBUG,
                name = INPUT_MAP,
                is_visible = true,
                database = %task.database,
                sql = %serde_json::to_string(&task.sql).unwrap_or_default()
            );
        }

        pub fn map_output(sql: &str, database: &str) {
            event!(
                Level::DEBUG,
                name = OUTPUT_MAP,
                is_visible = true,
                status = "success",
                sql = %sql,
                database = %database
            );
        }
    }

    pub mod omni_query {
        use super::*;

        pub static NAME_MAP: &str = "workflow.task.omni_query.map";
        pub static NAME_EXECUTE: &str = "workflow.task.omni_query.execute";
        pub static NAME_EXECUTE_QUERY: &str = "workflow.task.omni_query.execute_query";
        pub static TYPE: &str = "omni_query";

        pub static INPUT_MAP: &str = "workflow.task.omni_query.map.input";
        pub static OUTPUT_MAP: &str = "workflow.task.omni_query.map.output";

        pub fn map_input(task: &crate::config::model::OmniQueryTask) {
            event!(
                Level::DEBUG,
                name = INPUT_MAP,
                is_visible = true,
                integration = %task.integration,
                topic = %task.topic,
                fields = %serde_json::to_string(&task.query.fields).unwrap_or_default()
            );
        }

        pub fn map_output(integration: &str, topic: &str, fields_count: usize) {
            event!(
                Level::INFO,
                name = OUTPUT_MAP,
                is_visible = true,
                status = "success",
                integration = %integration,
                topic = %topic,
                fields_count = fields_count
            );
        }
    }

    pub mod loop_task {
        use super::*;

        pub static NAME_MAP: &str = "workflow.task.loop.map";
        pub static NAME_ITEM_MAP: &str = "workflow.task.loop.item_map";
        pub static TYPE: &str = "loop";

        pub static INPUT_MAP: &str = "workflow.task.loop.map.input";
        pub static OUTPUT_MAP: &str = "workflow.task.loop.map.output";
        pub static INPUT_ITEM_MAP: &str = "workflow.task.loop.item_map.input";
        pub static OUTPUT_ITEM_MAP: &str = "workflow.task.loop.item_map.output";

        pub fn map_input(values_count: usize) {
            event!(
                Level::INFO,
                name = INPUT_MAP,
                is_visible = true,
                values_count = values_count
            );
        }

        pub fn map_output(iterations_count: usize) {
            event!(
                Level::INFO,
                name = OUTPUT_MAP,
                is_visible = true,
                status = "success",
                iterations_count = iterations_count
            );
        }

        pub fn item_map_input(iteration_index: usize, task_name: &str, value: &str) {
            event!(
                Level::INFO,
                name = INPUT_ITEM_MAP,
                is_visible = true,
                iteration_index = iteration_index,
                task_name = %task_name,
                value = %value
            );
        }

        pub fn item_map_output(iteration_index: usize, task_name: &str) {
            event!(
                Level::INFO,
                name = OUTPUT_ITEM_MAP,
                is_visible = true,
                status = "success",
                iteration_index = iteration_index,
                task_name = %task_name
            );
        }
    }

    pub mod formatter {
        use super::*;

        pub static NAME_EXECUTE: &str = "workflow.task.formatter.execute";
        pub static TYPE: &str = "formatter";
        pub static INPUT: &str = "workflow.task.formatter.input";
        pub static OUTPUT: &str = "workflow.task.formatter.output";

        pub fn input(template: &str) {
            event!(
                Level::INFO,
                name = INPUT,
                is_visible = true,
                template = %template
            );
        }

        pub fn output(result: &str) {
            event!(
                Level::INFO,
                name = OUTPUT,
                is_visible = true,
                status = "success",
                result = %result
            );
        }
    }

    pub mod sub_workflow {
        use super::*;

        pub static NAME_EXECUTE: &str = "workflow.task.sub_workflow.execute";
        pub static TYPE: &str = "sub_workflow";
        pub static INPUT: &str = "workflow.task.sub_workflow.input";
        pub static OUTPUT: &str = "workflow.task.sub_workflow.output";

        pub fn input(workflow_ref: &str) {
            event!(
                Level::INFO,
                name = INPUT,
                is_visible = true,
                workflow_ref = %workflow_ref
            );
        }

        pub fn output(workflow_ref: &str, output: &crate::exec_types::OutputContainer) {
            event!(
                Level::INFO,
                name = OUTPUT,
                is_visible = true,
                status = "success",
                workflow_ref = %workflow_ref,
                output = %serde_json::to_string(output).unwrap_or_default()
            );
        }
    }
}

/// Creates a span for agent task execution within an automation
pub fn task_agent_execute_span(agent_ref: &str, consistency_run: usize) -> Span {
    tracing::info_span!(
        "workflow.task.agent.execute",
        oxy.name = task::agent::NAME,
        oxy.span_type = task::agent::TYPE,
        oxy.agent.ref = %agent_ref,
        oxy.agent.consistency_run = consistency_run,
    )
}

pub fn task_formatter_execute_span() -> Span {
    tracing::info_span!(
        "workflow.task.formatter.execute",
        oxy.name = task::formatter::NAME_EXECUTE,
        oxy.span_type = task::formatter::TYPE,
    )
}

pub fn task_sub_workflow_execute_span(workflow_ref: &str) -> Span {
    tracing::info_span!(
        "workflow.task.sub_workflow.execute",
        oxy.name = task::sub_workflow::NAME_EXECUTE,
        oxy.span_type = task::sub_workflow::TYPE,
        oxy.workflow.ref = %workflow_ref,
    )
}
