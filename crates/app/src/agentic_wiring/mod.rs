//! Oxy-specific wiring for the agentic stack.
//!
//! This module is the **only** place in the codebase that bridges between
//! the `oxy::*` platform types and the agentic-pipeline port traits. When
//! the platform refactor lands, most of the churn happens in here.
//!
//! # Contents
//!
//! - [`project_ctx`] — [`OxyProjectContext`] implements
//!   [`agentic_pipeline::platform::ProjectContext`] +
//!   [`agentic_automation::WorkspaceContext`].
//! - [`builder_bridges`] — Oxy impls of the four `agentic-builder` port
//!   traits (database, schema, semantic, validator).
//! - [`thread_owner`] — platform threads-table adapter for
//!   [`agentic_pipeline::platform::ThreadOwnerLookup`].

pub mod airhouse_pool;
pub mod app_airhouse;
pub mod builder_bridges;
pub mod compile_dispatcher;
pub mod metric_sink;
pub mod metric_tree_runner;
pub mod project_ctx;
pub mod thread_owner;

use std::sync::Arc;

use agentic_pipeline::platform::BuilderBridges;

pub use metric_sink::OxyAnalyticsMetricSink;
pub use metric_tree_runner::{OxyMetricTreeRunner, build_query_executor};
pub use project_ctx::OxyProjectContext;
pub use thread_owner::OxyThreadOwnerLookup;

/// Assemble the four builder-domain port impls from a shared project context.
///
/// The returned [`BuilderBridges`] bundle is cheap to clone and can be
/// reused across many pipeline builds for the same workspace.
pub fn build_builder_bridges(project_ctx: Arc<OxyProjectContext>) -> BuilderBridges {
    let secrets_manager = project_ctx.workspace_manager().secrets_manager.clone();
    BuilderBridges {
        db_provider: Arc::new(builder_bridges::OxyBuilderDatabaseProvider::new(
            project_ctx.clone(),
        )),
        project_validator: Arc::new(builder_bridges::OxyBuilderProjectValidator::new(
            project_ctx.workspace_manager().clone(),
        )),
        schema_provider: Arc::new(builder_bridges::OxyBuilderSchemaProvider::new()),
        semantic_compiler: Arc::new(builder_bridges::OxyBuilderSemanticCompiler::new(
            project_ctx,
        )),
        secrets_provider: Some(Arc::new(builder_bridges::OxyBuilderSecretsProvider::new(
            secrets_manager,
        ))),
    }
}
