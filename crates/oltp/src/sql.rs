//! Running DDL inside a tenant's Postgres.
//!
//! Split from the provider on purpose. The provider control plane (create
//! project, create role) is a REST API; schema and grant management is SQL
//! against the tenant database itself. Keeping them apart means the
//! security-critical DDL in [`crate::schema`] is unit-testable with no live
//! Postgres and no live provider.

use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum SqlError {
    #[error("could not connect to tenant database: {0}")]
    Connect(String),
    #[error("statement failed ({statement}): {source_message}")]
    Statement {
        statement: String,
        source_message: String,
    },
}

/// Executes a batch of DDL statements against a tenant database.
///
/// Implementations **must run the batch in order and stop at the first
/// failure**. [`crate::schema`] emits statements whose ordering is load-bearing
/// (a schema must exist before it is granted), so an executor that reorders or
/// continues past an error would silently produce a half-granted schema.
#[async_trait]
pub trait TenantSqlExecutor: Send + Sync {
    async fn execute_batch(&self, dsn: &str, statements: &[String]) -> Result<(), SqlError>;

    /// Run one `SELECT` and return every row as text (`NULL` is `None`), for the
    /// few places that copy a small bookkeeping table from one database to
    /// another. Callers cast non-text columns in the query.
    ///
    /// An executor that cannot read refuses, rather than answering "no rows":
    /// a copy that silently copied nothing would look like an empty ledger.
    async fn query_text_rows(
        &self,
        _dsn: &str,
        statement: &str,
    ) -> Result<Vec<Vec<Option<String>>>, SqlError> {
        Err(SqlError::Statement {
            statement: statement.to_string(),
            source_message: "this executor cannot read rows".to_string(),
        })
    }
}

/// Runs statements against a real Postgres.
///
/// Each batch opens its own connection: DDL here is infrequent (provision,
/// publish, platform reconcile), so a pool would be complexity without benefit,
/// and a short-lived connection avoids holding a scale-to-zero database awake.
pub struct PgSqlExecutor;

#[async_trait]
impl TenantSqlExecutor for PgSqlExecutor {
    async fn execute_batch(&self, dsn: &str, statements: &[String]) -> Result<(), SqlError> {
        let client = crate::connect::connect(dsn, "tenant DDL")
            .await
            .map_err(|e| SqlError::Connect(crate::connect::pg_detail(&e)))?;

        // Sequential and fail-fast: [`crate::schema`] emits statements whose
        // ordering is load-bearing, so continuing past an error would leave a
        // half-granted schema.
        for stmt in statements {
            client
                .batch_execute(stmt)
                .await
                .map_err(|e| SqlError::Statement {
                    statement: stmt.clone(),
                    // Not `e.to_string()`: that is the literal text "db error".
                    // A `RAISE ... USING HINT` — which is how the confinement
                    // check reports — lives entirely in the DbError, so the
                    // caller would see a failure with no reason attached.
                    source_message: crate::connect::pg_detail(&e),
                })?;
        }
        Ok(())
    }

    async fn query_text_rows(
        &self,
        dsn: &str,
        statement: &str,
    ) -> Result<Vec<Vec<Option<String>>>, SqlError> {
        let client = crate::connect::connect(dsn, "tenant read")
            .await
            .map_err(|e| SqlError::Connect(crate::connect::pg_detail(&e)))?;
        let messages = client
            .simple_query(statement)
            .await
            .map_err(|e| SqlError::Statement {
                statement: statement.to_string(),
                source_message: crate::connect::pg_detail(&e),
            })?;
        Ok(messages
            .into_iter()
            .filter_map(|m| match m {
                tokio_postgres::SimpleQueryMessage::Row(row) => Some(
                    (0..row.len())
                        .map(|i| row.get(i).map(str::to_string))
                        .collect(),
                ),
                _ => None,
            })
            .collect())
    }
}

/// Records statements instead of running them, so a test can assert the
/// `(dsn, statements)` a caller emits without a cluster.
///
/// `cfg(test)`: its only user is this crate's own unit tests (the dry-run
/// provisioning its earlier doc mentioned never shipped). Gate keeps it out of
/// the published API surface; widen to a `test-support` feature if another
/// crate's tests ever need it.
#[cfg(test)]
#[derive(Default)]
pub struct RecordingSqlExecutor {
    batches: std::sync::Mutex<Vec<(String, Vec<String>)>>,
    /// Statement substring that should fail when executed, simulating a
    /// mid-batch DDL error.
    fail_on: std::sync::Mutex<Option<String>>,
}

#[cfg(test)]
impl RecordingSqlExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make any statement containing `needle` fail, so callers can exercise
    /// partial-application handling.
    pub fn fail_on(&self, needle: impl Into<String>) {
        *self.fail_on.lock().expect("recorder lock") = Some(needle.into());
    }

    /// Every batch, as `(dsn, statements)`, in execution order.
    pub fn batches(&self) -> Vec<(String, Vec<String>)> {
        self.batches.lock().expect("recorder lock").clone()
    }

    /// All statements across all batches, flattened.
    pub fn statements(&self) -> Vec<String> {
        self.batches
            .lock()
            .expect("recorder lock")
            .iter()
            .flat_map(|(_, s)| s.clone())
            .collect()
    }

    /// Whether any executed statement contains `needle`.
    pub fn ran(&self, needle: &str) -> bool {
        self.statements().iter().any(|s| s.contains(needle))
    }
}

#[cfg(test)]
#[async_trait]
impl TenantSqlExecutor for RecordingSqlExecutor {
    async fn execute_batch(&self, dsn: &str, statements: &[String]) -> Result<(), SqlError> {
        let needle = self.fail_on.lock().expect("recorder lock").clone();

        // Record only what actually ran, so a caller inspecting `statements()`
        // after a failure sees the true partial state.
        let mut applied = Vec::new();
        for stmt in statements {
            if let Some(n) = &needle
                && stmt.contains(n.as_str())
            {
                self.batches
                    .lock()
                    .expect("recorder lock")
                    .push((dsn.to_string(), applied));
                return Err(SqlError::Statement {
                    statement: stmt.clone(),
                    source_message: "injected failure".to_string(),
                });
            }
            applied.push(stmt.clone());
        }
        self.batches
            .lock()
            .expect("recorder lock")
            .push((dsn.to_string(), applied));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn records_batches_in_order() {
        let ex = RecordingSqlExecutor::new();
        ex.execute_batch("dsn-a", &["ONE".into()]).await.unwrap();
        ex.execute_batch("dsn-b", &["TWO".into(), "THREE".into()])
            .await
            .unwrap();

        let batches = ex.batches();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].0, "dsn-a");
        assert_eq!(ex.statements(), vec!["ONE", "TWO", "THREE"]);
    }

    #[tokio::test]
    async fn stops_at_the_first_failure_and_records_only_what_ran() {
        let ex = RecordingSqlExecutor::new();
        ex.fail_on("BOOM");
        let err = ex
            .execute_batch("dsn", &["OK".into(), "BOOM".into(), "NEVER".into()])
            .await
            .unwrap_err();

        assert!(matches!(err, SqlError::Statement { .. }));
        assert_eq!(
            ex.statements(),
            vec!["OK"],
            "statements after the failure must not be recorded as applied"
        );
    }

    #[tokio::test]
    async fn ran_matches_on_substring() {
        let ex = RecordingSqlExecutor::new();
        ex.execute_batch("dsn", &[r#"CREATE SCHEMA "app_x""#.into()])
            .await
            .unwrap();
        assert!(ex.ran("CREATE SCHEMA"));
        assert!(!ex.ran("DROP SCHEMA"));
    }
}
