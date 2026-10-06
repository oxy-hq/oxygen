# Coordinator-Worker Architecture

## Overview

The coordinator-worker architecture manages multi-agent task execution: delegation between agents, workflow automation runs, human-in-the-loop (HITL), retry/fallback policies, and crash recovery. It sits between the pipeline layer (domain logic) and the transport layer (message delivery).

```
HTTP/CLI
    ↓
Pipeline Layer (PipelineBuilder, PipelineTaskExecutor)
    ↓
Coordinator ←── Transport ──→ Worker ──→ TaskExecutor
(task tree)     (durable)    (generic)   (domain-aware)
    ↓
DB (agentic_runs, events, suspensions, task_outcomes, task_queue)
    ↓
SSE → Frontend
```

## Core Components

### Coordinator

Manages an in-memory task tree. Receives `WorkerMessage`s from transport, routes outcomes, spawns child tasks, enforces timeouts.

**Key responsibilities:**
- Task tree management (parent-child relationships)
- Delegation: spawns child tasks when a pipeline suspends
- Fan-out: `ParallelDelegation` spawns N children, collects results
- Resume: sends answer back to suspended parent (via answer channel or `TaskSpec::Resume`)
- Retry/fallback: retries failed children with backoff, falls back to alternative targets
- Persistence: writes `task_status` + `task_metadata` on every transition via `persist_task_status()` (intermediate) or `transition_run()` (terminal)
- Event bubbling: child events propagated as `delegation_event` to all ancestors up to root
- Timeout enforcement: auto-fails `delegating` tasks after 30 min (`awaiting_input` tasks never timeout)

### Worker

Domain-agnostic executor. Pulls `TaskAssignment` from transport, delegates to `TaskExecutor`, forwards events/outcomes back. Spawns a heartbeat loop for each task to prevent the reaper from reclaiming it.

```
Worker.run()
  while assignment = transport.recv_assignment():
    heartbeat = transport.spawn_heartbeat(task_id, 15s)
    executing = executor.execute(assignment)
    spawn:
      forward events → transport
      forward outcomes → transport
      forward cancellation ← transport
    heartbeat.cancel()
```

### Transport

Abstracted via `CoordinatorTransport` + `WorkerTransport` traits.

**DurableTransport** (default): Assignments persisted in `agentic_task_queue` table before dispatch. Workers poll the table via `FOR UPDATE SKIP LOCKED`. Survives process crashes. Includes heartbeat mechanism and background reaper.

**LocalTransport** (testing): In-process tokio channels. Used in unit tests.

### Durable Task Queue

```
agentic_task_queue (
  task_id TEXT PK,
  run_id TEXT FK,
  queue_status TEXT,    -- queued → claimed → completed|failed|cancelled|dead_lettered
  spec JSONB,           -- serialized TaskSpec
  policy JSONB,         -- serialized TaskPolicy
  worker_id TEXT,       -- which worker claimed this
  last_heartbeat TIMESTAMPTZ,
  visibility_timeout_secs INT DEFAULT 60,
  claim_count INT,      -- incremented on each claim
  max_claims INT DEFAULT 3,
)
```

**Reaper** (background, every 30s): Re-queues tasks whose heartbeat expired past `visibility_timeout_secs`. Tasks exceeding `max_claims` are dead-lettered (`queue_status = 'dead_lettered'`).

### PipelineTaskExecutor

The composition point — knows all domains. Dispatches `TaskSpec` variants:

| TaskSpec | Action |
|----------|--------|
| `Agent` | Start analytics/builder pipeline via `PipelineBuilder` |
| `Workflow` | Load YAML, seed `WorkflowRunState`, enqueue initial `WorkflowDecision` |
| `Resume` | Load run from DB, rebuild pipeline, resume from `SuspendedRunData` |
| `WorkflowStep` | Execute a single SQL/semantic/omni/looker step |
| `WorkflowDecision` | Stateless decision task: load state, fold child answer, decide next action |

## Task Status Reference

`task_status` is the **single source of truth** for run lifecycle. The old `status` column was dropped; user-facing status is derived at the API layer via `user_facing_status()`.

| task_status | User-facing | Meaning | Resumable? | Timeout? |
|-------------|-------------|---------|------------|----------|
| `running` | "running" | Actively executing | Yes | No |
| `awaiting_input` | "suspended" | Blocked on human answer | Yes (HTTP) | **Never** |
| `delegating` | "running" | Waiting for child task(s) | Yes | 30 min |
| `done` | "done" | Completed successfully | No | — |
| `failed` | "failed" | Runtime error | No | — |
| `cancelled` | "cancelled" | User-initiated stop | No | — |
| `timed_out` | "failed" | Delegation exceeded timeout | No | — |
| `shutdown` | "failed" | Server graceful shutdown | Yes (recovery) | — |

## Task Lifecycle

```
                submit_root()
                     │
                     ▼
               ┌──────────┐
               │  running  │
               └────┬──┬───┘
                    │  │
       Suspended    │  │  Done/Failed/Cancelled
       ┌────────────┘  └──────────────┐
       ▼                              ▼
 ┌───────────────┐             ┌──────────┐
 │awaiting_input │             │   done   │
 └───────┬───────┘             └──────────┘
         │ answer                    
         ▼                     ┌──────────┐
   ┌──────────┐                │  failed  │
   │  running │                └──────────┘
   └──────────┘
                               ┌──────────┐
       Delegation              │cancelled │
       ┌─────────┐             └──────────┘
       ▼         │
 ┌───────────────┴──┐          ┌──────────┐
 │   delegating     │──→ all   │timed_out │
 │  child_ids: [...]│   done   └──────────┘
 │  completed: {...}│──→ resume parent
 │  failure_policy  │──→ or timeout after 30 min
 └──────────────────┘
```

## Persistence Patterns

Two patterns for writing task state:

1. **`persist_task_status()`**: Intermediate state tracking (`delegating`, `awaiting_input`, `running`)
   - Sets `task_status` + `task_metadata` only
   - Non-terminal states

2. **`transition_run()`**: Atomic terminal state transition
   - Sets `task_status` + `task_metadata` + `answer`/`error_message` in **one UPDATE**
   - Used for `done`/`failed`/`cancelled`/`timed_out`
   - Eliminates crash window between separate status writes

## Agent Flow

When an analytics question triggers an agent pipeline:

```
1. HTTP POST /runs { question, agent_id }
       │
2. PipelineBuilder.start() → StartedPipeline
       │
3. drive_with_coordinator(started, db, state, ...)
       │
       ├─ Create DurableTransport, Worker, Coordinator
       ├─ register_root(run_id)  ← virtual worker (already running)
       ├─ Worker.run()           ← handles child tasks via queue
       └─ Coordinator.run()      ← main event loop
              │
4. Analytics Orchestrator runs: Clarify → Specify → Solve → Execute → Interpret
       │
       ├─ Events stream to coordinator → DB → SSE → Frontend
       │
       ├─ [If LLM calls ask_user]
       │     TaskOutcome::Suspended { reason: HumanInput }
       │     → Coordinator marks awaiting_input (no timeout)
       │     → HTTP delivers answer
       │     → Coordinator resumes via TaskSpec::Resume
       │
       ├─ [If solver delegates to another agent]
       │     TaskOutcome::Suspended { reason: Delegation { target: Agent } }
       │     → Coordinator spawns child task via queue
       │     → Worker claims from queue, executes child
       │     → Child completes → Coordinator resumes parent
       │
       └─ [If solver delegates to workflow/automation]
             TaskOutcome::Suspended { reason: Delegation { target: Workflow } }
             → Coordinator spawns child task via queue
             → See "Automation Flow" below
```

## Automation Flow (WorkflowDecider — stateless)

Workflows now use a **Temporal-inspired stateless decision pattern**. Instead of a long-lived `WorkflowStepOrchestrator` actor with in-memory channels, workflow progress is driven by short-lived `WorkflowDecision` tasks that load state from DB, decide the next action, and exit. No in-memory channels survive a crash.

```
1. Analytics Executing stage detects SolutionSource::Automation
       │
2. Suspends with Delegation { target: Workflow { "proc.yml" } }
       │
3. Coordinator spawns child task (uuid.1) via durable queue
       │
4. Worker claims TaskSpec::Workflow { workflow_ref: "proc.yml" }
       │
5. PipelineTaskExecutor.execute_workflow():
       ├─ Loads YAML via WorkspaceContext.resolve_workflow_yaml()
       ├─ Parses into WorkflowConfig (local types, no oxy dependency)
       ├─ Seeds WorkflowRunState in agentic_workflow_state table
       ├─ Emits subrun_started event
       └─ Returns TaskOutcome::Done (workflow task itself is done)
              │
6. Coordinator detects workflow run_id in metadata:
       → Enqueues TaskSpec::WorkflowDecision { run_id, pending_child_answer: None }
       │
7. Worker claims WorkflowDecision task:
       ├─ Loads WorkflowRunState from DB
       ├─ Folds pending_child_answer into state (if any)
       ├─ Calls WorkflowDecider::decide(state, child_answer)
       ├─ Persists updated state (optimistic CC via decision_version)
       └─ Returns decision as TaskOutcome:
              │
       ├─ [Inline step: formatter/conditional]
       │     Executes directly with minijinja (no I/O)
       │     Returns StepExecutedInline → chains to next WorkflowDecision
       │
       ├─ [Delegated step: execute_sql, semantic_query, omni, looker]
       │     Returns DelegateStep { spec: WorkflowStep { step_config, ... } }
       │     → Coordinator enqueues grandchild task (uuid.1.1)
       │     → Worker claims WorkflowStep, executes, returns result
       │     → Coordinator enqueues WorkflowDecision { pending_child_answer }
       │     → Next decision folds result, advances to next step
       │
       ├─ [Agent step]
       │     Returns DelegateStep { spec: Agent { agent_ref, prompt } }
       │     → Coordinator spawns agent child → full analytics pipeline
       │     → On completion → WorkflowDecision with child answer
       │
       ├─ [Sub-workflow step]
       │     Returns DelegateStep { spec: Workflow { src } }
       │     → Coordinator spawns child → recursive workflow
       │
       ├─ [Loop step: loop_sequential]
       │     Returns DelegateParallel with N WorkflowStep children
       │     → All children complete → single WorkflowDecision folds all results
       │
       ├─ [WaitForMoreChildren]
       │     Parallel siblings still in flight → no-op, wait for next completion
       │
       └─ [Complete { final_answer }]
             All steps done → TaskOutcome::Done { answer: JSON array }
       │
8. Coordinator resumes analytics parent with workflow results
       │
9. Analytics enters Interpreting stage:
       parse_delegation_answer() converts JSON array to AnalyticsResult
       LLM calls render_chart with result_index referencing step data
       Charts rendered → chart_rendered events → Frontend
```

### WorkflowRunState (durable workflow state)

Persisted in `agentic_workflow_state` table. Each `WorkflowDecision` task loads this, updates it, and exits.

```
WorkflowRunState {
  run_id,                  -- PK, FK → agentic_runs.id
  workflow: WorkflowConfig,
  workflow_yaml_hash,      -- detect config changes
  workflow_context,        -- workspace path, database configs, globals
  variables,               -- user-provided variables
  trace_id,                -- event correlation
  current_step: usize,    -- which step to execute next
  results: {name → JSON}, -- step name → OutputContainer result
  render_context,          -- accumulated minijinja context
  pending_children: {idx → [task_ids]}, -- in-flight child tasks per step
  decision_version: i64,  -- optimistic concurrency (incremented on every update)
}
```

### Legacy: WorkflowStepOrchestrator

The `WorkflowStepOrchestrator` (long-lived actor with in-memory answer channels) still exists in the codebase but is being superseded by the `WorkflowDecider` pattern. The key advantage of the decider is that no in-memory state survives a crash — all workflow progress is in the DB.

### Event Flow Through Task Tree

```
Grandchild (uuid.1.1) emits subrun_step_started
  │
  ├─ Persisted on grandchild's run (uuid.1.1)
  │
  └─ Coordinator bubbles as delegation_event to ALL ancestors:
       ├─ → Orchestrator run (uuid.1) as delegation_event
       └─ → Analytics root (uuid) as delegation_event
              │
              SSE stream reads from root → analytics handler unwraps
              delegation_event → subrun_step_started → Frontend
```

## Task Tree Example

```
Analytics root (uuid)                    task_status: running
├─ Workflow child (uuid.1)               task_status: running
│  ├─ SQL step (uuid.1.1)               task_status: done
│  ├─ SQL step (uuid.1.2)               task_status: running
│  └─ [loop fan-out]
│     ├─ Iteration (uuid.1.3)           task_status: done
│     ├─ Iteration (uuid.1.4)           task_status: done
│     └─ Iteration (uuid.1.5)           task_status: running
└─ [future: second delegation if needed]
```

## Retry & Fallback

When a child task has a `TaskPolicy`:

```
Child fails with error "connection timeout"
  │
  ├─ check_retry_or_fallback():
  │   ├─ retry.max_retries > attempt?
  │   │   ├─ retry_on patterns match? (empty = match all)
  │   │   │   → RetryAction::Retry { delay, attempt+1, same spec }
  │   │   │   → Backoff delay (Fixed or Exponential)
  │   │   │   → Re-assign same TaskSpec to worker
  │   │   │   → Emit delegation_retry event
  │   │   └─ Pattern doesn't match → skip to fallback
  │   └─ fallback_targets[fallback_index] exists?
  │       → RetryAction::Fallback { new_spec, fallback_index+1 }
  │       → Reset attempt counter
  │       → Emit delegation_fallback event
  │
  └─ All exhausted → finalize_failed() → propagate to parent
```

## Crash Recovery

Recovery is **transparent** — no attempt increment, no visible boundary to users. Partial events from the interrupted execution are cleaned up, and a lightweight `recovery_resumed` marker is emitted.

```
Server Startup
    │
    ├─ DurableTransport.run_reaper() [pre-pass]
    │     ├─ Re-queue tasks "claimed" by dead workers (heartbeat expired)
    │     └─ Dead-letter tasks exceeding max_claims
    │
    ├─ get_resumable_root_runs()
    │     └─ Root runs with task_status IN (running, awaiting_input, delegating, shutdown)
    │
    ├─ For each resumable root:
    │     │
    │     ├─ Delete partial events from interrupted execution
    │     │   (find last step_end/done/error, delete everything after it)
    │     │
    │     ├─ Emit recovery_resumed marker (same attempt, no increment)
    │     │
    │     ├─ Coordinator::from_db(root_run_id) → rebuild task tree
    │     │   ├─ Load runs via parent_run_id chain (BFS)
    │     │   ├─ Rebuild TaskNode state from DB
    │     │   ├─ Restore retry state (attempt, fallback_index, policy)
    │     │   ├─ Rebuild completed map from agentic_task_outcomes (crash-safe)
    │     │   ├─ Detect PendingResumes (children done, parent not resumed)
    │     │   └─ Initialize child_counter from existing children (collision-safe)
    │     │
    │     ├─ Pick ONE continuation for each task that has two descriptions:
    │     │   ├─ root at a suspension vs. its own queue entry (recovery/root_entry.rs)
    │     │   └─ parent whose children all reported → the resume that carries
    │     │      their answer, never also a re-launch (recovery/children_done.rs)
    │     │
    │     ├─ Walk tree, classify each task:
    │     │   ├─ done/failed → skip
    │     │   ├─ awaiting_input → leave as-is (user answers via HTTP)
    │     │   ├─ children all reported → leave to the pending resume
    │     │   ├─ delegating, children still out → leave waiting
    │     │   ├─ running + suspend data → re-launch from checkpoint
    │     │   └─ stale (no checkpoint) → mark failed
    │     │
    │     ├─ Process pending resumes (assign Resume / AutomationDecision
    │     │   carrying the children's answer)
    │     ├─ Register in RuntimeState (SSE notifiers, cancel channels)
    │     ├─ Spawn Worker (claims child tasks from durable queue)
    │     └─ Spawn Coordinator.run() (main event loop)
    │
    └─ Spawn reaper background task (every 30s)
    │
    └─ Start HTTP server
```

### Recovery Safety

- **Transparent**: No attempt increment — frontend sees one continuous event stream
- **Partial event cleanup**: Events from interrupted steps are deleted before re-execution
- **Event dedup**: `insert_event` uses `ON CONFLICT DO NOTHING` on `(run_id, seq)`
- **Seq consistency**: `from_db()` uses `get_max_seq() + 1` — no seq conflicts
- **Crash-safe handoff**: `agentic_task_outcomes` is the atomic source of truth for child→parent results (written BEFORE parent metadata update)
- **Durable queue**: Task assignments survive crashes; workers re-claim from queue on restart
- **Heartbeat/reaper**: Stale claimed tasks are automatically re-queued

## Graceful Shutdown

On SIGTERM/Ctrl+C:

```
1. Axum receives shutdown signal
     ↓
2. shutdown_token.cancel()
     ├─ SSE streams close (clients see stream end)
     ├─ Reaper cancelled
     └─ Shutdown watcher fires:
          ├─ RuntimeState.shutdown_all(&db):
          │     For each active run:
          │       ├─ Mark task_status = "shutdown" in DB (resumable)
          │       └─ Send cancel signal to pipeline via cancel_txs
          └─ Pipelines receive cancel → coordinators exit
               └─ On next restart: recovery resumes "shutdown" runs
```

**`shutdown` vs `cancelled`**: User-initiated cancel sets `task_status = "cancelled"` which is terminal and non-resumable. Graceful shutdown sets `task_status = "shutdown"` which the recovery pipeline treats as resumable, just like `running` or `delegating`.

## Database Schema

```sql
-- Task tree with parent-child hierarchy
agentic_runs (
  id TEXT PK,
  question TEXT,
  answer TEXT,
  error_message TEXT,
  thread_id UUID FK,
  source_type TEXT,       -- "analytics", "builder"
  metadata JSONB,         -- domain-specific
  parent_run_id TEXT FK,  -- self-referential for task tree

  -- Single source of truth for coordinator state
  task_status TEXT,       -- running|awaiting_input|delegating|done|failed|cancelled|timed_out|shutdown
  task_metadata JSONB,    -- child_task_ids, completed, failure_policy, retry state

  attempt INT DEFAULT 0,  -- reserved for future user-initiated retry
  recovery_requested_at TIMESTAMPTZ,  -- reserved for future selective recovery
  created_at, updated_at
)

-- Event stream (persisted for SSE catch-up)
agentic_run_events (
  id BIGSERIAL PK,
  run_id TEXT FK,
  seq BIGINT,             -- monotonic per run, UNIQUE(run_id, seq)
  event_type TEXT,
  payload JSONB,
  attempt INT DEFAULT 0,  -- which recovery attempt emitted this event
  created_at
)

-- HITL suspension data
agentic_run_suspensions (
  run_id TEXT PK FK,
  prompt TEXT,
  suggestions JSONB,
  resume_data JSONB,
  created_at
)

-- Atomic child→parent result handoff
agentic_task_outcomes (
  child_id TEXT PK FK,
  parent_id TEXT,
  status TEXT,            -- done|failed|cancelled
  answer TEXT,
  created_at
)

-- Durable workflow state (Temporal-inspired, per-run)
agentic_workflow_state (
  run_id TEXT PK FK,        -- FK → agentic_runs.id
  workflow_config JSONB,    -- parsed WorkflowConfig
  workflow_yaml_hash TEXT,  -- detect config changes
  workflow_context JSONB,   -- workspace path, database configs
  variables JSONB,          -- user-provided variables
  trace_id TEXT,            -- event correlation
  current_step INT,         -- which step to execute next
  results JSONB,            -- step name → OutputContainer result
  render_context JSONB,     -- accumulated minijinja context
  pending_children JSONB,   -- step_index → [child_task_ids] in flight
  decision_version BIGINT DEFAULT 0,  -- optimistic concurrency control
  created_at, updated_at
)

-- Durable task queue (assignments survive crashes)
agentic_task_queue (
  task_id TEXT PK,
  run_id TEXT FK,
  parent_task_id TEXT,
  queue_status TEXT,      -- queued|claimed|completed|failed|cancelled|dead_lettered
  spec JSONB,             -- serialized TaskSpec
  policy JSONB,           -- serialized TaskPolicy
  worker_id TEXT,
  last_heartbeat TIMESTAMPTZ,
  claimed_at TIMESTAMPTZ,
  visibility_timeout_secs INT DEFAULT 60,
  claim_count INT DEFAULT 0,
  max_claims INT DEFAULT 3,
  created_at, updated_at
)
```

## Key Design Decisions

1. **Coordinator is domain-agnostic.** It sees `TaskSpec`, `TaskOutcome`, `SuspendReason` — never analytics/builder/workflow types.

2. **Event bubbling to root.** Child events are wrapped as `delegation_event` and propagated up the entire ancestor chain. The SSE stream reads from the root run, so all events are visible.

3. **Stateless workflow decisions (Temporal-inspired).** Instead of long-lived `WorkflowStepOrchestrator` actors with in-memory answer channels (`orchestrator_txs`), workflows use `WorkflowDecision` tasks. Each decision loads durable `WorkflowRunState` from DB, folds a child answer, decides the next action, persists updated state, and exits. No in-memory channels survive a crash. Optimistic concurrency via `decision_version` prevents lost updates.

4. **Virtual worker pattern.** The root pipeline is already running when the coordinator starts. No assignment needed — just register and wire events.

5. **WorkflowStep routing.** `WorkflowDecider` returns explicit `TaskSpec::WorkflowStep` or `TaskSpec::Agent` for each step. The coordinator enqueues these directly — no context-sniffing needed.

6. **Durable transport.** `DurableTransport` persists assignments in `agentic_task_queue` via `FOR UPDATE SKIP LOCKED`. Workers heartbeat every 15s; a background reaper re-queues stale tasks after `visibility_timeout_secs`. `LocalTransport` kept for unit tests.

7. **Single status source of truth.** `task_status` is the only status column. The old `status` column was dropped. User-facing status is derived via `user_facing_status()` at the API serialization layer.

8. **Transparent recovery.** Crash recovery does NOT increment the attempt counter or emit visible boundaries. Partial events from interrupted steps are deleted; a lightweight `recovery_resumed` marker is emitted. The frontend sees one continuous event stream.

9. **HITL never times out.** Only `delegating` tasks (waiting for children) time out after 30 min. `awaiting_input` tasks (waiting for human answer) persist indefinitely — they consume no resources and users may respond hours later.

10. **Cancelled is distinct from failed.** `TaskOutcome::Cancelled` routes to `handle_cancelled()`, not `handle_failed()`. This preserves user intent in the DB (`task_status = "cancelled"`) and skips retry/fallback logic.

11. **Shutdown is distinct from cancelled.** Graceful shutdown marks runs as `task_status = "shutdown"`, not `"cancelled"`. `shutdown` is resumable by the recovery pipeline on restart; `cancelled` is terminal and non-resumable. This prevents data loss when the server restarts.
