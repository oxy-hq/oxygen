//! Project and workspace management for Oxy

// SeaORM 2.0's query types nest deeper generically than 1.1's, and rustc's
// default query depth is not enough to lay out the async fns that build
// queries here. Raising the limit is the fix rustc itself suggests.
#![recursion_limit = "256"]

pub mod config_builder;
pub mod data_repo_service;
pub mod database_config;
pub mod database_operations;
pub mod model_config;
pub mod models;
pub mod workspace_creator;

pub use config_builder::ConfigBuilder;
pub use database_config::DatabaseConfigBuilder;
pub use database_operations::{DatabaseOperations, ValidationUtils};
pub use model_config::ModelConfigBuilder;
pub use workspace_creator::{
    DemoCopyResult, copy_airhouse_demo_files_to, copy_demo_files_to, copy_demo_files_to_with_skip,
    demo_dataset_files, write_minimal_config_yml,
};
