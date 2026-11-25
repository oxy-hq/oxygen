//! Unit tests of the sink's pure parts.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::{
    AirhouseSink, Bounded, ChangeSink, SinkError, broker_present, connect_failed, transaction,
};

fn error(message: &str) -> SinkError {
    SinkError(message.to_owned())
}

#[test]
fn a_write_is_one_transaction() {
    let statements = vec!["INSERT 1".to_owned(), "DELETE 2".to_owned()];
    assert_eq!(
        transaction(&statements),
        vec!["BEGIN", "INSERT 1", "DELETE 2", "COMMIT"]
    );
}

#[test]
fn an_empty_write_is_an_empty_transaction() {
    assert_eq!(transaction(&[]), vec!["BEGIN", "COMMIT"]);
}

#[test]
fn a_sink_error_displays_its_message() {
    assert_eq!(SinkError("boom".to_owned()).to_string(), "boom");
}

/// A statement's own error that its table or schema does not exist: Postgres's
/// SQLSTATEs, and DuckLake's catalog errors, which Airhouse reports as `XX000`.
#[test]
fn a_statement_on_a_missing_table_or_schema_is_missing() {
    for missing in [
        "[42P01] relation \"oltp_shop._progress\" does not exist",
        "[3F000] schema \"oltp_shop\" does not exist",
        "[XX000] Catalog Error: Table with name _progress does not exist!",
        "[XX000] Catalog Error: Schema with name oltp_shop does not exist!",
    ] {
        assert!(AirhouseSink.is_missing(&error(missing)), "{missing}");
    }
}

/// A failure to mint or connect never reads as missing, whatever the server
/// said: a missing role, database or catalog is not "nothing shipped".
#[test]
fn a_connect_error_is_never_missing() {
    for said in [
        "connect failed: [28000] role \"x\" does not exist",
        "connect failed: [3D000] database \"x\" does not exist",
        "connect failed: [42P01] relation \"x\" does not exist",
        "connect failed: [XX000] Catalog Error: Table with name x does not exist!",
        "broker mint failed: workspace does not exist",
    ] {
        let failed = connect_failed(said);
        assert!(failed.0.starts_with("connect: "), "{failed}");
        assert!(!AirhouseSink.is_missing(&failed), "{failed}");
    }
}

/// Other statement errors stay errors, `does not exist` in them or not.
#[test]
fn another_statement_error_is_not_missing() {
    for other in [
        "[XX000] Catalog Error: Table Function with name read_parquet does not exist!",
        "[XX000] Binder Error: Referenced column \"batch\" not found in FROM clause",
        "[42501] permission denied for table _progress",
        "connection closed",
    ] {
        assert!(!AirhouseSink.is_missing(&error(other)), "{other}");
    }
}

/// A sink with no connect step, such as an in-memory DuckDB.
struct Local;

#[async_trait]
impl ChangeSink for Local {
    async fn has_airhouse(&self, _: &DatabaseConnection, _: Uuid) -> Result<bool, SinkError> {
        unreachable!("not called")
    }

    async fn query(&self, _: Uuid, _: &str) -> Result<Vec<Vec<Option<String>>>, SinkError> {
        unreachable!("not called")
    }

    async fn admin(&self, _: Uuid, _: &[String]) -> Result<(), SinkError> {
        unreachable!("not called")
    }

    async fn write(&self, _: Uuid, _: &[String]) -> Result<(), SinkError> {
        unreachable!("not called")
    }
}

/// A worker with no Airhouse token broker cannot tell whether a workspace has
/// an Airhouse: that fails the writer, never holds it, and is never read as
/// a missing table.
#[test]
fn no_broker_is_a_failure_not_an_answer() {
    assert_eq!(broker_present(true), Ok(()));
    let failed = broker_present(false).unwrap_err();
    assert!(failed.0.contains("no Airhouse token broker"), "{failed}");
    assert!(!AirhouseSink.is_missing(&failed));
}

#[test]
fn by_default_does_not_exist_is_missing() {
    let duckdb = "Catalog Error: Table with name _progress does not exist!";
    assert!(Local.is_missing(&error(duckdb)));
    assert!(!Local.is_missing(&error("Binder Error: no such column")));
}

/// A sink whose reads and writes never answer.
struct Hangs;

#[async_trait]
impl ChangeSink for Hangs {
    async fn has_airhouse(&self, _: &DatabaseConnection, _: Uuid) -> Result<bool, SinkError> {
        std::future::pending().await
    }

    async fn query(&self, _: Uuid, _: &str) -> Result<Vec<Vec<Option<String>>>, SinkError> {
        std::future::pending().await
    }

    async fn admin(&self, _: Uuid, _: &[String]) -> Result<(), SinkError> {
        Ok(())
    }

    async fn write(&self, _: Uuid, _: &[String]) -> Result<(), SinkError> {
        std::future::pending().await
    }
}

/// A call that outlasts its bound fails, as a failure and never as a
/// missing table; one that answers in time is untouched.
#[tokio::test]
async fn a_call_past_its_bound_fails() {
    let bounded = Bounded::new(Arc::new(Hangs), Duration::from_millis(10));
    let read = bounded.query(Uuid::nil(), "SELECT 1").await.unwrap_err();
    assert_eq!(read.0, "timed out after 0.01 s");
    assert!(!bounded.is_missing(&read));
    let write = bounded.write(Uuid::nil(), &[]).await.unwrap_err();
    assert_eq!(write, read);
    assert_eq!(bounded.admin(Uuid::nil(), &[]).await, Ok(()));
}
