//! Shared infrastructure and types for Oxy
//!
//! This crate provides common functionality used across all Oxy slices:
//! - Database operations and connection management  
//! - Storage abstractions (local, S3, etc.)
//! - Error types and handling
//! - Common domain types
//!
//! Note: Some modules (checkpoint) have been temporarily disabled due to
//! circular dependencies with core. They will be refactored in a future update.

pub mod domain;
pub mod duckdb_s3;
pub mod errors;
pub mod fleet_role;
pub mod infrastructure;
pub mod key_validation;
pub mod log_noise;
pub mod openai_config;
pub mod semantic_error;
pub mod sql_literal;
pub mod state_dir;
pub mod utils;

// Re-export commonly used items
pub use errors::OxyError;
pub use key_validation::{KeyValidationError, KeyValidationErrorKind};
pub use openai_config::{AzureModel, ConfigType, CustomOpenAIConfig, HeaderValue};
