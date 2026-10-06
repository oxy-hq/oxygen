# Domain FSM Architecture

## Overview

The domain layer is the **reasoning engine** of the agentic subsystem: a generic finite state machine (`agentic-core`) parameterized by domain-specific types (analytics, builder). It sits between the pipeline composition layer and the coordinator/worker runtime.

```
Pipeline Layer (PipelineBuilder)
    ↓ constructs
Orchestrator<D, S, Ev>  ←── Events ──→ EventStream → bridge → DB/SSE
    │
    ├─ Domain D: type registry (Intent, Spec, Solution, Result, Answer)
    ├─ Solver S: DomainSolver<D> (clarify, specify, solve, execute, interpret)
    ├─ StateHandlers: HashMap<state_name, StateHandler<D, S, Ev>>
    └─ ProblemState<D>: FSM position
         │
         ↓ suspension/completion
    PipelineOutcome → Coordinator (see COORDINATOR.md)
```

The framework is **domain-agnostic at the core**. Two concrete domains plug in today:
- **agentic-analytics**: Clarify → Specify → Solve → Execute → Interpret
- **agentic-builder**: Solve only (skips Clarifying, Specifying, Executing)

## Core Components

### Domain trait

Pure type-level registry. No method bodies — just associated types.

```rust
pub trait Domain: Sized + Send + Sync + 'static {
    type Intent:   Send + Sync + Clone + 'static;
    type Spec:     Send + Sync + Clone + 'static;
    type Solution: Send + 'static;
    type Result:   Send + 'static;
    type Answer:   Send + Sync + Clone + 'static;
    type Catalog:  Send + 'static;
    type Error:    Display + Send + 'static;
}
```

Defined in [core/src/domain.rs](core/src/domain.rs).

### ProblemState FSM

The orchestrator's position. Each variant **carries the input to that stage**, not the output.

| State | Carries | Next (on success) |
|-------|---------|-------------------|
| `Clarifying(Intent)` | raw user intent | Specifying |
| `Specifying(Intent)` | clarified intent | Solving (or fan-out) |
| `Solving(Spec)` | resolved spec | Executing |
| `Executing(Solution)` | executable solution | Interpreting |
| `Interpreting(Result)` | execution result | Done |
| `Diagnosing { error, back }` | failure + route | whatever `back` says |
| `Done(Answer)` | terminal answer | — |

Defined in [core/src/state.rs](core/src/state.rs).

### DomainSolver trait

Per-state methods. Each returns `Result<Output, (Error, BackTarget)>` so failures carry explicit recovery routing.

```rust
pub trait DomainSolver<D: Domain>: Send + Sync + 'static {
    async fn clarify(intent, ctx, memory)       -> Result<Intent,  (Error, BackTarget)>;
    async fn specify(intent, ctx, memory)       -> Result<Vec<Spec>, (Error, BackTarget)>;
    async fn specify_single(intent, ctx, mem)   -> Result<Spec,    (Error, BackTarget)>;
    async fn solve(spec, ctx, memory)           -> Result<Solution,(Error, BackTarget)>;
    async fn execute(solution, ctx, memory)     -> Result<Result,  (Error, BackTarget)>;
    async fn interpret(result, ctx, memory)     -> Result<Answer,  (Error, BackTarget)>;

    // Dynamic state routing
    fn should_skip(state, data, run_ctx) -> Option<ProblemState<D>>;
    fn diagnose(error, back, ctx)        -> Result<ProblemState<D>, Error>;

    // Tools (per-state)
    fn tools_for_state(state)     -> Vec<ToolDef>;
    fn execute_tool(state, name, params) -> Result<Value, ToolError>;

    // Fan-out
    fn merge_results(results)     -> Result<Result, Error>;
    fn fanout_worker()            -> Option<Arc<dyn FanoutWorker<D, Ev>>>;
    fn max_fanout_retries()       -> u32;  // default: 2

    // HITL
    fn store_suspension_data(data);
    fn take_suspension_data()     -> Option<SuspendedRunData>;

    // Static skip (compile-time)
    const SKIP_STATES: &[&str]    = &[];
}
```

Defined in [core/src/solver.rs](core/src/solver.rs).

### BackTarget

Explicit recovery route carried by every error. Not a free `retry` — the solver decides *where* to go back to.

| Variant | Carries | Routes to |
|---------|---------|-----------|
| `Clarify(Intent, RetryContext)` | rewound intent | Clarifying |
| `Specify(Intent, RetryContext)` | rewound intent | Specifying |
| `Solve(Spec, RetryContext)` | rewound spec | Solving |
| `Execute(Solution, RetryContext)` | rewound solution | Executing |
| `Interpret(Result, RetryContext)` | rewound result | Interpreting |
| `Suspend { reason }` | `HumanInput` or `Delegation` | suspends pipeline |

`RetryContext` holds `{ errors: Vec<Error>, attempt: u32, previous_output: Option<Value> }` so the LLM sees what failed and tries again with context.

Defined in [core/src/back_target.rs](core/src/back_target.rs).

### Orchestrator

The FSM driver. Generic over `<D, S, Ev>`:
- `D: Domain` — type registry
- `S: DomainSolver<D>` — concrete solver impl
- `Ev: DomainEvents` — domain event enum (see [STREAMING.md](STREAMING.md))

**Key fields** (in [core/src/orchestrator.rs](core/src/orchestrator.rs)):
- `solver: S`
- `handlers: HashMap<&'static str, StateHandler<D, S, Ev>>`
- `skip_states: HashSet<&'static str>`
- `max_iterations: usize` — guards runaway loops

**Main loop** (`run_pipeline_inner()`):

```
loop:
  state_name = current_stage(&state)
  if skip_states.contains(state_name) or solver.should_skip(state, &data):
    → advance via default next or Some(next_state) from should_skip
    continue

  emit StateEnter { state_name, revision, sub_spec_index }
  handler = handlers[state_name]
  transition = (handler.execute)(solver, state, events, ctx, memory).await

  match transition:
    ok(next_data)        → advance via handler.next
    ok_to(next_data, to) → advance via explicit `to`
    pending_fan_out(ss)  → run_fanout(specs) — see below
    diagnosing(diag)     → state = diag
    fail(data, errors)   → state = handler.diagnose(errors) → BackTarget

  emit StateExit { state_name, outcome, sub_spec_index }
  iterations += 1
  if iterations > max_iterations: return Err(MaxIterationsExceeded)
```

### StateHandler

Pluggable per-state logic. Domains register one handler per state they care about.

```rust
pub struct StateHandler<D, S, Ev> {
    pub next: &'static str,
    pub execute: Arc<dyn Fn(
        &mut S, ProblemState<D>,
        &Option<EventStream<Ev>>,
        &RunContext<D>, &SessionMemory<D>,
    ) -> BoxFuture<TransitionResult<D>> + Send + Sync>,
    pub diagnose: Option<Arc<dyn Fn(
        &[Error], u32, ProblemState<D>
    ) -> Option<ProblemState<D>> + Send + Sync>>,
}
```

Analytics registers 5 handlers (clarifying, specifying, solving, executing, interpreting) via `build_analytics_handlers()`. Builder registers just `solving` and `interpreting` (other states skip via `SKIP_STATES`).

### TransitionResult

What a handler returns. Controls FSM routing.

| Variant | Behavior |
|---------|----------|
| `ok(data)` | Advance to `handler.next` with `data` |
| `ok_to(data, stage)` | Advance to explicit `stage` |
| `diagnosing(state)` | Jump to pre-built `Diagnosing` |
| `fail(data, errors)` | Call `handler.diagnose(errors)` → `BackTarget` |
| `pending_fan_out(specs)` | Trigger scatter-gather on multiple specs |

### RunContext & SessionMemory

**`RunContext<D>`**: Orchestrator-owned, read-only to handlers.
```rust
pub struct RunContext<D: Domain> {
    pub intent: Option<D::Intent>,    // set after Clarifying
    pub spec: Option<D::Spec>,        // set after Specifying
    pub retry_ctx: Option<RetryContext>, // from most recent back-edge
}
```
Solvers **cannot** store intermediate state — the orchestrator owns it. This keeps replay, resumption, and fan-out simple.

**`SessionMemory<D>`**: Multi-turn conversation history.
- `Vec<CompletedTurn>` — prior question-answer exchanges
- Cap at `max_turns` (default 10), FIFO eviction
- Loaded from DB thread history, passed to solver methods for LLM prompt context

## Fan-Out (Scatter-Gather)

When `specify()` returns `Vec<Spec>` with more than one element, the orchestrator spawns concurrent sub-spec executions.

```
Specifying
    │
    ├─ solver.specify() → vec![spec_1, spec_2, spec_3]
    │
    ▼
emit FanOut { count: 3 }
    │
for (index, spec) in specs.enumerate():
    spawn task:
      emit SubSpecStart { index }
      result = solver.fanout_worker().solve_and_execute(spec, index, ...)
      emit SubSpecEnd { index, outcome }
      return result
    │
await all tasks
    │
results = vec![r_1, r_2, r_3]
    │
▼
solver.merge_results(results) → single D::Result
    │
▼
Interpreting(merged_result)
```

**Events tagged with `sub_spec_index`** so the frontend can render multiple cards in parallel. Per-spec retry: each worker can independently retry up to `max_fanout_retries()` (default 2).

`FanoutWorker` trait ([core/src/solver.rs:26](core/src/solver.rs#L26)) lets domains customize how a single spec runs (analytics uses this to run Solving + Executing together per spec).

## HITL Suspension

When a tool like `ask_user` fires, the solver stores suspension data and returns `BackTarget::Suspend`.

```
solver.execute_tool("ask_user", {question: "..."}):
  → emit AwaitingHumanInput { questions }
  → solver.store_suspension_data(data)
  → return Err((NeedsInput, Suspend { reason: HumanInput }))

orchestrator receives Err, sees BackTarget::Suspend:
  → PipelineOutcome::Suspended { reason, resume_data: solver.take_suspension_data() }
  → send on outcomes channel, exit pipeline

[Coordinator persists suspension, waits for HTTP answer]

resume_pipeline(run_id, answer, resume_data):
  → reconstruct ProblemState from resume_data
  → re-enter orchestrator.run() at the stored state
  → emit InputResolved
  → continue as if uninterrupted
```

Suspension data is persisted in `agentic_run_suspensions` (see [COORDINATOR.md](COORDINATOR.md)), not in the event stream — events are append-only, suspension data is mutable.

## Dynamic State Skipping

Two layers:

1. **Static** via `const SKIP_STATES: &[&str]`. Builder skips `clarifying`, `specifying`, `executing` entirely. Compile-time; cannot be overridden at runtime.

2. **Dynamic** via `solver.should_skip(state, data, run_ctx) -> Option<ProblemState<D>>`. Analytics uses this to skip `Solving` when the semantic model compiled SQL directly — returns `Some(Executing(solution))` to jump ahead.

## Domain Implementations

### Analytics ([analytics/](analytics/))

Five-stage pipeline: Clarifying → Specifying → Solving → Executing → Interpreting.

| Stage | Tools | Emits |
|-------|-------|-------|
| Clarifying | `search_catalog`, `search_automations`, `ask_user` | `SchemaResolved`, `TriageCompleted`, `IntentClarified` |
| Specifying | `get_valid_dimensions`, `get_column_range` | `SpecResolved`, `SemanticShortcutAttempted`/`Resolved` |
| Solving | `explain_plan`, `dry_run` | `QueryGenerated` |
| Executing | (connector I/O) | `QueryExecuted` |
| Interpreting | `render_chart` | `AnalysisComplete`, `ProposedChart` |

**Domain types** ([analytics/src/types.rs](analytics/src/types.rs)):
- `Intent = AnalyticsIntent { question, history }`
- `Spec = QuerySpec { resolved_metrics, resolved_tables, join_path, result_shape, solution_source }`
- `Solution = AnalyticsSolution { sql, source, explanation }`
- `Result = AnalyticsResult { rows, columns, metadata }`
- `Answer = AnalyticsAnswer { text, chart_config }`
- `Catalog = AnalyticsCatalog { connectors, semantic_layer }`

**Semantic shortcut**: `SemanticCatalog::compile()` in Specifying tries airlayer first. On success, `spec.solution_source = SemanticLayer` and `should_skip("solving", _)` returns `Some(Executing(solution))`. On `TooComplex`, falls through to Solving with `LlmWithSemanticContext`.

**Back-edge policy**:
- Executing failure on **semantic** path → Specifying (re-plan the query shape)
- Executing failure on **LLM** path → Solving (regenerate SQL)
- Interpreting failure → Solving (queries may have returned unusable shape)

**Extension table** `analytics_run_extensions`:
```sql
run_id TEXT PK FK,
agent_id TEXT NOT NULL,    -- which .agentic.yml
spec_hint JSONB,           -- prior turn's spec, for cross-turn continuity
thinking_mode TEXT,        -- NULL | "extended_thinking"
execution_started_at TIMESTAMPTZ,   -- stamped once, by the attempt that begins a queued run
execution_heartbeat_at TIMESTAMPTZ  -- that attempt's proof of life (extension/execution.rs)
```
Migrator: `AnalyticsMigrator` with tracking table `seaql_migrations_analytics`.

### Builder ([builder/](builder/))

Single-stage pipeline: Solving (tool loop, ≤30 rounds) → Interpreting → Done.

```rust
const SKIP_STATES: &[&str] = &["clarifying", "specifying", "executing"];
```

**Tools** ([builder/src/tools/](builder/src/tools/)):
| Tool | Purpose | HITL? |
|------|---------|-------|
| `search_files` | glob pattern search | No |
| `read_file` | read content (optional line range) | No |
| `search_text` | regex across project | No |
| `write_file` | create or fully overwrite a file | **Yes** |
| `edit_file` | exact-string replacement in an existing file | **Yes** |
| `delete_file` | delete a file | **Yes** |
| `manage_directory` | create / delete / rename a directory | **Yes** |
| `validate_project` | validate Oxy YAML | No |
| `lookup_reference` | load a domain reference card | No |
| `lookup_schema` | JSON schema for Oxy types | No |
| `run_tests` | execute .test.yml files | No |
| `run_app` | execute every task in a `.app.yml` end-to-end | No |
| `execute_sql` | run SQL against connectors | No |
| `semantic_query` | airlayer compile + execute | No |
| `ask_user` | generic clarification | **Yes** |

**HITL via the file-write tools**: `write_file`, `edit_file`, and `delete_file` all suspend under the shared `file_change` suspension kind. The solver emits `BuilderEvent::FileChangePending { file_path, description, new_content, old_content }` and returns `BackTarget::Suspend`. User accepts/rejects via HTTP; on resume the facade applies the change (writing `new_content` for `write_file` / `edit_file`, deleting for `delete_file`), emits `BuilderEvent::FileChanged`, and emits a synthetic `ToolResult` event so SSE replay shows the user's decision. When the LLM batches multiple writes in a single turn, the facade pre-computes each `edit_file`'s `new_content` at suspension time (stored in `stage_data["precomputed_edits"]`) to avoid TOCTOU races.

**No extension table** — builder stores only the generic `agentic_runs.metadata` JSONB.

## Extension Table Pattern

Each domain that needs per-run state owns its table, its entity, and its migrator. Central `agentic_runs` carries only `source_type: String` and `metadata: JSONB` for generic fields.

| Domain | Table | Migrator | Tracking |
|--------|-------|----------|----------|
| runtime (common) | `agentic_runs`, `agentic_run_events`, `agentic_run_suspensions`, `agentic_task_outcomes`, `agentic_task_queue` | `RuntimeMigrator` | `seaql_migrations_orchestrator` |
| analytics | `analytics_run_extensions` | `AnalyticsMigrator` | `seaql_migrations_analytics` |
| workflow | `agentic_workflow_state` | `WorkflowMigrator` | `seaql_migrations_workflow` |
| builder | (none) | — | — |

**Startup order**: central platform migrator → runtime → analytics → workflow. Each migrator is independent with no foreign-key linking beyond `run_id` references.

**Facade functions** (not SeaORM entities) are the public API:
- Analytics: `get_run_meta()`, `insert_run_meta()`, `update_run_spec_hint()`, `update_run_thinking_mode()`
- Workflow: `load_workflow_state()`, `save_workflow_state()` (see [COORDINATOR.md](COORDINATOR.md) for `WorkflowDecision` flow)

## Task Lifecycle Example (Analytics)

```
User: "Total revenue by region?"
    │
    ▼
Clarifying(intent { question, history })
    │ solver.clarify() — triage tool loop
    │ tools: search_catalog, search_automations
    │ emit IntentClarified { question_type: Breakdown, metrics, dimensions }
    ▼
Specifying(clarified_intent)
    │ solver.specify() — try semantic first
    │ emit SemanticShortcutAttempted { measures, dimensions }
    │ catalog.compile() → Ok(sql)
    │ emit SemanticShortcutResolved { sql }
    │ spec = QuerySpec { solution_source: SemanticLayer, sql }
    ▼
[should_skip("solving", _) → Some(Executing(solution))]
    │
    ▼
Executing(solution)
    │ solver.execute() — run SQL
    │ emit QueryExecuted { columns, rows, row_count, duration_ms }
    ▼
Interpreting(result)
    │ solver.interpret() — LLM synthesis
    │ emit AnalysisComplete { insight }
    │ emit ProposedChart { chart_type, title }
    ▼
Done(answer)
    │ emit Done { duration_ms }
    ▼
PipelineOutcome::Done → coordinator
```

## Key Design Decisions

1. **Domain is pure types.** The `Domain` trait has zero methods — just associated types. This forces a clean separation: *what* a domain handles (types) vs *how* it handles it (solver).

2. **Orchestrator owns context.** Handlers receive `&RunContext`, never `&mut`. Prior-stage outputs (intent, spec) are accumulated by the orchestrator, not the solver. Enables replay, resumption, fan-out without state-management bugs.

3. **Back-edges are explicit.** Every error carries a `BackTarget` specifying *where* to retry. No ambient "retry on failure" — the solver decides whether to re-plan (`Specify`), regenerate (`Solve`), or give up (`Suspend`).

4. **States carry inputs, not outputs.** `Solving(Spec)` holds the spec to solve, not the solution being built. Makes serialization trivial and prevents half-finished state from leaking into the next stage.

5. **Dynamic skipping via `should_skip`.** Lets domains optimize paths at runtime (analytics skips `Solving` when semantic model compiled SQL) without the FSM framework knowing about domain-specific paths.

6. **Pluggable state handlers.** A `HashMap<state_name, StateHandler>` instead of a giant match. Domains register only the states they implement; builder registers 2, analytics registers 5.

7. **Fan-out is first-class.** `specify()` returns `Vec<Spec>` — single-spec is just the `len()==1` case. `sub_spec_index` on every event tags which branch emitted it, so the frontend renders N cards.

8. **HITL is a suspension, not a callback.** The solver returns `BackTarget::Suspend`; the orchestrator exits. Resume happens via a fresh orchestrator call with the persisted `SuspendedRunData`. No in-process channels required.

9. **Extension tables, not metadata sprawl.** Each domain owns its table, migrator, and tracking row. Central `agentic_runs` stays minimal.

10. **Solver can never see another domain.** `agentic-analytics` and `agentic-builder` must never import each other. Composition happens exactly once — in `agentic-pipeline`.
