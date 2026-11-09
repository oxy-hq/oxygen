//! Test utilities, fixtures, and mocks for Oxy crates.
//!
//! This crate provides shared test infrastructure including:
//! - Mock implementations for external services (LLM APIs, Docker, etc.)
//! - Fixture management utilities
//! - Common test helpers

use std::path::PathBuf;

pub mod containers;
pub mod fixtures;
pub mod migration;
pub mod mocks;

// Re-export commonly used test dependencies for convenience
pub use serial_test::serial;
pub use tempfile::{Builder as TempFileBuilder, TempDir};
pub use wiremock::matchers::{body_json, method, path};
pub use wiremock::{Mock, MockServer, ResponseTemplate};

/// Returns the path to the oxy binary for integration tests.
///
/// Resolution order:
/// 1. `OXY_BIN` env var (set by CI or user)
/// 2. `target/llvm-cov-target/debug/oxy` (coverage builds)
/// 3. `target/debug/oxy` (regular debug)
pub fn get_oxy_binary() -> PathBuf {
    if let Ok(bin) = std::env::var("OXY_BIN") {
        return PathBuf::from(bin);
    }

    let workspace_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    for subdir in ["target/llvm-cov-target/debug/oxy", "target/debug/oxy"] {
        let p = workspace_dir.join(subdir);
        if p.exists() {
            return p;
        }
    }

    workspace_dir.join("target/debug/oxy")
}

/// Returns the path to the `tests/fixtures/oxy_example/` integration-test
/// workspace, copying `examples/bigquery-sample.key` into it first if the
/// legacy `examples/` path holds a key.
///
/// CI writes the BigQuery credential to `examples/bigquery-sample.key`
/// (per `.github/workflows/{ci,coverage}.yaml`). Oxy's `key_path`
/// resolver refuses paths that escape the project directory, so we
/// stage the key into the fixture before every CLI invocation. The
/// copy is idempotent — re-runs and incremental dev workflows are fine.
pub fn oxy_example_fixture_dir() -> PathBuf {
    let workspace_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let fixture = workspace_dir.join("tests/fixtures/oxy_example");
    let legacy_key = workspace_dir.join("examples/bigquery-sample.key");
    let fixture_key = fixture.join("bigquery-sample.key");
    if legacy_key.exists() {
        let _ = std::fs::copy(&legacy_key, &fixture_key);
    }
    fixture
}

/// Convenience macro for skipping tests when an environment variable is not set.
///
/// # Example
/// ```ignore
/// skip_if_env_missing!("OPENAI_API_KEY");
/// ```
#[macro_export]
macro_rules! skip_if_env_missing {
    ($var:expr) => {
        if std::env::var($var).is_err() {
            eprintln!("Skipping test: {} not set", $var);
            return;
        }
    };
}

/// Convenience macro for skipping async tests when an environment variable is not set.
#[macro_export]
macro_rules! skip_if_env_missing_async {
    ($var:expr) => {
        if std::env::var($var).is_err() {
            eprintln!("Skipping test: {} not set", $var);
            return Ok(());
        }
    };
}
