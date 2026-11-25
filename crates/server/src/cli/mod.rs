//! The `oxy` CLI's one-shot commands (`compile`, `migrate`, `migrate-agents`
//! and the one-release stubs), registered with the `oxy-cli-shim` parser as
//! one owner after the long-lived `start`, `serve` and `worker`
//! (`crate::cli_owners`). Up from `oxy-app`'s `cli` in decompose CLI
//! (2026-11-25); the internal `crate::cli::commands::…` paths kept their shape.

pub mod commands;

/// `oxy dev compile` (`dev-tools` feature), composed into `oxy dev`
/// (`crate::dev`) with the `seed` there and `oxy-api-eval`'s `test`.
#[cfg(feature = "dev-tools")]
pub use commands::dev::{DevCommand, renamed_hint, run as run_dev};
/// The dev compile's entry point, which `oxy dev seed` (`crate::dev::seed`)
/// runs on every seeded workspace.
#[cfg(feature = "dev-tools")]
pub use commands::{CompileArgs, run_compile};
pub use commands::{SubCommand, prepare, run};
