//! Shim: the shared DB harness for in-source (`#[cfg(test)]`) tests lives in
//! `oxy_test_utils::db` (`crates/test-utils/src/db.rs`). This keeps the
//! `crate::server::test_support::{SKIP_MSG, test_db}` call sites unchanged and
//! names the runtime migrator, which `oxy-test-utils` cannot depend on.

pub(crate) use oxy_test_utils::db::SKIP_MSG;

/// [`oxy_test_utils::db::test_db`] with the runtime migrator.
pub(crate) async fn test_db() -> Option<sea_orm::DatabaseConnection> {
    oxy_test_utils::db::test_db::<agentic_runtime::migration::RuntimeMigrator>().await
}
