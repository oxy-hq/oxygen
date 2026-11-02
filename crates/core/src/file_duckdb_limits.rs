//! What a file DuckDB may still do on the cloud roles.
//!
//! A `type: duckdb` database with `dataset:` (a directory of CSV/parquet) or
//! `path:` (a `.duckdb` file) keeps its data in the workspace's git checkout.
//! It is deprecated: on the `ide`, `serve` and `worker` roles the compile
//! refuses it (`compile_config_gate.rs`) and points at Airhouse
//! (`type: airhouse_managed`). Revisions compiled before that refusal still
//! read the S3 copy the compiler made of their data, until those workspaces
//! have moved; on the cloud roles such a database is opened read-only, and
//! anything that would write into it (an Airform run) is refused by name.
//!
//! The single-process role (`OXY_ROLE=all`: the CLI, `oxy seed`, a laptop's
//! `oxy start`) is exempt and behaves as it always has; its compile only warns.
//! That is the default here, so a process that never declares a role (a test,
//! a library caller) is unaffected.
//!
//! Lives in `oxy` because the connectors do, and core cannot depend on the app
//! crate that reads `OXY_ROLE`. The app pushes the answer down at boot with
//! [`set_limits_apply`], next to
//! [`crate::workspace_fs_probe::set_process_owns_workspace_files`].

use std::sync::atomic::{AtomicBool, Ordering};

use oxy_shared::errors::OxyError;

/// `false` until the app declares a cloud role: an un-configured process is a
/// single-process instance.
static LIMITS_APPLY: AtomicBool = AtomicBool::new(false);

/// Declare whether this process runs a cloud role. Called once at startup from
/// the app's role initialisation: `ide`, `serve` and `worker` pass `true`,
/// `all` passes `false`.
pub fn set_limits_apply(apply: bool) {
    LIMITS_APPLY.store(apply, Ordering::Relaxed);
}

/// Whether this process runs a cloud role, where a file DuckDB is read-only.
pub fn limits_apply() -> bool {
    LIMITS_APPLY.load(Ordering::Relaxed)
}

/// The error for something that would write into a file DuckDB on a cloud
/// role (an Airform target, a write through the connector).
pub fn write_refused_error(database: &str, what: &str) -> OxyError {
    OxyError::ConfigurationError(format!(
        "database `{database}` is a DuckDB file database, which is read-only in a cloud \
         deployment, so {what} cannot write into it. Write to the workspace's Airhouse \
         (`type: airhouse_managed`) or to S3 instead"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_undeclared_process_is_single_process_and_unlimited() {
        // Nothing in this crate's tests declares a role, which is the
        // single-process default every library caller gets.
        assert!(!limits_apply());
    }

    #[test]
    fn the_write_refusal_names_the_database() {
        let write = write_refused_error("ledger", "an Airform run").to_string();
        assert!(
            write.contains("`ledger`") && write.contains("Airhouse"),
            "{write}"
        );
    }
}
