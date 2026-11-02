//! `POST /admin/orgs/{org_id}/oltp/test-connection` — open one session to an
//! org's OLTP database as its read-only analyst, run `SELECT 1`, and say what
//! the server or the network answered.
//!
//! **Why it exists.** The console's OLTP panel reads only Oxy's own
//! control-plane rows ([`super::handlers::status_for_org`]) and never the
//! tenant database, so a scale-to-zero compute stays asleep. That also means
//! it read `active` all through 2026-10-21, while Neon refused every
//! connection to Poke House Staging's project for its quota. Every app on the
//! org failed with `connect failed: error connecting to server`, and the cause
//! surfaced only when someone opened `psql` with an app's own credential. This
//! is that session, one click from the panel.
//!
//! **Only on a click.** It opens a real connection, so it wakes a suspended
//! compute; nothing calls it on a page load or a poll.
//!
//! **As the analyst, never a writer.** The analyst is the one role the console
//! may already hand out ([`crate::resolver::resolve_analyst_connection_for_org`])
//! and the only one that cannot write. The question is "does the database
//! answer", and any role reaching it answers that.
//!
//! **What it reports is the server's text.** [`crate::connect::connect`] keeps
//! the first address's refusal rather than the last address's io error, and
//! [`crate::connect::pg_detail`] renders it with its SQLSTATE, so a quota
//! refusal reads as one here and not as networking. Credentials never appear:
//! neither the driver's errors nor `pg_detail` carry the DSN.

use std::time::{Duration, Instant};

use axum::extract::{Json, Path};
use axum::http::StatusCode;
use serde::Serialize;
use tracing::{error, instrument};
use uuid::Uuid;

use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_platform::db::establish_connection;

use crate::connect::{connect, pg_detail};
use crate::resolver::{ResolveError, resolve_analyst_connection_for_org};

/// How long the whole attempt may take. A Neon compute waking from suspend
/// answers in a few seconds; past this the answer is the timeout itself.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// What one attempt found. `ok` and `stage` are the verdict; the rest is what
/// an operator needs to act on it.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ConnectionTestResponse {
    /// A session opened and `SELECT 1` answered.
    pub ok: bool,
    /// Where it stopped, when it did: `resolve` (Oxy could not build the
    /// connection — no database, not active, no analyst credential),
    /// `connect` (the server or the network refused the session), `query`, or
    /// `timeout`. `None` when `ok`.
    pub stage: Option<&'static str>,
    /// What stopped it, as the server, the network or the resolver said it.
    pub error: Option<String>,
    /// The role it connected as — always the read-only analyst. `None` when
    /// no connection could be built.
    pub role: Option<String>,
    /// The host it dialled, as the panel already shows it.
    pub host: Option<String>,
    pub elapsed_ms: u64,
}

/// Open one analyst session to `org_id`'s database and report the outcome.
///
/// A database that refuses is a `200` carrying `ok: false` — the probe
/// worked, and its answer is the refusal. Only a fault reading Oxy's own rows
/// is an error status.
#[instrument(skip(user), fields(user_id = %user.id, org_id = %org_id))]
pub async fn test_connection(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
) -> Result<Json<ConnectionTestResponse>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        error!("oltp probe: connect to the control plane: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let started = Instant::now();
    let conn = match resolve_analyst_connection_for_org(&db, org_id).await {
        Ok(conn) => conn,
        Err(e @ (ResolveError::Db(_) | ResolveError::Crypto(_))) => {
            error!("oltp probe: resolve the analyst connection: {e}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        Err(e) => {
            return Ok(Json(report(
                Err(("resolve", e.to_string())),
                None,
                started.elapsed(),
            )));
        }
    };
    let outcome = match tokio::time::timeout(PROBE_TIMEOUT, probe(&conn.dsn())).await {
        Ok(outcome) => outcome,
        Err(_) => Err((
            "timeout",
            format!("no answer within {}s", PROBE_TIMEOUT.as_secs()),
        )),
    };
    Ok(Json(report(
        outcome,
        Some((conn.user, conn.host)),
        started.elapsed(),
    )))
}

/// One session, one `SELECT 1`. The error names the stage that failed.
async fn probe(dsn: &str) -> Result<(), (&'static str, String)> {
    let client = connect(dsn, "oltp probe")
        .await
        .map_err(|e| ("connect", pg_detail(&e)))?;
    client
        .simple_query("SELECT 1")
        .await
        .map_err(|e| ("query", pg_detail(&e)))?;
    Ok(())
}

/// The response for an outcome. `target` is the `(role, host)` dialled, when a
/// connection could be built at all.
fn report(
    outcome: Result<(), (&'static str, String)>,
    target: Option<(String, String)>,
    elapsed: Duration,
) -> ConnectionTestResponse {
    let (role, host) = target.unzip();
    let (ok, stage, error) = match outcome {
        Ok(()) => (true, None, None),
        Err((stage, error)) => (false, Some(stage), Some(error)),
    };
    ConnectionTestResponse {
        ok,
        stage,
        error,
        role,
        host,
        elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::tests::{QUOTA, fake_server};

    /// The 2026-10-21 refusal, as the panel would now show it: the server's
    /// own words at the `connect` stage, not a networking fault.
    #[tokio::test]
    async fn a_server_that_refuses_the_session_is_reported_in_its_own_words() {
        let port = fake_server(usize::MAX).await;
        let (stage, error) = probe(&format!("postgres://u@127.0.0.1:{port}/d?sslmode=disable"))
            .await
            .expect_err("the server refuses every session");
        assert_eq!(stage, "connect");
        assert!(error.contains(QUOTA), "{error}");
        assert!(error.starts_with("[XX000]"), "SQLSTATE first: {error}");
    }

    #[tokio::test]
    async fn nothing_listening_is_reported_with_its_os_error() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let (stage, error) = probe(&format!("postgres://u@127.0.0.1:{port}/d?sslmode=disable"))
            .await
            .expect_err("nothing listens there");
        assert_eq!(stage, "connect");
        assert!(
            error.starts_with("error connecting to server") && error.contains("(os error"),
            "{error}"
        );
    }

    #[test]
    fn a_report_carries_the_stage_and_the_target_it_had() {
        let ok = report(
            Ok(()),
            Some(("oxy_analyst_ro".into(), "ep-x.neon.tech".into())),
            Duration::from_millis(312),
        );
        assert_eq!(
            ok,
            ConnectionTestResponse {
                ok: true,
                stage: None,
                error: None,
                role: Some("oxy_analyst_ro".into()),
                host: Some("ep-x.neon.tech".into()),
                elapsed_ms: 312,
            }
        );
        let unresolved = report(
            Err(("resolve", "org has no OLTP database yet".into())),
            None,
            Duration::ZERO,
        );
        assert!(!unresolved.ok);
        assert_eq!(unresolved.stage, Some("resolve"));
        assert_eq!(unresolved.role, None, "no connection was built to name one");
    }
}
