//! CLI command-line interface

pub mod commands;
pub mod render;

/// `oxy dev compile` (`dev-tools` feature), composed into `oxy dev` by
/// `oxy-server` with its own `seed` and `oxy-api-eval`'s `test`.
#[cfg(feature = "dev-tools")]
pub use commands::dev::{DevCommand, renamed_hint, run as run_dev};
/// The dev compile's entry point, which `oxy dev seed` (`oxy-server`) runs on
/// every seeded workspace.
#[cfg(feature = "dev-tools")]
pub use commands::{CompileArgs, run_compile};
pub use commands::{SubCommand, prepare, run};
