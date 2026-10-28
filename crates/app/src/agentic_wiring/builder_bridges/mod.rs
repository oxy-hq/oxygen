//! Oxy-backed impls of the `agentic-builder` port traits.
//!
//! These bridges are the adapters between the builder domain (in
//! `agentic-builder`) and Oxy's config / validation stack. They live in the
//! host application so the agentic crates stay `oxy::*`-free.

mod database_provider;
mod project_validator;
mod schema_provider;
mod secrets_provider;
mod semantic_compiler;

pub use database_provider::OxyBuilderDatabaseProvider;
pub use project_validator::OxyBuilderProjectValidator;
pub use schema_provider::OxyBuilderSchemaProvider;
pub use secrets_provider::OxyBuilderSecretsProvider;
pub use semantic_compiler::OxyBuilderSemanticCompiler;
