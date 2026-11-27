//! [`AgenticState`]: the agentic runtime state the host shares with every
//! agentic route and with the platform's own state (`oxy-app-core`'s
//! `AppState::agentic_state`).
//!
//! It lives here, in the facade, rather than in `agentic-http` beside the
//! routes that read it, because every field is a `agentic-pipeline` or
//! `agentic-runtime` type: holding it must not pull the HTTP transport into
//! the graph (`internal-docs/crate-tiers.md`, stage A). `agentic-http`
//! re-exports it at `agentic_http::state`, so the routes and the host name it
//! there as before.

use std::ops::Deref;
use std::sync::Arc;

use agentic_runtime::event_registry::EventRegistry;
use agentic_runtime::router::{NoopTaskRouter, TaskRouter};
use agentic_runtime::state::RuntimeState;
use sea_orm::DatabaseConnection;
use tokio_util::sync::CancellationToken;

use crate::platform::ThreadOwnerLookup;

/// Shared state for all agentic routes.
///
/// Wraps the transport-agnostic [`RuntimeState`] and adds domain-specific
/// extensions (the event registry). Holds the
/// shared SeaORM [`DatabaseConnection`] so handlers don't open a new one per
/// request, and a [`ThreadOwnerLookup`] so thread-ownership auth checks do
/// not reach into the platform `threads` table from this crate.
pub struct AgenticState {
    pub runtime: Arc<RuntimeState>,
    pub event_registry: Arc<EventRegistry>,
    pub shutdown_token: CancellationToken,
    pub db: DatabaseConnection,
    pub thread_owner: Arc<dyn ThreadOwnerLookup>,
    /// Cross-process wake source for worker claim loops. In production
    /// this is a [`agentic_runtime::router::PostgresTaskRouter`] sharing
    /// one LISTEN connection across all runs on this instance; in tests
    /// or when the listener can't be wired (e.g. IAM auth mode without
    /// credential refresh), this falls back to [`NoopTaskRouter`] and
    /// workers rely solely on the 10s backstop poll.
    pub router: Arc<dyn TaskRouter>,
    /// Whether a qualifying chat start (`queued_run::ChatAdmission`) and a
    /// requester's agentic-workflow start (`queued_run::AutomationAdmission`) go
    /// on the Global queue for any driver, rather than being driven in this
    /// process. A property of the host: set it only where the deployment runs
    /// drivers that claim queued runs (`oxy serve` and `oxy worker` do). Off by
    /// default, so a host with no driver (a test, a router built without the
    /// runtime) never queues a start nothing would pick up. Starts that do not
    /// qualify are driven here either way.
    pub queued_starts: bool,
}

impl AgenticState {
    pub fn new(
        shutdown_token: CancellationToken,
        db: DatabaseConnection,
        thread_owner: Arc<dyn ThreadOwnerLookup>,
    ) -> Self {
        Self {
            runtime: Arc::new(RuntimeState::new()),
            event_registry: Arc::new(crate::build_event_registry()),
            shutdown_token,
            db,
            thread_owner,
            router: Arc::new(NoopTaskRouter),
            queued_starts: false,
        }
    }

    /// Replace the default no-op router with a real (typically Postgres)
    /// router. Production app boot calls this after constructing the
    /// router from env config.
    pub fn with_router(mut self, router: Arc<dyn TaskRouter>) -> Self {
        self.router = router;
        self
    }

    /// Put qualifying chat and workflow starts on the queue
    /// ([`Self::queued_starts`]). Without this they are driven in the process
    /// that took the request.
    pub fn with_queued_starts(mut self, queued: bool) -> Self {
        self.queued_starts = queued;
        self
    }
}

impl Deref for AgenticState {
    type Target = RuntimeState;

    fn deref(&self) -> &RuntimeState {
        &self.runtime
    }
}
