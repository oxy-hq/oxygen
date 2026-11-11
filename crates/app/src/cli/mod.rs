//! CLI command-line interface

pub mod commands;
pub mod render;

pub use commands::{SubCommand, prepare, run};
