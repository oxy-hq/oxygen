//! Which side of the wire failed: the connection, or the statement.
//!
//! A `DbErr` reaching an HTTP handler becomes a 500 either way, and the two
//! 500s want opposite people. `database_unreachable` is infrastructure —
//! resolve the host, look at the server, read its log. `database_query_failed`
//! is ours — read the SQL. Collapsing them into one body costs an afternoon:
//! a local box served `{"message":"workspace lookup failed"}` on ~40% of
//! authenticated requests because `localhost` resolved to `::1` first and the
//! IPv6 loopback forward reset every new connection during the Postgres
//! startup handshake. Warm connections kept working, so it read as a hard
//! three-request ceiling that decayed over time — and the response said
//! nothing that would have pointed at DNS.

use sea_orm::{DbErr, RuntimeErr};

/// Which side of the wire failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbFailure {
    /// We never reached a usable connection, so the statement never ran.
    Unreachable,
    /// The server answered; the statement was the problem.
    Query,
}

impl DbFailure {
    /// The stable machine-readable code for this failure. Goes on the wire as
    /// `code` in an error body, so changing one is a contract change.
    pub const fn code(self) -> &'static str {
        match self {
            DbFailure::Unreachable => "database_unreachable",
            DbFailure::Query => "database_query_failed",
        }
    }

    /// Classify a `sea_orm` error by which side of the wire failed.
    ///
    /// Matches on variants rather than on message text: the wording belongs to
    /// sea-orm, sqlx and the OS, all three of which are free to change it, and
    /// one of them is localised. `crates/platform/src/db/client.rs` keeps its
    /// own string fallback for the startup-retry decision, which is a different
    /// question — "try again in a moment?" rather than "whose problem is this?"
    /// — and can afford to be generous where this cannot.
    pub fn classify(err: &DbErr) -> Self {
        match err {
            // The pool gave up handing one over: timed out waiting, or closed
            // under us. Both mean no statement was ever sent.
            DbErr::ConnectionAcquire(_) => DbFailure::Unreachable,
            // sea-orm raises `Conn` for everything else `pool.acquire()`
            // returned, which is where a handshake the peer reset lands —
            // sqlx bubbles anything that is not `ConnectionRefused` straight
            // out of the acquire rather than retrying it to the deadline.
            DbErr::Conn(_) => DbFailure::Unreachable,
            // A statement got as far as being sent. Usually ours — but the
            // connection can also die mid-flight, and that is not.
            DbErr::Exec(rt) | DbErr::Query(rt) => classify_runtime(rt),
            // Conversions, missing rows, arity, RBAC: the server answered.
            _ => DbFailure::Query,
        }
    }
}

/// Split an `Exec`/`Query` failure by whether the connection or the statement
/// was at fault.
fn classify_runtime(rt: &RuntimeErr) -> DbFailure {
    let RuntimeErr::SqlxError(err) = rt else {
        // `Internal` is sea-orm talking about a statement it built.
        return DbFailure::Query;
    };
    match &**err {
        // Nothing here is about the SQL: the socket, the TLS handshake, a
        // desynced protocol, a pool that timed out or closed, or a connection
        // string we could not even parse.
        sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::Protocol(_)
        | sqlx::Error::Configuration(_)
        | sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::WorkerCrashed => DbFailure::Unreachable,
        // The server answered, but with one of the two SQLSTATEs that mean
        // "not now" rather than "not that query" — `53300 too_many_connections`
        // and `57P03 cannot_connect_now`. sqlx already knows which those are;
        // asking it beats keeping a second list of codes in sync.
        // `is_transient_in_connect_phase` is `#[doc(hidden)]` (checked against
        // sqlx-core 0.9.0), so a sqlx bump can drop it with no semver signal —
        // re-check it on every bump.
        sqlx::Error::Database(db) if db.is_transient_in_connect_phase() => DbFailure::Unreachable,
        _ => DbFailure::Query,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ConnAcquireErr, RuntimeErr};
    use std::io;

    fn sqlx_conn(err: sqlx::Error) -> DbErr {
        DbErr::Conn(RuntimeErr::SqlxError(err.into()))
    }

    fn sqlx_query(err: sqlx::Error) -> DbErr {
        DbErr::Query(RuntimeErr::SqlxError(err.into()))
    }

    /// The 2026-09 local incident, exactly: `localhost` resolved to `::1`, the
    /// IPv6 loopback forward accepted the TCP connection and then reset it
    /// during the Postgres startup handshake. sqlx bubbles anything that is not
    /// `ConnectionRefused` straight out of the acquire, so the gate sees this.
    #[test]
    fn a_reset_during_the_startup_handshake_is_unreachable() {
        let err = sqlx_conn(sqlx::Error::Io(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "Connection reset by peer (os error 54)",
        )));
        assert_eq!(DbFailure::classify(&err), DbFailure::Unreachable);
    }

    #[test]
    fn a_pool_acquire_timeout_is_unreachable() {
        let err = DbErr::ConnectionAcquire(ConnAcquireErr::Timeout);
        assert_eq!(DbFailure::classify(&err), DbFailure::Unreachable);
    }

    #[test]
    fn a_statement_the_server_rejected_is_a_query_failure() {
        let err = sqlx_query(sqlx::Error::ColumnNotFound("org_id".to_string()));
        assert_eq!(DbFailure::classify(&err), DbFailure::Query);
    }

    /// The whole point of the type: an operator reading a 500 body can tell the
    /// two apart without turning on debug logging.
    #[test]
    fn the_two_failures_carry_different_codes() {
        assert_ne!(
            DbFailure::Unreachable.code(),
            DbFailure::Query.code(),
            "a connect failure and a query failure must not answer the same code"
        );
    }
}
