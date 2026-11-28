//! Port for resolving *managed* databases (`airhouse_managed`,
//! `postgres_managed`) to a ready-to-connect Postgres DSN.
//!
//! The resolution itself — minting an ephemeral airhouse credential through
//! the SA-backed broker, or looking up a per-org OLTP analyst connection —
//! needs the `airhouse` / `oxy-oltp` vendor crates, which are an
//! Integration-&-infra concern, not a platform-core one. Keeping those out of
//! `oxy` core means core must not call them directly. Instead it defines this
//! trait and reads an implementation that the application layer (`oxy-app`,
//! which owns those crates) registers once at startup — the same shape as
//! `oxy_observability::global`.
//!
//! Core still owns the *shape* of a managed connection: it wraps the resolved
//! DSN in a [`crate::connector`] `ConnectorX` (Postgres wire). The port
//! supplies only the DSN string — the one piece that needs the vendor crates.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use oxy_shared::errors::OxyError;
use uuid::Uuid;

/// Resolves a managed database to a ready-to-connect Postgres DSN.
///
/// Implemented by the application layer, which owns the `airhouse` /
/// `oxy-oltp` integrations; registered once via
/// [`set_managed_connection_resolver`]. The two [`DatabaseType`] managed arms
/// of `Connector::from_db` call through it.
///
/// [`DatabaseType`]: crate::config::model::DatabaseType
#[async_trait]
pub trait ManagedConnectionResolver: Send + Sync + std::fmt::Debug {
    /// Resolve an `airhouse_managed` database for `(workspace_id, subject)`,
    /// minting a fresh ephemeral per-user credential. `effective_role` selects
    /// the airhouse role the credential is minted at; `None` yields
    /// least-privilege Reader. Returns a Postgres-wire DSN.
    async fn resolve_airhouse_managed(
        &self,
        workspace_id: Uuid,
        subject: Uuid,
        effective_role: Option<entity::workspace_members::WorkspaceRole>,
    ) -> Result<String, OxyError>;

    /// Resolve a `postgres_managed` database to the per-org OLTP **read-only
    /// analyst** DSN for `workspace_id`. Writes never reach OLTP through this
    /// path — see the application-side implementation. Returns a Postgres DSN.
    async fn resolve_postgres_managed(&self, workspace_id: Uuid) -> Result<String, OxyError>;
}

static RESOLVER: OnceLock<Arc<dyn ManagedConnectionResolver>> = OnceLock::new();

/// Register the process-wide managed-connection resolver. The application
/// layer calls this once at startup. First registration wins; later calls are
/// ignored.
pub fn set_managed_connection_resolver(resolver: Arc<dyn ManagedConnectionResolver>) {
    if RESOLVER.set(resolver).is_err() {
        tracing::warn!("managed-connection resolver already registered; ignoring re-registration");
    }
}

/// The registered managed-connection resolver, or `None` when the application
/// layer wired none — e.g. CLI / system entry points that never open a managed
/// connection. The managed `from_db` arms turn `None` into a typed
/// `ConfigurationError`.
pub fn managed_connection_resolver() -> Option<&'static Arc<dyn ManagedConnectionResolver>> {
    RESOLVER.get()
}
