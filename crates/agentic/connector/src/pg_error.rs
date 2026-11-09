//! Turning a `tokio_postgres` error into something an author can act on.
//!
//! **`tokio_postgres::Error`'s `Display` is the literal string `"db error"`.**
//! Everything that identifies the failure — SQLSTATE, message, DETAIL, HINT,
//! POSITION — hangs off its `source()` as a [`DbError`], so `format!("{e}")`
//! throws all of it away and reports the same three characters for a missing
//! table, a permission denial, and a syntax error.
//!
//! That is not a theoretical loss. These strings are the entire diagnostic
//! surface for an Oxy Function author: they cannot read the server logs, and a
//! handler cannot branch on a cause it never receives. A `ctx.oltp` write that
//! failed `relation "bookings" does not exist` arrived as `ctx.tx: db error`,
//! which reads as an oxy fault rather than a missing migration.
//!
//! These two helpers live here rather than in `postgres.rs` because BOTH
//! Postgres paths need them and only one had them: the plain-query connector
//! extracted the detail from the start, while the transaction path
//! (`postgres_tx`, which backs `ctx.tx` and `ctx.oltp`) formatted with `{e}`
//! and lost it. One implementation, so the two cannot drift apart again.

use tokio_postgres::error::{DbError, ErrorPosition};

use crate::connector::QueryFailedDetails;

/// Flatten a driver error into one human-readable line.
///
/// For sites that don't carry the originating SQL — transport and TLS
/// failures, schema introspection. Query-execution sites should use
/// [`pg_query_failed`] instead so SQLSTATE / DETAIL / HINT / POSITION reach the
/// caller as separate fields rather than a pre-joined string.
pub(crate) fn pg_error_message(e: &tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => flatten(db),
        // Not a server error: a closed socket, a TLS handshake failure, a
        // connect timeout. `Display` names the phase, and the cause hangs off
        // `source()`, so both are joined (see [`with_sources`]).
        None => with_sources(e),
    }
}

/// `Display` of `e` followed by each `source()` in turn, joined with `: `.
///
/// `tokio_postgres::Error::connect` displays as the fixed text
/// `error connecting to server` and keeps the `io::Error` behind it
/// (`Network is unreachable (os error 101)`, `Connection refused (os error
/// 111)`) as its source, so `e.to_string()` alone says a connection failed
/// without saying why. A source whose text is already contained in what has
/// been written is skipped, so an error that embeds its own cause does not
/// print it twice.
fn with_sources(e: &(dyn std::error::Error + 'static)) -> String {
    let mut msg = e.to_string();
    let mut source = e.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        if !msg.contains(&text) {
            msg.push_str(": ");
            msg.push_str(&text);
        }
        source = cause.source();
    }
    msg
}

/// Build a structured [`QueryFailedDetails`] from a driver error.
///
/// Each server-side field lands in its own slot so the IDE can render a
/// structured block and highlight the offending token via `position`.
/// Transport-level errors populate only `message`.
pub(crate) fn pg_query_failed(
    sql: impl Into<String>,
    e: &tokio_postgres::Error,
) -> QueryFailedDetails {
    let sql = sql.into();
    let details = match e.as_db_error() {
        Some(db) => QueryFailedDetails {
            sql,
            message: db.message().to_string(),
            code: Some(db.code().code().to_string()),
            detail: db.detail().map(str::to_string),
            hint: db.hint().map(str::to_string),
            position: db.position().and_then(|p| match p {
                ErrorPosition::Original(n) => Some(*n),
                // An internal position points into a query the SERVER
                // generated (a function body), not the one the caller sent, so
                // highlighting that offset in the caller's SQL would point at
                // an unrelated token.
                ErrorPosition::Internal { .. } => None,
            }),
        },
        None => QueryFailedDetails {
            sql,
            message: e.to_string(),
            ..Default::default()
        },
    };
    details.redacted()
}

/// `[SQLSTATE] message — detail (hint: …)`, skipping the parts absent.
fn flatten(db: &DbError) -> String {
    let mut msg = format!("[{}] {}", db.code().code(), db.message());
    if let Some(detail) = db.detail() {
        msg.push_str(&format!(" — {detail}"));
    }
    if let Some(hint) = db.hint() {
        msg.push_str(&format!(" (hint: {hint})"));
    }
    msg
}
