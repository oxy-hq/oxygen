//! The Airhouse connector a react run queries through: a pooled connection
//! whose credential Airhouse confirmed may only read
//! (`AirhouseTokenBroker::mint_read_only`, which refuses a credential without
//! the `read_only` echo).
//!
//! Airhouse enforces the credential on every statement (#256), so the read-only
//! path is the server's, not a check here: model-written SQL runs on it as
//! sent. Statements and transactions are refused before anything is sent, a
//! second fence for a caller that would write. Every other method forwards.
//! Every method of `DatabaseConnector` is stated (`every_trait_method_is_stated`),
//! so a default cannot quietly answer for the inner connector.

use std::sync::Arc;

use agentic_connector::{
    AsArrowConnector, ConnectorError, DatabaseConnector, ExecutionResult, SchemaInfo, SqlDialect,
    SqlTransaction, StringLiteral,
};
use agentic_core::result::{TypedRowStream, TypedValue};
use async_trait::async_trait;

pub(crate) struct ReadOnlyAirhouse {
    inner: Arc<dyn DatabaseConnector>,
}

impl ReadOnlyAirhouse {
    /// Wraps a connection opened with a credential the broker confirmed
    /// read-only. Only `connector::build_read_only_airhouse` builds one.
    pub(crate) fn confirmed(inner: Arc<dyn DatabaseConnector>) -> Self {
        Self { inner }
    }
}

fn refused(what: &str) -> ConnectorError {
    ConnectorError::Other(format!(
        "refused on a read-only Airhouse connection: {what}. Nothing was sent."
    ))
}

#[async_trait]
impl DatabaseConnector for ReadOnlyAirhouse {
    fn dialect(&self) -> SqlDialect {
        self.inner.dialect()
    }

    fn string_literal(&self) -> StringLiteral {
        self.inner.string_literal()
    }

    async fn execute_query(
        &self,
        sql: &str,
        sample_limit: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        self.inner.execute_query(sql, sample_limit).await
    }

    /// The credential is the read-only guarantee.
    fn supports_read_only(&self) -> bool {
        true
    }

    async fn execute_query_read_only(
        &self,
        sql: &str,
        sample_limit: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        self.inner.execute_query(sql, sample_limit).await
    }

    /// Ready by construction: the broker refused any credential Airhouse did
    /// not confirm read-only, so this connection exists only when it is.
    async fn read_only_ready(&self) -> Result<(), ConnectorError> {
        Ok(())
    }

    async fn execute_query_full(&self, sql: &str) -> Result<TypedRowStream, ConnectorError> {
        self.inner.execute_query_full(sql).await
    }

    async fn execute_query_full_untyped(
        &self,
        sql: &str,
    ) -> Result<TypedRowStream, ConnectorError> {
        self.inner.execute_query_full_untyped(sql).await
    }

    async fn execute_query_first_row(
        &self,
        sql: &str,
    ) -> Result<Option<Vec<TypedValue>>, ConnectorError> {
        self.inner.execute_query_first_row(sql).await
    }

    async fn begin_transaction(&self) -> Result<Box<dyn SqlTransaction>, ConnectorError> {
        Err(refused("a transaction"))
    }

    fn as_arrow(&self) -> Option<&dyn AsArrowConnector> {
        None
    }

    async fn execute_statement(&self, _sql: &str) -> Result<(), ConnectorError> {
        Err(refused("a statement"))
    }

    async fn execute_statement_tagged(&self, _sql: &str, _tag: &str) -> Result<(), ConnectorError> {
        Err(refused("a statement"))
    }

    async fn prepare_schema(&self) -> Result<(), ConnectorError> {
        self.inner.prepare_schema().await
    }

    fn introspect_schema(&self) -> Result<SchemaInfo, ConnectorError> {
        self.inner.introspect_schema()
    }
}

#[cfg(test)]
mod tests {
    /// A source scan: every method of `DatabaseConnector` is stated here.
    #[test]
    fn every_trait_method_is_stated() {
        let trait_src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../agentic/connector/src/connector.rs"
        ))
        .expect("agentic-connector source is readable from the workspace");
        let names = |src: &str, marker: &str| -> Vec<String> {
            let start = src.find(marker).expect(marker);
            let block = &src[start..];
            let end = block.find("\n}\n").expect("end of block");
            (block[..end].lines())
                .map(str::trim)
                .filter(|l| !l.starts_with("//"))
                .filter_map(|l| {
                    l.strip_prefix("async fn ")
                        .or_else(|| l.strip_prefix("fn "))
                })
                .filter_map(|l| l.split('(').next())
                .map(str::to_string)
                .collect()
        };
        let trait_fns = names(&trait_src, "pub trait DatabaseConnector");
        let impl_fns = names(
            include_str!("read_only_airhouse.rs"),
            "impl DatabaseConnector for ReadOnlyAirhouse",
        );
        assert!(trait_fns.len() >= 9, "too few trait methods: {trait_fns:?}");
        for name in &trait_fns {
            assert!(
                impl_fns.contains(name),
                "ReadOnlyAirhouse does not state `{name}`"
            );
        }
    }
}
