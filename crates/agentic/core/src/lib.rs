//! # agentic-core
//!
//! Generic framework for agentic problem-solving pipelines.
//!
//! ## Pipeline stages
//!
//! ```text
//!  Intent
//!    │
//!    ▼
//! Clarifying ──► Specifying ──► Solving ──► Executing ──► Interpreting ──► Done
//!    ▲               ▲             ▲             ▲               ▲
//!    └───────────────┴─────────────┴─────────────┴───────────────┘
//!                        Diagnosing (back-edges)
//! ```
//!
//! Implement [`Domain`] to declare your associated types, [`HasIntent`] on
//! your `Spec` type to enable intent recovery on back-edges, and
//! [`DomainSolver`] to provide the async logic for each stage.  Then wrap
//! everything in an [`Orchestrator`] and call [`Orchestrator::run`].

pub mod back_target;
pub mod delegation;
pub mod domain;
pub mod evaluator;
pub mod events;
pub mod hub_task;
pub mod human_input;
pub mod orchestrator;
pub mod result;
pub mod solver;
pub mod state;
pub mod subrun;
pub mod tools;
pub mod transport;
pub mod ui_stream;

pub use evaluator::{ConsistencyEvaluator, EvalError, EvalResult};

#[cfg(feature = "storage")]
pub mod app_storage;
#[cfg(feature = "storage")]
pub mod storage;

pub use back_target::{BackTarget, RetryContext};
pub use delegation::{
    BackoffStrategy, DelegationItem, DelegationTarget, FanoutFailurePolicy, ResolvedAdmission,
    RetryPolicy, SuspendReason, TaskAssignment, TaskOutcome, TaskPolicy, TaskSpec,
};
pub use domain::Domain;
pub use events::{CoreEvent, DomainEvents, Event, EventStream, HumanInputQuestion, Outcome};
pub use human_input::{
    AutoAcceptProvider, DeferredInputProvider, HumanInputHandle, HumanInputProvider, ResumeInput,
    SuspendedRunData,
};
pub use orchestrator::{
    CompletedTurn, Orchestrator, OrchestratorError, PipelineOutput, RunContext, SessionMemory,
    StateHandler, TransitionResult, build_default_handlers, child_trace_id, next_trace_id,
    run_fanout,
};
pub use result::{CellValue, QueryResult, QueryRow};
pub use solver::{DomainSolver, FanoutWorker};
pub use state::ProblemState;
pub use subrun::{
    OxyCommentBlock, SubrunError, SubrunOutput, SubrunRef, SubrunRunner, SubrunStep,
    SubrunStepResult, parse_oxy_comment_block,
};
pub use tools::{ToolDef, ToolError};
pub use transport::{CoordinatorTransport, TransportError, WorkerMessage, WorkerTransport};
pub use ui_stream::{UiBlock, UiTransformState, serialize_ui_block};

#[cfg(feature = "storage")]
pub use app_storage::{
    Artifact, PersistedTurn, PreferenceStore, QueryLog, QueryLogEntry, SessionSummary,
    StorageError, StorageHandle, SuspendedPipeline, SuspendedPipelineStore, TurnStore,
    truncate_artifact_content,
};
#[cfg(feature = "storage")]
pub use storage::{InMemoryStorage, JsonFileStorage};
