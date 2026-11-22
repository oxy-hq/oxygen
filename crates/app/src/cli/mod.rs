//! CLI command-line interface

pub mod commands;
pub mod render;

/// `oxy dev seed | compile` (`dev-tools` feature), composed into `oxy dev` by
/// `oxy-server` with `oxy-api-eval`'s `test`.
#[cfg(feature = "dev-tools")]
pub use commands::dev::{DevCommand, renamed_hint, run as run_dev};
pub use commands::{SubCommand, prepare, run};
