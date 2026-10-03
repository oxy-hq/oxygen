# agentic-automation

Sibling domain alongside `agentic-analytics` and `agentic-builder`. Owns two surfaces:

1. The **stateless decision runner** — Temporal-style: a single `WorkflowDecider::decide`
   call computes the next action from durable state, and `commit_decision` atomically
   persists the state patch, emitted events, and any terminal queue/run transition.
   No long-lived in-memory channels survive a decision boundary, so crashes resume
   from the DB cleanly.

2. The **subrun search adapter** (`OxyAutomationRunner`; `OxyProcedureRunner`
   is a back-compat `pub use` alias) — concrete impl of
   `agentic_core::subrun::SubrunRunner` used by the analytics domain to
   discover `.procedure.yml` / `.automation.yml` / `.sql` files that match a
   user question. Execution itself goes through the coordinator/worker
   path, not the trait.

The cross-domain contract (`SubrunRunner`, `SubrunStep`, `SubrunRef`,
`OxyCommentBlock`, `parse_oxy_comment_block`) lives in
`agentic_core::subrun`. `agentic-pipeline` is the only crate that
wires this concrete impl into other domains — workflow itself doesn't
know which domains consume it.

## Key Types

### Stateless runner (decision-based)

```rust
// Pure decision function — no DB, no I/O.
pub struct WorkflowDecider { /* … */ }
impl WorkflowDecider {
    pub async fn decide(
        &self,
        state: WorkflowRunState,
        pending_child_answer: Option<ChildCompletion>,
        prior_state: Option<&WorkflowRunState>,
    ) -> (WorkflowRunState, WorkflowDecision);
}

// Durable state row persisted in `agentic_workflow_state`.
pub struct WorkflowRunState {
    pub run_id: String,
    pub workflow: WorkflowConfig,
    pub results: HashMap<String, Value>,
    pub step_hashes: HashMap<String, String>,
    pub prior_step_hashes: HashMap<String, String>,  // pre-materialised cache
    pub prior_results: HashMap<String, Value>,
    pub decision_version: i64,                       // optimistic concurrency
    // …
}

// Atomic state-patch + event-emit + queue-transition.
pub async fn commit_decision(db: &DatabaseConnection, commit: DecisionCommit)
    -> Result<CommitOutcome, DbErr>;
```

### Subrun search adapter

```rust
pub struct OxyAutomationRunner { /* … */ }
impl agentic_core::subrun::SubrunRunner for OxyAutomationRunner {
    async fn search(&self, query: &str) -> Vec<SubrunRef>;
}
```

Subrun lifecycle events are emitted directly by `WorkflowStepOrchestrator`
on the coordinator event channel:

- `subrun_started` — the full step DAG before execution
- `subrun_step_started` / `subrun_step_completed` — per-task progress
- `subrun_step_cache_hit` — step result reused from prior run
- `subrun_completed` — final success/failure

`WorkflowEventBridge` is retained as an empty placeholder for back-compat
with existing imports.

## Host injection

The crate is host-agnostic. Callers supply a `WorkspaceContext` impl
(`resolve_workflow_yaml`, `get_connector`, `get_integration`, …) so the
runner stays decoupled from `oxy::*` types. For Oxy that impl lives in
`app::agentic_wiring::OxyProjectContext`.

Second port: `AirwayAdmissionResolver` (`src/airway_admission.rs`). An
`airway` step queues the same `TaskSpec::Airway` a schedule does, so it must
carry the same `airway_source_config` admission — but resolving it needs a
`DatabaseConnection`, the run's `workspace_id` (which appears nowhere in this
crate), and the pipeline's `source_kind` (which lives in the referenced
`.airway.yml`, not the step config). The facade supplies all three via
`agentic_pipeline::airway_config::PipelineAirwayAdmissionResolver`, injected
with `AutomationDecider::with_airway_admission_resolver`. Both dispatch sites
(`step_decider.rs`, `step_orchestrator/mod.rs`) resolve **at dispatch**, where
the spec is built and before the coordinator writes the child's queue row —
though only `step_decider.rs` is on a production path; `AutomationStepOrchestrator`
is the actor the decider replaced and nothing constructs it outside its own
tests, so its injection point is parity for a future reviver, documented as such.
No resolver injected → airway's `permissive` / `production` default; a
resolver that errors **fails the step** rather than defaulting.

`Err` from the port means *determinate, or transient past all patience*: the
resolver owns the retry, because this crate has no error type richer than
`String` and no queue-level retry to defer to (the root automation task is
enqueued with `policy: None`, so `Coordinator::check_retry_or_fallback` never
fires, and `commit_decision` writes the terminal failure transactionally before
the coordinator sees an outcome). The facade impl classifies determinate
failures (unresolvable ref, unparseable YAML, malformed row) from transient ones
(reset connection, exhausted pool) and re-attempts only the latter, inside a
budget far smaller than the decision task's 60s queue lease.

## Extension table

`agentic_workflow_state` (managed by `WorkflowMigrator`, tracking table
`seaql_migrations_workflow`) — per-run Temporal-style state including the
pre-materialised prior-cache snapshot.

## Rules

- Depends on `agentic-core`, `agentic-runtime`, `agentic-connector`, `oxy-looker`.
  **Does NOT depend on** `agentic-analytics`, `agentic-builder`,
  `agentic-pipeline`, `agentic-http`, or `oxy`. No domain depends on
  another domain — cross-domain wiring lives in `agentic-pipeline`.
- **SQL is rendered with `render_sql_string(.., workspace.string_literal(database))`, never
  `render_jinja_string`.** The `sqlquote` filter escapes by the rule of the engine the SQL is
  sent to, and the host names that rule per database (`WorkspaceContext::string_literal`) without
  building a connector. `render_jinja_string` is for text no database reads (paths, prompts, HTTP
  parts): there `sqlquote` refuses a value holding a quote or a backslash, because no spelling of
  one is read the same on every engine. A new step type that sends SQL takes the first.
- Migrator is independent (`seaql_migrations_workflow`) so the workflow schema
  evolves without coordinating with runtime/analytics migrations.
- Used by `agentic-pipeline` directly (via `WorkflowDecider` +
  `commit_decision`) and indirectly by analytics (pipeline injects
  `OxyAutomationRunner` into `AnalyticsSolver` via `with_subrun_runner`).
