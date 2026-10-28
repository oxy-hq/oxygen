use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

use agentic_pipeline::platform::ThreadOwnerLookup;
use agentic_pipeline::{AnalyticsSchemaCatalog, BuilderTestRunnerTrait};
use agentic_runtime::event_registry::EventRegistry;
use agentic_runtime::router::{NoopTaskRouter, TaskRouter};
use sea_orm::DatabaseConnection;
use tokio_util::sync::CancellationToken;

pub use agentic_runtime::state::{RunError, RunStatus, RuntimeState};

/// Shared state for all agentic routes.
///
/// Wraps the transport-agnostic [`RuntimeState`] and adds domain-specific
/// extensions (schema cache, builder test runner, event registry). Holds the
/// shared SeaORM [`DatabaseConnection`] so handlers don't open a new one per
/// request, and a [`ThreadOwnerLookup`] so thread-ownership auth checks do
/// not reach into the platform `threads` table from this crate.
pub struct AgenticState {
    pub runtime: Arc<RuntimeState>,
    pub schema_cache: Arc<Mutex<HashMap<String, AnalyticsSchemaCatalog>>>,
    pub builder_test_runner: Option<Arc<dyn BuilderTestRunnerTrait>>,
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
    /// process. Asked on every start, so the host can turn it on and off at run
    /// time.
    ///
    /// **Off unless the host says otherwise**, because no driver reads which run
    /// kinds it can execute before it claims one: a driver built before the chat
    /// executor (`analytics_chat_run`) claims a queued chat and fails it as an
    /// unknown kind, and one built before the requester record runs a queued
    /// workflow on its own subject-less context. So the starts stay in the
    /// request's process until every driver is on a build that runs them.
    pub queued_starts: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Whether this process holds the workspace working copy (the Factory, or
    /// a single-process `OXY_ROLE=all` server). A Builder run edits workspace
    /// files and is driven in the memory of the process that started it, so a
    /// process without the files refuses to start one, or to resume one, on the
    /// customer routes and names the Builder routes instead
    /// ([`crate::routes::run::BUILDER_START_ROUTE`]). The role manifest pins
    /// those to the Factory.
    ///
    /// **True unless the host says otherwise**, which is what every process was
    /// assumed to be before the analytics routes left the Factory.
    pub holds_working_copy: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl AgenticState {
    pub fn new(
        shutdown_token: CancellationToken,
        db: DatabaseConnection,
        thread_owner: Arc<dyn ThreadOwnerLookup>,
    ) -> Self {
        Self {
            runtime: Arc::new(RuntimeState::new()),
            schema_cache: Arc::new(Mutex::new(HashMap::new())),
            builder_test_runner: None,
            event_registry: Arc::new(agentic_pipeline::build_event_registry()),
            shutdown_token,
            db,
            thread_owner,
            router: Arc::new(NoopTaskRouter),
            queued_starts: Arc::new(|| false),
            holds_working_copy: Arc::new(|| true),
        }
    }

    pub fn with_builder_test_runner(mut self, runner: Arc<dyn BuilderTestRunnerTrait>) -> Self {
        self.builder_test_runner = Some(runner);
        self
    }

    /// Replace the default no-op router with a real (typically Postgres)
    /// router. Production app boot calls this after constructing the
    /// router from env config.
    pub fn with_router(mut self, router: Arc<dyn TaskRouter>) -> Self {
        self.router = router;
        self
    }

    /// Say, per start, whether qualifying chat and workflow starts go on the
    /// queue ([`Self::queued_starts`]). Without this they never do.
    pub fn with_queued_starts(
        mut self,
        enabled: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        self.queued_starts = Arc::new(enabled);
        self
    }

    /// Say whether this process holds the workspace working copy
    /// ([`Self::holds_working_copy`]). Without this it is assumed to.
    pub fn with_working_copy(mut self, holds: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        self.holds_working_copy = Arc::new(holds);
        self
    }
}

impl Deref for AgenticState {
    type Target = RuntimeState;

    fn deref(&self) -> &RuntimeState {
        &self.runtime
    }
}
