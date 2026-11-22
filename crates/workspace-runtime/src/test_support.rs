//! The shared DB harness for this crate's in-source (`#[cfg(test)]`) tests:
//! [`oxy_test_utils::db::test_db`] with the runtime migrator, as `oxy-app`'s
//! `server::test_support` names it. The preview platform's tests came down
//! with it from `oxy-app` (W5, 2026-11-22) and kept their call sites.

pub(crate) use oxy_test_utils::db::SKIP_MSG;

/// [`oxy_test_utils::db::test_db`] with the runtime migrator.
pub(crate) async fn test_db() -> Option<sea_orm::DatabaseConnection> {
    oxy_test_utils::db::test_db::<agentic_runtime::migration::RuntimeMigrator>().await
}
