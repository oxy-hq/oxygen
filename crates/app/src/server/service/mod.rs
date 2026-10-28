// Re-export service modules from core
pub use oxy::service::{message, secret_manager, sync, task_manager};
pub use oxy::types;

// These modules depend on extracted crates and must stay in CLI
pub mod api_key;
pub mod app;
pub mod eval;
pub mod formatters; // CLI-specific formatters (different from oxy::service::formatters)
pub mod project;
pub mod test;
pub mod test_runs;
