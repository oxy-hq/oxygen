//! The connection a [`super::DuckDbConnector`] runs its statements on, and
//! how a statement gets its turn on it.

use std::{
    ops::Deref,
    sync::{Arc, Mutex, MutexGuard},
};

use agentic_core::hub_task::spawn_blocking_with_hub;
use duckdb::Connection;
use tokio::sync::OwnedMutexGuard;

use super::{dir_cache::SharedDb, read_only};
use crate::connector::ConnectorError;

/// A connector's connection, plus — while that connection is a clone of a
/// cached directory database — the cache entry it was cloned from.
///
/// Derefs to the [`Connection`], so statement code reads exactly as it did
/// when the connector held a bare connection.
pub(super) struct Session {
    conn: Connection,
    shared: Option<Arc<SharedDb>>,
}

impl Session {
    /// A connection nobody else reads.
    pub(super) fn private(conn: Connection) -> Self {
        Self { conn, shared: None }
    }

    /// A clone of `origin`'s database: its catalog is shared with every other
    /// connector checked out of the same entry.
    pub(super) fn shared(conn: Connection, origin: Arc<SharedDb>) -> Self {
        Self {
            conn,
            shared: Some(origin),
        }
    }

    /// Whether this session still reads the shared catalog.
    pub(super) fn is_shared(&self) -> bool {
        self.shared.is_some()
    }
}

impl Deref for Session {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.conn
    }
}

/// A [`Session`] and the queue its statements wait in.
///
/// Statements on one connector run one at a time. They wait for their
/// [`Turn`] asynchronously, so a queue of statements holds no thread; the
/// `std` mutex inside is only ever taken by the holder of the turn (and by
/// the synchronous `introspect_schema`), so taking it does not wait on
/// another statement.
///
/// **Leaving the shared catalog.** A shared session keeps its clone only for
/// a script both the lexer ([`read_only::is_read_only`], the fast route) and
/// DuckDB's own parser ([`super::parse_check`], the guarantee) read as
/// queries; the parser check runs on the blocking pool, since binding can
/// read a file. Anything else first trades the clone for a private database built
/// the way [`super::DuckDbConnector::from_directory`] builds one — the cost
/// every run paid before the cache. That build:
///
/// - runs on the blocking pool, never on a runtime worker;
/// - holds neither the turn nor the session, so reads queued on this
///   connector keep running on the clone while it builds — they come before
///   the write, so the clone is still what they should see;
/// - runs once per connector: concurrent writes wait for the one in flight,
///   asynchronously, then run on its result.
///
/// Everything the session ran before the swap was a read, so no session
/// state is lost. On failure the session keeps its clone, and the statement
/// fails with the build error.
pub(super) struct Sessions {
    session: Arc<Mutex<Session>>,
    turn: Arc<tokio::sync::Mutex<()>>,
    isolating: tokio::sync::Mutex<()>,
}

/// The right to run one statement on the session; held until it finishes,
/// on whatever thread runs it.
pub(super) struct Turn {
    session: Arc<Mutex<Session>>,
    _turn: OwnedMutexGuard<()>,
}

impl Turn {
    pub(super) fn session(&self) -> Result<MutexGuard<'_, Session>, ConnectorError> {
        lock(&self.session)
    }
}

fn lock(session: &Mutex<Session>) -> Result<MutexGuard<'_, Session>, ConnectorError> {
    session
        .lock()
        .map_err(|e| ConnectorError::ConnectionError(format!("mutex poisoned: {e}")))
}

impl Sessions {
    pub(super) fn new(session: Session) -> Self {
        Self {
            session: Arc::new(Mutex::new(session)),
            turn: Arc::new(tokio::sync::Mutex::new(())),
            isolating: tokio::sync::Mutex::new(()),
        }
    }

    /// The session, for a synchronous caller that only reads.
    pub(super) fn session(&self) -> Result<MutexGuard<'_, Session>, ConnectorError> {
        lock(&self.session)
    }

    /// Wait for the turn to run `sql`, on a session `sql` cannot use to write
    /// into a catalog other connectors read.
    pub(super) async fn turn_for(&self, sql: &str) -> Result<Turn, ConnectorError> {
        self.turn_with(sql, read_only::is_read_only).await
    }

    /// [`Self::turn_for`] with the lexer given, so a test can make it wrong
    /// and see the parser hold.
    pub(super) async fn turn_with(
        &self,
        sql: &str,
        lexer_reads: impl Fn(&str) -> bool,
    ) -> Result<Turn, ConnectorError> {
        loop {
            let turn = self.take_turn().await;
            let origin = turn.session()?.shared.clone();
            let Some(origin) = origin else {
                return Ok(turn);
            };
            if lexer_reads(sql) && parser_reads(&origin, sql).await? {
                return Ok(turn);
            }
            drop(turn);
            self.isolate(origin).await?;
        }
    }

    async fn take_turn(&self) -> Turn {
        Turn {
            session: Arc::clone(&self.session),
            _turn: Arc::clone(&self.turn).lock_owned().await,
        }
    }

    /// Move the session onto a private copy of `origin`, unless a concurrent
    /// caller already has.
    async fn isolate(&self, origin: Arc<SharedDb>) -> Result<(), ConnectorError> {
        let _one_build = self.isolating.lock().await;
        if !self.take_turn().await.session()?.is_shared() {
            return Ok(());
        }
        tracing::info!(
            dir = %origin.dir().display(),
            "DuckDB: a statement may write, so this connector leaves the shared directory \
             database for a private copy"
        );
        let private = spawn_blocking_with_hub(move || origin.open_private())
            .await
            .map_err(|e| {
                ConnectorError::ConnectionError(format!("blocking task panicked: {e}"))
            })??;
        let turn = self.take_turn().await;
        let mut session = turn.session()?;
        session.conn = private;
        session.shared = None;
        Ok(())
    }
}

/// DuckDB's parser check ([`SharedDb::only_reads`]), on the blocking pool: it
/// prepares `sql` on the shadow, and binding a query that names a file reads
/// that file, which must not happen on a runtime worker.
async fn parser_reads(origin: &Arc<SharedDb>, sql: &str) -> Result<bool, ConnectorError> {
    let (origin, sql) = (Arc::clone(origin), sql.to_string());
    spawn_blocking_with_hub(move || origin.only_reads(&sql))
        .await
        .map_err(|e| ConnectorError::ConnectionError(format!("blocking task panicked: {e}")))
}
