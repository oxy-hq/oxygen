//! The reads themselves: one session as the app's writer, one `READ ONLY`
//! transaction with a statement timeout, rolled back when done.
//!
//! **Why the writer and not the org's analyst.** The analyst role cannot see
//! an app's schema unless the org opened it to analytics (`app_*` is hidden
//! by default, because live app state may be regulated), and the writer is
//! the one role confined to exactly this app's schema — which is what this
//! browser must be confined to. It holds DML rights; the transaction is
//! `READ ONLY`, so Postgres refuses any write in it, and the only SQL sent is
//! the catalog reads and [`super::page::rows_sql`].
//!
//! Bounded three ways: [`STATEMENT_TIMEOUT_MS`] per statement and a
//! [`LOCK_TIMEOUT_MS`] wait for a lock, both `SET LOCAL` so they end with the
//! transaction, and [`READ_DEADLINE`] around the whole read, connect
//! included (a suspended Neon compute wakes on connect).

use std::time::Duration;

use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Transaction};

use super::super::environment_scope::ScopeError;
use super::catalog::{self, Table, find_table};
use super::error;
use super::page::{self, Cell, OrderTerm, PageRequest};
use oxy_oltp::resolver::WriterConnection;

/// The longest one statement may run.
pub const STATEMENT_TIMEOUT_MS: u32 = 5_000;
/// The longest a statement waits for a lock (a migration holding one).
pub const LOCK_TIMEOUT_MS: u32 = 1_000;
/// The whole read, connect included.
const READ_DEADLINE: Duration = Duration::from_secs(20);

/// `SET LOCAL`: these end with the transaction, so the session carries
/// nothing past this read.
fn guards_sql() -> String {
    format!(
        "SET LOCAL statement_timeout = {STATEMENT_TIMEOUT_MS}; \
         SET LOCAL lock_timeout = {LOCK_TIMEOUT_MS}; \
         SET LOCAL idle_in_transaction_session_timeout = {}",
        READ_DEADLINE.as_millis()
    )
}

/// One page as read: the order it was read in, its rows, and whether more
/// follow.
#[derive(Debug)]
pub struct RowsRead {
    pub table: Table,
    pub order: Vec<OrderTerm>,
    pub limit: u32,
    pub rows: Vec<Vec<Cell>>,
    pub has_more: bool,
}

/// The app schema's tables.
pub async fn tables(connection: &WriterConnection) -> Result<Vec<Table>, ScopeError> {
    deadline(async {
        let mut client = open(connection).await?;
        let txn = begin(&mut client).await?;
        let catalog = catalog::read(&txn, &connection.schema)
            .await
            .map_err(pg_error)?;
        finish(txn).await;
        Ok::<_, ScopeError>(catalog)
    })
    .await
}

/// One page of `table`, which must be a table of the app's schema.
pub async fn rows(
    connection: &WriterConnection,
    table: &str,
    request: &PageRequest,
) -> Result<RowsRead, ScopeError> {
    deadline(async {
        let mut client = open(connection).await?;
        let txn = begin(&mut client).await?;
        let catalog = catalog::read(&txn, &connection.schema)
            .await
            .map_err(pg_error)?;
        let table = find_table(&catalog, table)
            .cloned()
            .ok_or_else(|| error::table_not_found(table))?;
        let order = page::order_terms(&table, request.order.as_ref())?;
        let limit = page::rows_per_page(request.limit, table.columns.len());
        let sql = page::rows_sql(&connection.schema, &table, &order);
        // One row past the page says whether another follows.
        let fetch = i64::from(limit) + 1;
        let offset = i64::from(request.offset);
        let found = txn
            .query(&sql, &[&fetch, &offset])
            .await
            .map_err(pg_error)?;
        finish(txn).await;
        let has_more = found.len() > limit as usize;
        let rows = found
            .iter()
            .take(limit as usize)
            .map(|row| decode(&table, row))
            .collect();
        Ok::<_, ScopeError>(RowsRead {
            table,
            order,
            limit,
            rows,
            has_more,
        })
    })
    .await
}

/// One row's cells, two columns of the result per table column.
fn decode(table: &Table, row: &tokio_postgres::Row) -> Vec<Cell> {
    table
        .columns
        .iter()
        .enumerate()
        .map(|(i, column)| page::cell(column.kind, row.get(2 * i), row.get(2 * i + 1)))
        .collect()
}

async fn deadline<T>(read: impl Future<Output = Result<T, ScopeError>>) -> Result<T, ScopeError> {
    tokio::time::timeout(READ_DEADLINE, read)
        .await
        .unwrap_or_else(|_| Err(error::read_timeout()))
}

async fn open(connection: &WriterConnection) -> Result<Client, ScopeError> {
    oxy_oltp::connect::connect(&connection.dsn, "app data browser")
        .await
        .map_err(|e| error::store_unreachable(oxy_oltp::connect::pg_detail(&e)))
}

/// A `READ ONLY` transaction with the timeouts set.
async fn begin(client: &mut Client) -> Result<Transaction<'_>, ScopeError> {
    let txn = client
        .build_transaction()
        .read_only(true)
        .start()
        .await
        .map_err(pg_error)?;
    txn.batch_execute(&guards_sql()).await.map_err(pg_error)?;
    Ok(txn)
}

/// Nothing was written, so there is nothing to commit; a failed rollback
/// only means the session is going away, which it is.
async fn finish(txn: Transaction<'_>) {
    if let Err(e) = txn.rollback().await {
        tracing::debug!(error = %e, "app data browser: rollback");
    }
}

fn pg_error(e: tokio_postgres::Error) -> ScopeError {
    refusal_for(e.code(), || oxy_oltp::connect::pg_detail(&e))
}

/// What a failed statement tells the operator, by its SQLSTATE. A statement
/// the timeout cancelled, and a sort Postgres has no ordering for (`42883`,
/// the backstop behind [`super::catalog::Column::sortable`]), are the
/// request's to change; anything else is the database's failure, reported
/// in Postgres's own words. Never "unreachable" for a refused request: that
/// sends the operator to an outage and records one in the audit log.
fn refusal_for(code: Option<&SqlState>, detail: impl FnOnce() -> String) -> ScopeError {
    match code {
        Some(c) if *c == SqlState::QUERY_CANCELED || *c == SqlState::LOCK_NOT_AVAILABLE => {
            error::read_timeout()
        }
        Some(c) if *c == SqlState::UNDEFINED_FUNCTION => error::unorderable(),
        _ => error::store_unreachable(detail()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guards are `SET LOCAL`, so a pooled or reused session would not
    /// keep them, and they are the timeouts the response reports.
    #[test]
    fn the_guards_are_local_to_the_transaction() {
        let sql = guards_sql();
        assert_eq!(sql.matches("SET LOCAL").count(), 3, "{sql}");
        assert!(sql.contains(&format!("statement_timeout = {STATEMENT_TIMEOUT_MS}")));
        assert!(sql.contains(&format!("lock_timeout = {LOCK_TIMEOUT_MS}")));
    }

    /// A sort Postgres cannot do is the request's fault, a `400`, never a
    /// `502` that tells the operator the customer's database is down.
    #[test]
    fn no_ordering_operator_is_a_bad_page_not_an_outage() {
        let refused = refusal_for(Some(&SqlState::UNDEFINED_FUNCTION), || "unused".into());
        assert_eq!(refused.status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(refused.code, "invalid_page");
        let timeout = refusal_for(Some(&SqlState::QUERY_CANCELED), || "unused".into());
        assert_eq!(timeout.code, "read_timeout");
        let down = refusal_for(None, || "connection refused".into());
        assert_eq!(down.status, axum::http::StatusCode::BAD_GATEWAY);
        assert_eq!(down.code, "oltp_unreachable");
        assert!(
            down.message.contains("connection refused"),
            "{}",
            down.message
        );
    }
}
