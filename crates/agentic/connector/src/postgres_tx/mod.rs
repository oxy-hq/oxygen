//! Postgres implementation of [`SqlTransaction`].
//!
//! ## Why a dedicated connection, not the connector's
//!
//! [`PostgresConnector`] holds one lazily-opened client behind a `Mutex` that
//! serialises queries. A transaction has to pin a session across *several*
//! `await` points, so reusing that client would mean holding its mutex for the
//! whole transaction — every other query on that database would block behind an
//! app's `BEGIN`, for as long as the app's JavaScript takes to run.
//!
//! So `begin_transaction` opens its own connection. It costs a connect per
//! transaction, which is the correct trade: transactions are rare relative to
//! queries, and the alternative couples an app's control flow to every other
//! reader of the same database.
//!
//! ## Why `Drop` cannot commit
//!
//! An abandoned transaction must roll back. The isolate can be terminated
//! mid-transaction — a timeout, a client disconnect, a dashboard cancel — and
//! at that point no Rust code of ours is guaranteed to run a `ROLLBACK`. The
//! defence is that dropping the handle aborts the connection driver task and
//! drops the client, which closes the socket; Postgres rolls back any open
//! transaction on an unexpected disconnect. That makes "we crashed" and "we
//! rolled back" the same outcome at the server, which is the only property
//! worth relying on here.
//!
//! [`PostgresConnector`]: crate::postgres::PostgresConnector
//! [`SqlTransaction`]: crate::transaction::SqlTransaction

mod convert;

use agentic_core::hub_task::spawn_with_hub;
use async_trait::async_trait;
use tokio_postgres::Client;
use tokio_postgres::types::ToSql;

use crate::connector::ConnectorError;
use crate::pg_error::{pg_error_message, pg_query_failed};
use crate::transaction::{SqlTransaction, TxParams};

use convert::{OwnedParam, check_decodable, json_to_param, row_to_json};

/// Ceiling on statements issued through one transaction handle.
///
/// A backstop against a runaway loop in app code holding a write transaction
/// open indefinitely, not a budget an honest app should ever notice. The
/// wall-clock ceiling that actually bounds a transaction is enforced by the
/// caller (the function runtime's per-invocation timeout).
const MAX_STATEMENTS: u64 = 1_000;

/// Row ceiling for a single `query` inside a transaction.
///
/// Deliberately far below `ctx.query`'s 100k: those rows are materialised while
/// the transaction — and every lock it holds — stays open, so the cost of a
/// runaway `SELECT *` here is lock duration on live tables, not just pod memory.
/// Bulk reads belong outside the transaction.
const MAX_ROWS: usize = 10_000;

/// Server-side backstops set alongside `BEGIN`.
///
/// The transaction stays open across the author's JavaScript, which may await
/// anything — including a `ctx.fetch` to a slow third party. Our own ceilings
/// cannot cover the case where the isolate dies while a host op is in flight
/// and the handle is not dropped promptly, so make the *server* the backstop:
/// Postgres kills the session itself rather than trusting us to.
const STATEMENT_TIMEOUT_MS: u32 = 30_000;
const IDLE_IN_TRANSACTION_TIMEOUT_MS: u32 = 60_000;

/// Budget for the best-effort cancel of an abandoned read.
///
/// `cancel_query` opens a *new* connection, and the connector's config carries
/// no `connect_timeout` — so if the database has just become unreachable this
/// would block until the caller's 330s ceiling, turning a fast, actionable
/// error ("add a LIMIT") into a stalled invocation. The comment on the call
/// says best-effort; this is what makes that true.
const CANCEL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// A pinned Postgres session with an open `BEGIN`.
pub struct PgTransaction {
    client: Option<Client>,
    /// Driver task for the dedicated connection. Aborted on drop so the socket
    /// closes and the server rolls back — see the module docs.
    driver: tokio::task::JoinHandle<()>,
    statements: u64,
    /// Set when the block can no longer be committed. See
    /// [`PgTransaction::commit`] for why this has to be tracked rather than
    /// inferred from `COMMIT`'s result.
    poisoned: Option<Poison>,
    /// Set once we have sent a cancel for an abandoned read. From then on this
    /// connection must not carry another statement — see [`PgTransaction::finish`].
    abandoned: bool,
    /// Whether to verify the server certificate — carried so `cancel_query`
    /// (a fresh connection) uses the same posture as `begin`. Must match the
    /// `sslmode` in the config, or a managed tenant's cancel would connect
    /// plaintext against an `sslmode=require` server and fail.
    verify_tls: bool,
}

/// Why a transaction can no longer be committed.
///
/// The distinction is author-facing, not bookkeeping: the two arrive by
/// different routes and imply different fixes, and a single message covering
/// both would be wrong for one of them.
#[derive(Debug, Clone)]
struct Poison {
    sql: String,
    cause: PoisonCause,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PoisonCause {
    /// Postgres rejected the statement, which aborts the block server-side.
    ServerRejected,
    /// *We* stopped reading a result mid-stream — the row cap, or a column we
    /// cannot decode. The statement itself was fine; we then cancel it, which
    /// is what actually aborts the block. Distinguished because the errors on
    /// these paths name a fix (`LIMIT`, `::text`) and an author who applies it
    /// needs to know the retry belongs in a *new* transaction.
    AbandonedMidRead,
}

impl PgTransaction {
    /// Open a dedicated connection, issue `BEGIN`, and return the handle.
    ///
    /// `verify_tls` mirrors [`crate::postgres::PostgresConnector`]'s field:
    /// `config` already carries the `sslmode` (so a managed tenant is
    /// `Require`), and this decides whether the certificate is checked. Passing
    /// a real TLS connector rather than `NoTls` is what lets `ctx.oltp` reach a
    /// managed tenant at all — `Require` + `NoTls` cannot negotiate.
    ///
    /// This also changes `ctx.tx` on an author-configured `type: postgres`
    /// database: at its `Prefer` default `begin` now negotiates TLS where it
    /// never did, falling back to plaintext when the server declines. Strictly
    /// better (opportunistic encryption, no new failure), but a behaviour change
    /// on a path outside the OLTP feature — worth knowing when reading it.
    pub(crate) async fn begin(
        config: &tokio_postgres::Config,
        verify_tls: bool,
    ) -> Result<Self, ConnectorError> {
        let (client, connection) = config
            .connect(crate::postgres::tls_connector(verify_tls))
            .await
            .map_err(|e| {
                ConnectorError::ConnectionError(format!("connect failed: {}", pg_error_message(&e)))
            })?;
        let driver = spawn_with_hub(async move {
            if let Err(e) = connection.await {
                tracing::debug!("transaction connection closed: {e}");
            }
        });
        let tx = Self {
            client: Some(client),
            driver,
            statements: 0,
            poisoned: None,
            abandoned: false,
            verify_tls,
        };
        // One round trip: the guards must be in force before the author's first
        // statement, and `SET` inside the block scopes them to this transaction.
        tx.simple(&format!(
            "BEGIN; SET LOCAL statement_timeout = {STATEMENT_TIMEOUT_MS}; \
             SET LOCAL idle_in_transaction_session_timeout = {IDLE_IN_TRANSACTION_TIMEOUT_MS};"
        ))
        .await?;
        Ok(tx)
    }

    /// Run a statement with no parameters and no result (`BEGIN`/`COMMIT`/`ROLLBACK`).
    async fn simple(&self, sql: &str) -> Result<(), ConnectorError> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| ConnectorError::Other("transaction already closed".into()))?;
        client
            .batch_execute(sql)
            .await
            .map_err(|e| ConnectorError::QueryFailed(pg_query_failed(sql, &e)))
    }

    /// Prepare `sql`, bind `params` to the types Postgres inferred, and hand
    /// back everything the caller needs to execute it.
    ///
    /// Preparing first is what lets a JSON number bind correctly to an `int4`
    /// column — see `convert`'s module docs.
    async fn bind(
        &mut self,
        sql: &str,
        params: &TxParams,
    ) -> Result<(tokio_postgres::Statement, Vec<OwnedParam>, bool), ConnectorError> {
        self.statements += 1;
        if self.statements > MAX_STATEMENTS {
            return Err(ConnectorError::Other(format!(
                "a single transaction may issue at most {MAX_STATEMENTS} statements. \
                 Batch the work (one INSERT with many rows) or split it across transactions."
            )));
        }
        // Arm the poison marker *before* the first server round trip, not after
        // it resolves. If this future is dropped mid-statement — the host op
        // timeout, a cancelled invocation — no code of ours runs afterwards, and
        // an un-armed marker would let a later `commit()` through on a block the
        // server may already have aborted. Armed-then-cleared is the
        // cancellation-safe order; the cost is that a cancelled statement
        // conservatively poisons a block that might have been fine.
        let armed = self.arm(sql);

        let client = self
            .client
            .as_ref()
            .ok_or_else(|| ConnectorError::Other("transaction already closed".into()))?;
        // `prepare` is a server round trip, so a rejection here (syntax error,
        // unknown relation) aborts the transaction block just as a failed
        // execute does — leave it armed.
        let stmt = client
            .prepare(sql)
            .await
            .map_err(|e| ConnectorError::QueryFailed(pg_query_failed(sql, &e)))?;

        let expected = stmt.params();
        if expected.len() != params.len() {
            // Client-side from here down: `prepare` succeeded, so the block is
            // healthy and no further statement reaches the server. Disarm.
            self.disarm(armed);
            return Err(ConnectorError::query_failed(
                sql,
                format!(
                    "this statement takes {} parameter(s) but {} were passed",
                    expected.len(),
                    params.len()
                ),
            ));
        }
        let owned = expected
            .iter()
            .zip(params.iter())
            .enumerate()
            .map(|(i, (ty, value))| json_to_param(value, ty, i + 1))
            .collect::<Result<Vec<_>, _>>();
        let owned = match owned {
            Ok(o) => o,
            Err(e) => {
                self.disarm(armed);
                return Err(e);
            }
        };
        Ok((stmt, owned, armed))
    }

    /// Close out the transaction with `sql`, then release the connection.
    async fn finish(mut self: Box<Self>, sql: &str) -> Result<(), ConnectorError> {
        // A handle whose client is already gone has nothing to close; make the
        // caller's unconditional-cleanup path safe.
        if self.client.is_none() {
            return Ok(());
        }
        if self.abandoned {
            // A cancel request is processed asynchronously and Postgres
            // documents that it may arrive after its target has finished — in
            // which case it cancels whatever the backend runs NEXT. Issuing a
            // ROLLBACK here is exactly that next statement, so it could come
            // back as a bare 57014 nobody caused. Drop instead: closing the
            // socket rolls the block back at the server just as reliably (see
            // the module docs) and cannot be cancelled out from under us.
            self.client = None;
            self.driver.abort();
            return Ok(());
        }
        let result = self.simple(sql).await;
        self.client = None;
        self.driver.abort();
        result
    }

    /// Provisionally mark `sql` as having poisoned the block.
    ///
    /// Returns whether *this* call armed it, so a nested disarm cannot clear a
    /// marker set by an earlier failed statement.
    fn arm(&mut self, sql: &str) -> bool {
        if self.poisoned.is_none() {
            self.poisoned = Some(Poison {
                sql: sql.to_string(),
                cause: PoisonCause::ServerRejected,
            });
            true
        } else {
            false
        }
    }

    /// Re-label an armed marker once we know *we* abandoned the read rather
    /// than the server rejecting it.
    fn mark_abandoned(&mut self, armed: bool) {
        if armed && let Some(p) = self.poisoned.as_mut() {
            p.cause = PoisonCause::AbandonedMidRead;
        }
    }

    /// Clear a marker this call armed, once the statement is known to have left
    /// the block healthy.
    fn disarm(&mut self, armed: bool) {
        if armed {
            self.poisoned = None;
        }
    }
}

/// Borrow the owned params as the slice shape `tokio_postgres` wants.
fn as_refs(owned: &[OwnedParam]) -> Vec<&(dyn ToSql + Sync)> {
    owned
        .iter()
        .map(|p| p.as_ref() as &(dyn ToSql + Sync))
        .collect()
}

impl Drop for PgTransaction {
    fn drop(&mut self) {
        if self.client.is_some() {
            tracing::warn!(
                "transaction handle dropped without commit/rollback; \
                 closing the connection so Postgres rolls it back"
            );
        }
        self.driver.abort();
    }
}

#[async_trait]
impl SqlTransaction for PgTransaction {
    async fn query(
        &mut self,
        sql: &str,
        params: &TxParams,
    ) -> Result<Vec<serde_json::Value>, ConnectorError> {
        use futures::TryStreamExt;

        let (stmt, owned, armed) = self.bind(sql, params).await?;
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| ConnectorError::Other("transaction already closed".into()))?;

        // `query_raw`, not `query`: the latter collects every row before we
        // could check the cap, so a `SELECT *` over a large table spikes pod
        // memory before we ever see a row count. Streaming stops us *buffering*
        // at the ceiling — stopping the SERVER needs the cancel below, because
        // dropping the stream leaves it happily producing rows that
        // tokio-postgres then drains on the connection driver, queueing every
        // later statement (the ROLLBACK included) behind that drain.
        // Pre-flight, before a single row moves: an undecodable column caught
        // here leaves the block healthy, so the author's `::text` retry works
        // inside this same transaction. Caught mid-stream it would not — see
        // `check_decodable`.
        if let Err(e) = check_decodable(stmt.columns()) {
            self.disarm(armed);
            return Err(e);
        }

        let cancel = client.cancel_token();
        // `Err(_, true)` = we abandoned the read; the statement did not fail.
        let result: Result<Vec<serde_json::Value>, (ConnectorError, bool)> = async {
            let stream = client
                .query_raw(&stmt, as_refs(&owned))
                .await
                .map_err(|e| (ConnectorError::QueryFailed(pg_query_failed(sql, &e)), false))?;
            futures::pin_mut!(stream);

            let mut out = Vec::new();
            while let Some(row) = stream
                .try_next()
                .await
                .map_err(|e| (ConnectorError::QueryFailed(pg_query_failed(sql, &e)), false))?
            {
                if out.len() >= MAX_ROWS {
                    // An error, not a truncation flag like `ctx.query`:
                    // silently returning a prefix from inside a transaction
                    // invites the author to write their next statement against
                    // an incomplete read.
                    return Err((
                        ConnectorError::query_failed(
                            sql,
                            format!(
                                "this query returned more than {MAX_ROWS} rows. Add a \
                                 LIMIT — a transaction holds its locks for as long as it reads, \
                                 so a bulk read belongs on your normal read path, outside a \
                                 transaction. Note this ends the transaction: rerun the fixed \
                                 query in a new one."
                            ),
                        ),
                        true,
                    ));
                }
                out.push(row_to_json(&row).map_err(|e| (e, true))?);
            }
            Ok(out)
        }
        .await;

        match result {
            Ok(rows) => {
                self.disarm(armed);
                Ok(rows)
            }
            Err((e, abandoned)) => {
                if abandoned {
                    // The stream is dropped by now. Cancel so the server stops
                    // producing rows we will never read: it frees the backend,
                    // unblocks the ROLLBACK that follows, and — because a
                    // cancelled query errors with 57014 — genuinely aborts the
                    // block, which is what makes the poison we keep truthful
                    // rather than merely conservative. Best-effort: it races
                    // the query finishing on its own, and losing that race only
                    // costs us a transaction we were ending anyway.
                    match tokio::time::timeout(
                        CANCEL_TIMEOUT,
                        cancel.cancel_query(crate::postgres::tls_connector(self.verify_tls)),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            tracing::debug!("cancelling an abandoned read failed: {e}")
                        }
                        Err(_) => tracing::debug!(
                            "cancelling an abandoned read timed out after {:?}",
                            CANCEL_TIMEOUT
                        ),
                    }
                    self.abandoned = true;
                    self.mark_abandoned(armed);
                }
                Err(e)
            }
        }
    }

    async fn exec(&mut self, sql: &str, params: &TxParams) -> Result<u64, ConnectorError> {
        let (stmt, owned, armed) = self.bind(sql, params).await?;
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| ConnectorError::Other("transaction already closed".into()))?;
        let outcome = client
            .execute(&stmt, &as_refs(&owned))
            .await
            .map_err(|e| ConnectorError::QueryFailed(pg_query_failed(sql, &e)));
        if outcome.is_ok() {
            self.disarm(armed);
        }
        outcome
    }

    /// Commit — unless the block can no longer be committed.
    ///
    /// **`COMMIT` on an aborted transaction does not error.** Postgres ends the
    /// block and reports the `ROLLBACK` command tag, so the driver returns
    /// `Ok(())` and nothing distinguishes "committed 40 rows" from "applied
    /// nothing". An author who catches a failed statement and returns normally
    /// — the ordinary "try the insert, ignore the conflict" idiom — would get a
    /// resolved `ctx.tx()` over an empty write.
    ///
    /// So refuse explicitly, roll back, and say which of the two things
    /// happened. A caller who wanted the failure ignored can still `rollback()`
    /// deliberately.
    async fn commit(mut self: Box<Self>) -> Result<(), ConnectorError> {
        if let Some(p) = self.poisoned.take() {
            let _ = self.finish("ROLLBACK").await;
            let explanation = match p.cause {
                PoisonCause::ServerRejected => {
                    "an earlier statement failed and aborted this transaction, so committing \
                     would silently persist nothing. The transaction has been rolled back. Let \
                     the error propagate out of the callback instead of catching it, or roll \
                     back deliberately."
                }
                // Deliberately does NOT claim the statement failed — it did not.
                // We stopped reading and cancelled it, which is what ended the
                // block. Retrying the fixed query in the same transaction cannot
                // work, so say where it belongs instead.
                PoisonCause::AbandonedMidRead => {
                    "this transaction stopped reading a result part-way (the row cap, or a \
                     column that cannot be decoded) and was cancelled, so it can no longer be \
                     committed. The statement itself did not fail. Apply the fix the original \
                     error named and run the work again in a NEW transaction — retrying inside \
                     this one cannot succeed."
                }
            };
            return Err(ConnectorError::query_failed(p.sql, explanation.to_string()));
        }
        self.finish("COMMIT").await
    }

    async fn rollback(self: Box<Self>) -> Result<(), ConnectorError> {
        self.finish("ROLLBACK").await
    }
}
