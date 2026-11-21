//! The database-level read-only backstop behind the react `query` tool.
//!
//! Every case calls the connector directly: no analytics gate stands in front,
//! so a write that is refused here is refused by the connector and the engine
//! underneath it. Postgres runs in a real server (testcontainers, or
//! `OXY_TEST_POSTGRES_URL`); DuckDB is the real bundled engine.
//!
//!   cargo nextest run -p agentic-connector --features postgres,duckdb \
//!     --test integration -E 'test(read_only_backstop)'

use std::sync::{Arc, Mutex};

use agentic_connector::{ConnectorError, DatabaseConnector, ExecutionResult, SqlDialect};
use async_trait::async_trait;

/// A connector that implements only what the trait requires, and records
/// every SQL it is handed.
#[derive(Default)]
struct Bare {
    seen: Mutex<Vec<String>>,
}

#[async_trait]
impl DatabaseConnector for Bare {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::BigQuery
    }

    async fn execute_query(&self, sql: &str, _: u64) -> Result<ExecutionResult, ConnectorError> {
        self.seen.lock().unwrap().push(sql.to_string());
        Ok(ExecutionResult::empty())
    }
}

#[tokio::test]
async fn a_connector_without_read_only_refuses() {
    let bare = Arc::new(Bare::default());
    assert!(!bare.supports_read_only());
    let err = bare
        .execute_query_read_only("SELECT 1", 10)
        .await
        .expect_err("the default must refuse");
    assert_eq!(
        err.to_string(),
        "read-only execution is not supported by BigQuery"
    );
    assert!(
        bare.seen.lock().unwrap().is_empty(),
        "the default must never fall through to execute_query"
    );
}

#[cfg(feature = "postgres")]
#[path = "read_only_backstop_pg.rs"]
mod postgres;

/// Built without `postgres`, the Postgres backstop tests above compile out.
/// Where `OXY_TEST_REQUIRE_POSTGRES=1` asks for them (CI), that is a
/// failure, not a silent pass: run with `--features postgres,duckdb`.
#[cfg(not(feature = "postgres"))]
#[test]
fn postgres_backstop_tests_are_compiled_in_when_required() {
    assert_ne!(
        std::env::var("OXY_TEST_REQUIRE_POSTGRES").as_deref(),
        Ok("1"),
        "OXY_TEST_REQUIRE_POSTGRES=1 but agentic-connector was built without \
         its `postgres` feature, so the read-only backstop tests did not run"
    );
}

#[cfg(feature = "postgres")]
#[path = "read_only_backstop_pg_roles.rs"]
mod postgres_roles;

#[cfg(feature = "duckdb")]
#[path = "read_only_backstop_duckdb.rs"]
mod duckdb;
