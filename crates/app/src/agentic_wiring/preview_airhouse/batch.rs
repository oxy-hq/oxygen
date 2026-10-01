//! One batch: plan it (verify every statement, overlay the reads, pick the
//! credential and how the connection is used), then send it on one
//! connection, rolling back a transaction a failed statement leaves open.

use std::collections::BTreeSet;
use std::time::Duration;

use agentic_connector::{ConnectorError, DatabaseConnector, ExecutionResult};
use agentic_core::result::TypedRowStream;
use airhouse::preview_sql::{
    PreviewNamespace, Refused, RewriteOptions, ShadowMap, StatementRole, Verified, overlay_reads,
    verify_statements,
};
use futures::StreamExt;

use super::backend::{Lease, Scope, Use};

/// A batch ready to send: the credential it runs on, how it uses its
/// connection, and its statements as verified (reads with the overlay
/// applied).
#[derive(Debug)]
pub(super) struct Plan {
    pub scope: Scope,
    pub usage: Use,
    pub statements: Vec<Verified>,
}

/// Verify every statement of `sql` — one refusal refuses the batch, and
/// nothing is sent — then point each read at the preview's copies, and scope
/// the batch's credential to exactly the preview schemas it writes. A batch
/// with a `BEGIN` holds its connection alone; any other shares it.
///
/// Reads are verified before the overlay; the overlay only renames tables to
/// this preview's own schemas, and refuses anything that is not a read. The
/// reads *inside* a write (`INSERT … SELECT`, `UPDATE … FROM`) are not
/// overlaid here: a procedure step's SQL already went through
/// `preview_sql::rewrite`, which overlays them, and the verifier has checked
/// the write as it will be sent.
///
/// Schema DDL is refused even for the preview's own schemas: one is created
/// only after its registry row (`previews::registry::ensure_schema`), or the
/// TTL sweep would never drop it, and dropped only by that sweep.
///
/// A write is sent only to relations the run's shadow map already records.
/// The map is the record of what the preview made, and the TTL drop drops
/// only what it records: a relation made any other way (an agent's `CREATE
/// TABLE "preview_…__s".scratch AS …` straight through this connector) would
/// be someone else's to the drop, and keep its schema — and whatever it copied
/// — for good. A procedure step passes because its review records the step
/// (`registry::record_step`) and moves this map on before the step is sent.
pub(super) fn plan(
    sql: &str,
    ns: &PreviewNamespace,
    shadow: &ShadowMap,
    opts: &RewriteOptions,
) -> Result<Plan, Refused> {
    let mut statements = verify_statements(sql, ns, opts.catalog.as_deref())?;
    let mut written = BTreeSet::new();
    let mut usage = Use::Shared;
    for statement in &mut statements {
        match &statement.role {
            StatementRole::Read => {
                statement.sql = overlay_reads(&statement.sql, ns, shadow, opts)?;
            }
            StatementRole::Write(schemas) => {
                recorded(statement, ns, shadow)?;
                written.extend(schemas.iter().cloned());
            }
            StatementRole::SchemaDdl(_) => {
                return Err(Refused(format!(
                    "`{}`: a preview's schemas are created by its registry and dropped by the \
                     TTL sweep, never by a statement it sends",
                    statement.sql
                )));
            }
            StatementRole::Begin => usage = Use::Transaction,
            StatementRole::End => {}
        }
    }
    let scope = if written.is_empty() {
        Scope::Reader
    } else {
        Scope::Writer(written.into_iter().collect())
    };
    Ok(Plan {
        scope,
        usage,
        statements,
    })
}

/// Refuse a write to a relation the run's shadow map does not record ([`plan`]).
fn recorded(
    statement: &Verified,
    ns: &PreviewNamespace,
    shadow: &ShadowMap,
) -> Result<(), Refused> {
    let prefix = ns.prefix();
    for (schema, relation) in &statement.relations {
        let live = schema.strip_prefix(&prefix).unwrap_or(schema);
        if shadow
            .state(&(live.to_string(), relation.clone()))
            .is_none()
        {
            return Err(Refused(format!(
                "`{}` writes {schema}.{relation}, which this preview run has not recorded; only a \
                 procedure step's own reviewed SQL may create or change a preview relation",
                statement.sql
            )));
        }
    }
    Ok(())
}

/// How the caller asked for the batch, which decides how its result
/// statement — the last one that is not `BEGIN`/`COMMIT`/`ROLLBACK` — is sent.
#[derive(Clone, Copy)]
pub(super) enum Call<'a> {
    Statement,
    Tagged(&'a str),
    Query(u64),
}

/// Whether `statement`'s rows are dropped rather than kept: a read whose rows
/// nobody asked for. The verifier passed it as a read, so its rows have no
/// effect to keep, but its binding still runs — see [`send_unsent`] for how.
fn unsent(statement: &Verified, is_result: bool) -> bool {
    statement.role == StatementRole::Read && !is_result
}

/// Whether `sql` is shaped so DuckDB accepts it wrapped — in `EXPLAIN`,
/// `DESCRIBE (…)`, or `SELECT * FROM (…)` — the same `SELECT`/`WITH`/`FROM`/
/// `TABLE`/`VALUES` leading keywords the airhouse connector's `classify`
/// (`crates/airhouse/src/connector/mod.rs`) treats as `StatementKind::Subquery`.
/// Everything else `is_read_only` also allows as a read — `SHOW`, `DESCRIBE`,
/// `EXPLAIN` itself, `SUMMARIZE`, `PIVOT`, `UNPIVOT` — is a top-level
/// statement DuckDB errors on wrapping (`EXPLAIN DESCRIBE t` fails), so
/// [`send_unsent`] runs it for real instead. A parenthesized query body
/// (`(SELECT …)`) has no bare leading keyword either — matching `classify`,
/// which falls through to `DdlDml` for the same reason — so it counts as not
/// explainable too.
fn explainable(sql: &str) -> bool {
    let first = sql
        .trim_start()
        .split_ascii_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    matches!(
        first.as_str(),
        "SELECT" | "WITH" | "FROM" | "TABLE" | "VALUES"
    )
}

/// Bind `statement`'s names and types without scanning: what an [`explainable`]
/// [`unsent`] statement runs in place of the full read, so a missing table or
/// column still fails the batch.
async fn explain(conn: &dyn DatabaseConnector, statement: &Verified) -> Result<(), ConnectorError> {
    conn.execute_statement(&format!("EXPLAIN {}", statement.sql))
        .await
}

/// Send an [`unsent`] statement: [`explainable`] SQL binds via `EXPLAIN`,
/// without scanning; anything else `is_read_only` allows but DuckDB cannot
/// wrap runs in full instead ([`send_discarding`]) — a binding error still
/// fails the batch either way, only the cost differs.
async fn send_unsent(
    conn: &dyn DatabaseConnector,
    statement: &Verified,
) -> Result<(), ConnectorError> {
    if explainable(&statement.sql) {
        explain(conn, statement).await
    } else {
        send_discarding(conn, statement, None).await
    }
}

/// Send one statement, discarding what it returns. A read (the result of an
/// `execute_statement` call) runs once: the untyped full-result path executes
/// it a single time, where `execute_statement` would plan and profile it as a
/// sampled query.
async fn send_discarding(
    conn: &dyn DatabaseConnector,
    statement: &Verified,
    tag: Option<&str>,
) -> Result<(), ConnectorError> {
    match (&statement.role, tag) {
        (StatementRole::Read, _) => conn
            .execute_query_full_untyped(&statement.sql)
            .await
            .map(drop),
        // Bare: Airhouse lets a scoped Writer run transaction control only
        // when nothing else is in the query, a comment included.
        (StatementRole::Begin | StatementRole::End, _) | (_, None) => {
            conn.execute_statement(&statement.sql).await
        }
        (_, Some(tag)) => conn.execute_statement_tagged(&statement.sql, tag).await,
    }
}

/// A connection with a transaction this batch may have opened. Dropped while
/// one may be open — a statement failed and the rollback did not run, or the
/// caller gave up mid-batch — it rolls back before anyone else gets the
/// connection, or poisons it.
struct Session {
    lease: Option<Lease>,
    open: bool,
}

impl Session {
    fn conn(&self) -> &dyn DatabaseConnector {
        self.lease
            .as_ref()
            .expect("the lease is held until the session ends")
            .connector()
    }

    /// Send one statement as `call` asks, keeping track of the transaction.
    /// Only the result statement's rows are kept; a read before it has its
    /// rows dropped ([`unsent`], [`send_unsent`]), but a binding error still
    /// fails the batch.
    ///
    /// The transaction counts as open from the moment `BEGIN` is sent, not
    /// when it succeeds: a `BEGIN` cancelled in flight may have run, and a
    /// spurious `ROLLBACK` is harmless where a missing one is not. It counts
    /// as closed only once `COMMIT`/`ROLLBACK` succeeds.
    async fn send(
        &mut self,
        statement: &Verified,
        call: Call<'_>,
        is_result: bool,
    ) -> Result<Option<ExecutionResult>, ConnectorError> {
        if unsent(statement, is_result) {
            send_unsent(self.conn(), statement).await?;
            return Ok(None);
        }
        if statement.role == StatementRole::Begin {
            self.open = true;
        }
        let conn = self.conn();
        let sent = match call {
            Call::Query(limit) if is_result && statement.role == StatementRole::Read => {
                conn.execute_query(&statement.sql, limit).await.map(Some)
            }
            Call::Tagged(tag) => send_discarding(conn, statement, Some(tag))
                .await
                .map(|_| None),
            _ => send_discarding(conn, statement, None).await.map(|_| None),
        };
        if statement.role == StatementRole::End && sent.is_ok() {
            self.open = false;
        }
        sent
    }

    /// Roll back a transaction the failed statement may have left open, then
    /// report the failure. The error is the statement's. Cancelled while it
    /// rolls back, the transaction still counts as open, so `Drop` rolls back
    /// again.
    async fn abort(&mut self, error: ConnectorError) -> ConnectorError {
        if self.open
            && let Some(lease) = self.lease.as_mut()
        {
            roll_back(lease, "a failed batch").await;
        }
        self.open = false;
        error
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let Some(mut lease) = self.lease.take().filter(|_| self.open) else {
            return;
        };
        // The caller gave up inside a transaction. The lease goes with the
        // rollback, so nobody is handed the connection mid-transaction; with
        // no runtime to roll back on the connection is poisoned instead.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            lease.poison();
            return;
        };
        runtime.spawn(async move { roll_back(&mut lease, "an abandoned batch").await });
    }
}

/// How long a `ROLLBACK` may take. It queues behind whatever an abandoned
/// statement is still running on the connection, so an unbounded wait would
/// hold the pool slot for as long as that runs.
const ROLLBACK_WITHIN: Duration = Duration::from_secs(30);

/// Roll back on `lease`'s connection, or poison it: a `ROLLBACK` that fails,
/// or gets no answer within [`ROLLBACK_WITHIN`], may leave the connection
/// inside the transaction, so it is never handed out again.
async fn roll_back(lease: &mut Lease, batch: &str) {
    let sent = tokio::time::timeout(
        ROLLBACK_WITHIN,
        lease.connector().execute_statement("ROLLBACK"),
    )
    .await;
    let error = match sent {
        Ok(Ok(())) => return,
        Ok(Err(e)) => e.to_string(),
        Err(_) => format!("no answer within {ROLLBACK_WITHIN:?}"),
    };
    tracing::warn!(target: "preview", %error,
        "preview airhouse: rolling back {batch} failed; dropping its connection");
    lease.poison();
}

/// Send every statement on `lease`'s one connection, in order. A failure
/// stops the batch, rolls back a transaction it opened, and is returned. The
/// result statement's rows come back for [`Call::Query`].
pub(super) async fn run(
    lease: Lease,
    statements: &[Verified],
    call: Call<'_>,
) -> Result<Option<ExecutionResult>, ConnectorError> {
    let result_at = statements
        .iter()
        .rposition(|s| !matches!(s.role, StatementRole::Begin | StatementRole::End));
    let mut session = Session {
        lease: Some(lease),
        open: false,
    };
    let mut result = None;
    for (i, statement) in statements.iter().enumerate() {
        match session.send(statement, call, Some(i) == result_at).await {
            Ok(Some(rows)) => result = Some(rows),
            Ok(None) => {}
            Err(e) => return Err(session.abort(e).await),
        }
    }
    Ok(result)
}

/// Why a batch cannot be streamed, if it cannot: the streamed statement's
/// pages are read after this call returns, so it must come last, and no
/// transaction may be open around it.
pub(super) fn unstreamable(statements: &[Verified]) -> Option<&'static str> {
    if statements
        .iter()
        .any(|s| matches!(s.role, StatementRole::Begin | StatementRole::End))
    {
        return Some("a streamed result cannot be read inside BEGIN … COMMIT");
    }
    match statements.last().map(|s| &s.role) {
        Some(StatementRole::Read) => None,
        _ => Some("only a read can be streamed, as the batch's last statement"),
    }
}

/// Send all but the last statement (reads among them have their rows
/// dropped, [`unsent`], [`send_unsent`]), then stream the last. Check
/// [`unstreamable`] first: a streamed batch has no transaction, so its
/// connection is a shared one and the stream holds nothing another query
/// waits on.
pub(super) async fn stream(
    lease: Lease,
    statements: &[Verified],
    untyped: bool,
) -> Result<TypedRowStream, ConnectorError> {
    let (last, head) = statements
        .split_last()
        .ok_or_else(|| ConnectorError::Other("an empty batch".into()))?;
    for statement in head {
        if unsent(statement, false) {
            send_unsent(lease.connector(), statement).await?;
        } else {
            send_discarding(lease.connector(), statement, None).await?;
        }
    }
    let conn = lease.connector();
    let mut streamed = if untyped {
        conn.execute_query_full_untyped(&last.sql).await?
    } else {
        conn.execute_query_full(&last.sql).await?
    };
    let rows = std::mem::replace(&mut streamed.rows, Box::pin(futures::stream::empty()));
    streamed.rows = Box::pin(rows.map(move |row| {
        let _connection = &lease;
        row
    }));
    Ok(streamed)
}
