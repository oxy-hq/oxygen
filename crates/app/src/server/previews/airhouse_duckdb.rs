//! **Test stand-in; never wired into the server.** In-process DuckDB standing
//! in for the workspace's Airhouse behind every preview port at once
//! (`agentic_wiring::preview_airhouse::PreviewAirhousePorts`): the batches a
//! preview run sends, the schemas its registry creates
//! ([`super::ddl_duckdb::DuckDbPreviewDdl`], the same fixed statements
//! production sends), and the two "can this Airhouse confine a Writer?"
//! answers, which a test sets.
//!
//! It is compiled into the library (the platform integration tests drive whole
//! runs over it, and this crate has no test-only feature), hidden from the
//! docs, and kept out of the server by
//! `tests::the_duckdb_stand_in_is_never_wired_into_the_server`, a source scan.
//!
//! The rewrite, the verifier, the registry, copy-on-write and the shadow map
//! all run for real against it; only the credential fence (Airhouse's own
//! `write_schemas` enforcement) is not modelled. Batches share one
//! connection, so they run one after another, as the tests drive them.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agentic_connector::{ConnectorError, DatabaseConnector, DuckDbConnector};
use airhouse::preview_sql::PreviewNamespace;
use async_trait::async_trait;
use uuid::Uuid;

use super::ddl::SchemaCreator;
use super::ddl_duckdb::DuckDbPreviewDdl;
use crate::agentic_wiring::preview_airhouse::{
    Lease, PreviewAirhouseBackend, PreviewAirhousePorts, Scope, Use, Writers,
};

/// What an Airhouse older than 0.1.49 answers, standing in.
pub const OLD_AIRHOUSE: &str =
    "this Airhouse cannot confine a Writer to named schemas (stand-in for airhouse 0.1.48)";

/// What a mint whose `write_schemas` echo came back wrong answers, standing in.
pub const UNCONFINED_ECHO: &str =
    "Airhouse did not confine this preview Writer (stand-in for an echo that does not match)";

pub struct DuckDbAirhouse {
    ddl: Arc<Mutex<duckdb::Connection>>,
    connector: Arc<dyn DatabaseConnector>,
    /// What the capabilities probe answers.
    deployment: Writers,
    /// What minting the step's exact-scope Writer answers.
    confine: Writers,
    /// Writer checkouts still to fail (a connection that drops), then none.
    failing_writes: Arc<AtomicUsize>,
}

impl DuckDbAirhouse {
    /// An Airhouse that confines preview Writers (0.1.49 and later), over the
    /// database `conn` is open on.
    pub fn new(conn: Arc<Mutex<duckdb::Connection>>) -> Result<Self, String> {
        Self::answering(conn, Writers::Confined, Writers::Confined)
    }

    /// One that cannot: every preview write stays held.
    pub fn old(conn: Arc<Mutex<duckdb::Connection>>) -> Result<Self, String> {
        let no = Writers::Unavailable(OLD_AIRHOUSE.into());
        Self::answering(conn, no.clone(), no)
    }

    /// One that says it can, then mints a Writer whose scope echo is wrong.
    pub fn unconfined_echo(conn: Arc<Mutex<duckdb::Connection>>) -> Result<Self, String> {
        let echo = Writers::Unavailable(UNCONFINED_ECHO.into());
        Self::answering(conn, Writers::Confined, echo)
    }

    fn answering(
        conn: Arc<Mutex<duckdb::Connection>>,
        deployment: Writers,
        confine: Writers,
    ) -> Result<Self, String> {
        let batches = conn
            .lock()
            .map_err(|_| "duckdb connection poisoned".to_string())?
            .try_clone()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            ddl: conn,
            connector: Arc::new(DuckDbConnector::new(batches)),
            deployment,
            confine,
            failing_writes: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// The next `n` Writer checkouts fail before anything is sent, as a
    /// dropped connection would.
    pub fn with_failing_writes(self, n: usize) -> Self {
        self.failing_writes.store(n, Ordering::SeqCst);
        self
    }

    pub fn shared(self) -> Arc<dyn PreviewAirhousePorts> {
        Arc::new(self)
    }
}

/// Every batch on the one connection.
struct Batches {
    conn: Arc<dyn DatabaseConnector>,
    failing_writes: Arc<AtomicUsize>,
}

#[async_trait]
impl PreviewAirhouseBackend for Batches {
    async fn checkout(&self, scope: &Scope, _usage: Use) -> Result<Lease, ConnectorError> {
        let fail = matches!(scope, Scope::Writer(_))
            && self
                .failing_writes
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok();
        if fail {
            return Err(ConnectorError::ConnectionError(
                "stand-in: the Writer's connection dropped".into(),
            ));
        }
        Ok(Lease::new(Arc::clone(&self.conn)))
    }
}

#[async_trait]
impl PreviewAirhousePorts for DuckDbAirhouse {
    fn backend(
        &self,
        _workspace_id: Uuid,
        _ns: &PreviewNamespace,
    ) -> Arc<dyn PreviewAirhouseBackend> {
        Arc::new(Batches {
            conn: Arc::clone(&self.connector),
            failing_writes: Arc::clone(&self.failing_writes),
        })
    }

    fn schema_creator(&self, _workspace_id: Uuid, ns: &PreviewNamespace) -> Arc<dyn SchemaCreator> {
        Arc::new(DuckDbPreviewDdl::new(Arc::clone(&self.ddl), ns.clone()))
    }

    async fn deployment_writers(&self) -> Result<Writers, String> {
        Ok(self.deployment.clone())
    }

    async fn confine_writer(
        &self,
        _workspace_id: Uuid,
        _ns: &PreviewNamespace,
        _schemas: &[String],
    ) -> Result<Writers, String> {
        Ok(self.confine.clone())
    }

    /// DuckDB's in-memory database is catalog `memory`; like an unconfigured
    /// deployment, the stand-in names none, so catalog-qualified names are
    /// refused.
    fn catalog(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    /// No server code names the stand-in: only this module and test files do.
    #[test]
    fn the_duckdb_stand_in_is_never_wired_into_the_server() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        visit(&src, &mut |path, text| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            let test_file =
                name.ends_with("tests.rs") || path.components().any(|c| c.as_os_str() == "tests");
            let mentions = text.contains("DuckDbAirhouse") || text.contains("airhouse_duckdb::");
            if mentions && !test_file && name != "airhouse_duckdb.rs" {
                offenders.push(path.display().to_string());
            }
        });
        assert!(
            offenders.is_empty(),
            "server code uses the stand-in: {offenders:?}"
        );
    }

    fn visit(dir: &std::path::Path, f: &mut dyn FnMut(&std::path::Path, &str)) {
        for entry in std::fs::read_dir(dir).expect("src is readable").flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, f);
            } else if path.extension().is_some_and(|e| e == "rs") {
                f(&path, &std::fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
}
