//! Application-side implementation of core's
//! [`oxy::connector::ManagedConnectionResolver`] port.
//!
//! Core (`oxy`) resolves `airhouse_managed` / `postgres_managed` databases
//! through this port so the platform base need not depend on the `airhouse` /
//! `oxy-oltp` vendor crates. This module — in oxy-app, which owns those
//! integrations — does the mint / analyst lookup and returns a ready
//! Postgres-wire DSN; core wraps it in a `ConnectorX`.
//!
//! Registered once at startup via [`install`], called from `oxy_app::cli::prepare` before any
//! subcommand runs, so every entry point that may open a managed connection
//! has it.

use std::sync::Arc;

use async_trait::async_trait;
use oxy::connector::ManagedConnectionResolver;
use oxy_shared::errors::OxyError;
use uuid::Uuid;

#[derive(Debug)]
struct AppManagedConnectionResolver;

#[async_trait]
impl ManagedConnectionResolver for AppManagedConnectionResolver {
    async fn resolve_airhouse_managed(
        &self,
        workspace_id: Uuid,
        subject: Uuid,
        effective_role: Option<entity::workspace_members::WorkspaceRole>,
    ) -> Result<String, OxyError> {
        // `effective_role` picks the airhouse role the credential is minted at
        // (Owner→Admin, Admin→Writer, Member/Viewer→Reader); `None` is
        // least-privilege Reader, so write-capable Automation steps must thread
        // one through. Reader denies DDL/DML at the database.
        let airhouse_role = effective_role
            .map(airhouse::airhouse_role_for)
            .unwrap_or(airhouse::UserRole::Reader);
        let endpoint = airhouse::wire_endpoint().ok_or_else(|| {
            OxyError::ConfigurationError(
                "airhouse_managed: AIRHOUSE_WIRE_HOST is not configured; the airhouse \
                 integration must be enabled (set AIRHOUSE_BASE_URL, AIRHOUSE_ADMIN_TOKEN, \
                 AIRHOUSE_WIRE_HOST, AIRHOUSE_WIRE_PORT)"
                    .into(),
            )
        })?;
        let broker = airhouse::token_broker().ok_or_else(|| {
            OxyError::ConfigurationError(
                "airhouse_managed: token broker not initialised; airhouse env vars \
                 (AIRHOUSE_BASE_URL / AIRHOUSE_ADMIN_TOKEN / AIRHOUSE_WIRE_HOST) are required"
                    .into(),
            )
        })?;
        let cred = broker
            .mint_for_user(
                workspace_id,
                subject,
                airhouse_role,
                airhouse::DEFAULT_INTERNAL_TTL,
            )
            .await
            .map_err(OxyError::from)?;
        // Airhouse speaks Postgres wire; force `cxprotocol=cursor` so ConnectorX
        // issues DECLARE CURSOR + FETCH rather than `COPY ... TO STDOUT`, which
        // the airhouse wire impl rejects.
        Ok(format!(
            "{}:{}@{}:{}/{}?cxprotocol=cursor",
            cred.username, cred.password, endpoint.host, endpoint.port, cred.tenant,
        ))
    }

    async fn resolve_postgres_managed(&self, workspace_id: Uuid) -> Result<String, OxyError> {
        let db = oxy::database::client::establish_connection()
            .await
            .map_err(|e| OxyError::RuntimeError(format!("postgres_managed: {e}")))?;
        let conn = oxy_oltp::resolver::resolve_analyst_connection(&db, workspace_id)
            .await
            .map_err(|e| OxyError::ConfigurationError(format!("postgres_managed: {e}")))?;
        // The generated password always contains `#` (see
        // `oxy_oltp::roles::generate_password`), which starts a URI fragment, so
        // it must be percent-encoded or the DSN cannot authenticate at all.
        //
        // sslmode is carried from the tenant's provider and deliberately NOT
        // upgraded to verify-full: this path runs through connectorx 0.4.5,
        // whose `rewrite_tls_args` panics on VerifyCa/VerifyFull, and `require`
        // verifies the peer only when an `sslrootcert` FILE is supplied. So
        // `require` here ENCRYPTS but does not authenticate the peer — an
        // accepted gap; the agentic `PostgresConnector` path does verify.
        Ok(format!(
            "{}:{}@{}:{}/{}?sslmode={}",
            conn.user,
            oxy_oltp::roles::encode_userinfo(&conn.password),
            conn.host,
            conn.port,
            conn.database,
            conn.sslmode,
        ))
    }
}

/// Register the application's managed-connection resolver with oxy core. Called
/// once from `oxy_app::cli::prepare` at startup, before any subcommand can build a managed
/// connector. Idempotent (first registration wins).
pub fn install() {
    oxy::connector::set_managed_connection_resolver(Arc::new(AppManagedConnectionResolver));
}
