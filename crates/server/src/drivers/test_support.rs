//! Shim: the shared DB harness for in-source (`#[cfg(test)]`) tests lives in
//! `oxy_test_utils::db` (`crates/test-utils/src/db.rs`). This names the runtime
//! migrator, which `oxy-test-utils` cannot depend on, for the drivers' tests.
//! It was `oxy-app`'s `server::test_support` until the drivers, its last
//! callers there, moved up into `oxy-server` (decompose BL1, 2026-11-23).

pub(crate) use oxy_test_utils::db::SKIP_MSG;

/// [`oxy_test_utils::db::test_db`] with the runtime migrator.
pub(crate) async fn test_db() -> Option<sea_orm::DatabaseConnection> {
    oxy_test_utils::db::test_db::<agentic_runtime::migration::RuntimeMigrator>().await
}
