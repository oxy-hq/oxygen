//! [`AgenticState`]: the agentic runtime state the host shares with every
//! agentic route and with the platform's own state (`oxy-app-core`'s
//! `AppState::agentic_state`), and the [`ThreadOwnerLookup`] port it holds.
//!
//! Every field is a runtime type, so it lives here rather than in the facade
//! (`agentic-pipeline`, where it was until 2026-11-27): the platform state can
//! hold it without depending on the facade or the domains it composes
//! (`internal-docs/crate-tiers.md`, stage B). The facade still builds it,
//! through `agentic_pipeline::state::new_agentic_state`, which supplies the
//! domains' event registry, and re-exports it at `agentic_pipeline::state`
//! and `agentic_http::state` under its old paths.

use std::ops::Deref;
use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use tokio_util::sync::CancellationToken;

use crate::lifecycle::event_registry::EventRegistry;
use crate::lifecycle::state::RuntimeState;
use crate::orchestrator::router::{NoopTaskRouter, TaskRouter};

/// Who owns a conversation thread, and which workspace it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadScope {
    /// `None` for a thread nobody owns — one auto-provisioned for a run
    /// started without a thread.
    pub owner: Option<uuid::Uuid>,
    pub workspace_id: uuid::Uuid,
}

/// Thread-ownership lookup for transport-layer auth checks.
///
/// Implemented by the host against its threads table. Pipeline + HTTP call
/// into this trait instead of importing a `threads` entity directly.
///
/// Returns `Ok(None)` when the thread does not exist; `Ok(Some(None))` when
/// the thread exists but has no owner; `Ok(Some(Some(id)))` when owned.
#[async_trait]
pub trait ThreadOwnerLookup: Send + Sync {
    async fn thread_owner(
        &self,
        thread_id: uuid::Uuid,
    ) -> Result<Option<Option<uuid::Uuid>>, String>;

    /// The thread's owner and workspace; `Ok(None)` when it does not exist.
    ///
    /// For callers that must also know the thread is in *their* workspace —
    /// a run started on a thread reads its history and writes into it.
    ///
    /// Default `Ok(None)`: an adapter that cannot say which workspace a
    /// thread is in cannot vouch for one, so its threads read as missing
    /// (fail closed). The host adapter (`OxyThreadOwnerLookup`) answers.
    async fn thread_scope(&self, _thread_id: uuid::Uuid) -> Result<Option<ThreadScope>, String> {
        Ok(None)
    }

    /// Create a new conversation thread in `project_id` (the run's
    /// workspace) titled `title`, returning the new thread id.
    ///
    /// Used by the run handler to auto-provision a thread when a client
    /// starts a run without one — so the run's `thread_id` FK is satisfied
    /// and the client can reuse the returned id for follow-up questions.
    ///
    /// Default impl returns `Err` so fakes that only implement ownership
    /// lookups (tests, embedded use) compile unchanged; the host adapter
    /// (`OxyThreadOwnerLookup`) inserts a real `threads` row.
    async fn create_thread(
        &self,
        _project_id: uuid::Uuid,
        _title: &str,
    ) -> Result<uuid::Uuid, String> {
        Err("thread creation not supported by this ThreadOwnerLookup".to_string())
    }
}

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
    /// this is a [`crate::orchestrator::router::PostgresTaskRouter`] sharing
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
    /// `event_registry` is the domains' registry, which the facade builds
    /// (`agentic_pipeline::build_event_registry`); hosts call
    /// `agentic_pipeline::state::new_agentic_state`, which passes it.
    pub fn new(
        shutdown_token: CancellationToken,
        db: DatabaseConnection,
        thread_owner: Arc<dyn ThreadOwnerLookup>,
        event_registry: EventRegistry,
    ) -> Self {
        Self {
            runtime: Arc::new(RuntimeState::new()),
            event_registry: Arc::new(event_registry),
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
