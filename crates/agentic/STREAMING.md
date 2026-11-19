# Event Streaming Pipeline

## Overview

Events flow from the domain FSM through a batched DB write pipeline and out to SSE subscribers. Domain-specific payloads are processed by a registry at the HTTP boundary so the runtime and transport layers never import domain types.

```
Domain Solver
    │ emits Event<Ev>
    ▼
EventStream<Ev>  (mpsc channel)
    │
    ▼
run_bridge()  ←── 20 ms tick ──→ batch writes
    │                            to agentic_run_events
    ├─ duration_ms injection (terminal events)
    ├─ on_event callback (state updates, suspension)
    └─ state.notify(run_id)
    │
    ▼
RuntimeState.notifiers  (Arc<Notify> per run)
    │
    ▼
SSE handler  (per-connection)
    │ uses EventRegistry[source_type]
    ▼
StreamProcessor
    │ CoreEvent → UI block(s)
    │ DomainEvent → domain-specific UI block(s)
    ▼
Frontend (SSE stream)
```

## Core Components

### Event Types

Events are split into two layers:

**`CoreEvent`** ([core/src/events.rs](core/src/events.rs)) — framework-level lifecycle events emitted by the orchestrator:
- `StateEnter { state, revision, trace_id, sub_spec_index }`
- `StateExit { state, outcome, trace_id, sub_spec_index }`
- `BackEdge { from, to, reason, trace_id }`
- `LlmStart / LlmToken / LlmEnd` — LLM streaming per HTTP round
- `ThinkingStart / ThinkingToken / ThinkingEnd` — extended-thinking blobs
- `ToolCall { name, input } / ToolResult { name, output }`
- `AwaitingHumanInput { questions } / InputResolved`
- `FanOut { count } / SubSpecStart { index } / SubSpecEnd { index, outcome }`
- `DelegationStarted / DelegationEvent / DelegationCompleted / DelegationFailed`
- `Done { duration_ms } / Error { message, duration_ms }`
- `ValidationPass / ValidationFail`

**`DomainEvents`** trait — domain enum types. Concrete impls:
- `AnalyticsEvent` ([analytics/src/events.rs](analytics/src/events.rs)): `TriageCompleted`, `IntentClarified`, `SpecResolved`, `SemanticShortcutAttempted/Resolved`, `QueryGenerated`, `QueryExecuted`, `AnalysisComplete`, `ProposedChart`, `AutomationStarted/StepStarted/Completed`, `ToolUsed`

**Unified wrapper**:
```rust
pub enum Event<Ev: DomainEvents> {
    Core(CoreEvent),
    Domain(Ev),
}
```

Both flavors implement `.serialize() -> (event_type: String, payload: Value)` so the bridge doesn't care which is which.

### EventStream

`EventStream<Ev> = mpsc::Sender<Event<Ev>>`. Passed as `Option<EventStream<Ev>>` to every solver method. `None` means "not streaming" (used in unit tests).

Solvers emit via convenience macros:
```rust
emit_core!(events, StateEnter { state: "clarifying", ... });
emit_domain!(events, AnalyticsEvent::IntentClarified { ... });
```

Sender clones are cheap; fan-out workers each get their own clone.

### Bridge (run_bridge)

The bridge is the single consumer of `EventStream`. It drains the channel, batches writes to `agentic_run_events`, and notifies SSE subscribers.

**Signature** ([runtime/src/bridge.rs:33](runtime/src/bridge.rs#L33)):
```rust
pub async fn run_bridge<Ev: DomainEvents>(
    db: &DatabaseConnection,
    state: &RuntimeState,
    run_id: &str,
    mut event_rx: mpsc::Receiver<Event<Ev>>,
    pipeline_start: Instant,
    on_event: Option<OnEventFn<Ev>>,
    attempt: i32,
)
```

**Loop behavior**:

```
buffer = []
tick   = every 20 ms
loop:
  select:
    ev = event_rx.recv() → 
        (event_type, payload) = ev.serialize()
        if event_type is terminal ("done", "error"):
          payload.duration_ms = now - pipeline_start
        buffer.push((event_type, payload, attempt))
        on_event?(ev, &mut run_state)   ← e.g. flag suspension
        if event_type is terminal or suspension:
          flush_now()   ← don't wait for tick
    tick:
        if buffer: flush_now()
    channel closed:
        flush_remaining(); return

flush_now():
  crud::batch_insert_events(db, run_id, buffer)   ← ON CONFLICT DO NOTHING
  state.notify(run_id)   ← wake all SSE subscribers
  buffer.clear()
```

**Why batch**: Per-token LLM events at 30–100 Hz would hammer Postgres. Batching drops DB load by ~20× while keeping UI latency under the human-perceptual threshold.

**`on_event` callback**: Optional hook for domain-specific state updates. Used to:
- Detect `AwaitingHumanInput` → set `task_status = "awaiting_input"`
- Detect `Done` → write answer, clear active state
- Update domain extension tables on key events

### Persistence (agentic_run_events)

```sql
agentic_run_events (
  id BIGSERIAL PK,
  run_id TEXT FK,
  seq BIGINT,             -- monotonic per run
  event_type TEXT,        -- "state_enter", "llm_token", "query_executed", ...
  payload JSONB,          -- serialized Event
  attempt INT DEFAULT 0,  -- recovery attempt (0 = original run)
  created_at TIMESTAMPTZ,
  UNIQUE(run_id, seq)
)
```

- **`seq`** is monotonic per run. `batch_insert_events()` allocates `next_seq .. next_seq + batch.len()` atomically.
- **`ON CONFLICT DO NOTHING`** on `(run_id, seq)` makes inserts idempotent — safe to retry on crash recovery.
- **`attempt`** is set by the bridge; does NOT increment on transparent crash recovery (see COORDINATOR.md § Recovery Safety).

### RuntimeState.notifiers

In-memory `DashMap<run_id, Arc<Notify>>`. SSE handlers await on the `Notify`; the bridge wakes all waiters on flush.

```rust
state.notify(run_id):
  if let Some(n) = notifiers.get(run_id):
    n.notify_waiters()   ← wake every subscriber for this run
```

No per-event fanout through channels — instead, subscribers read new rows from the DB. This scales: N SSE connections = 1 notifier, not N channels.

### EventRegistry

Domain-specific event processing at the HTTP boundary. The runtime and transport layers never import domain event types; instead, each domain registers a `DomainHandler` keyed by `source_type`.

**`DomainHandler` struct** ([runtime/src/event_registry.rs:35](runtime/src/event_registry.rs#L35)):
```rust
pub struct DomainHandler {
    pub processor: RowProcessor,        // Fn(event_type, payload) -> Option<Vec<UiBlock>>
    pub summary_fn: SummaryFn,          // Fn(state) -> Option<String>
    pub tool_summary_fn: ToolSummaryFn, // Fn(tool_name) -> Option<String>
    pub should_accumulate: Option<AccumulationFilter>,
}
```

**Registration** (at server startup):
```rust
let registry = EventRegistry::new();
registry.register("analytics", agentic_analytics::event_handler());
```

Each domain exports `pub fn event_handler() -> DomainHandler`:
- **analytics**: deserializes `AnalyticsEvent` via `domain_row_processor::<AnalyticsEvent>()`; accumulates `intent_clarified`, `spec_resolved`, `query_executed`, etc. for end-of-state summary metadata

### StreamProcessor

Per-SSE-connection state. Built from `registry.stream_processor(source_type)`.

```rust
StreamProcessor {
    ui_transform_state: UiTransformState,  // squash deltas, accumulate state metadata
    domain_accumulator: Option<Box<...>>,  // per-state event accumulator
}

fn process_row(event_type, payload) -> Vec<UiBlock>:
    // 1. Deserialize via domain processor
    // 2. Squash consecutive llm_token → single llm_delta block
    // 3. Accumulate should_accumulate events into StateExit metadata
    // 4. Emit UI blocks
```

## SSE Streaming Flow

HTTP handler (`GET /runs/:id/events`):

```
1. Client connects with optional Last-Event-ID header
   │
2. handler spawns async stream:
   │
   ├─ Catch-up: read agentic_run_events WHERE run_id=? AND seq > last_event_id
   │     For each row: processor.process_row(event_type, payload) → UI blocks
   │     Yield each block as SSE `data: {...}\n\n`
   │     Client sees history in seq order
   │
   ├─ Live loop:
   │     last_seq = max seq from catch-up
   │     loop:
   │       notifier.notified().await        ← bridge woke us
   │       rows = SELECT … WHERE seq > last_seq
   │       for row in rows:
   │         blocks = processor.process_row(row)
   │         for block in blocks:
   │           yield SSE event
   │         last_seq = row.seq
   │       if terminal event seen: break
   │
   └─ close stream (Done / Error)
```

**Last-Event-ID catch-up** lets clients reconnect after network blips without losing events. The client sends `Last-Event-ID: 42`, server resends everything with `seq > 42`.

**Squashing** — the StreamProcessor coalesces consecutive `llm_token` events into a single `llm_delta` UI block, so the frontend doesn't re-render on every token.

## Event Bubbling Through Task Tree

Child task events propagate up to every ancestor as `delegation_event`, so the SSE stream on the root sees everything. See [COORDINATOR.md § Event Flow Through Task Tree](COORDINATOR.md#event-flow-through-task-tree) for the task-tree perspective.

```
Grandchild (uuid.1.1) emits subrun_step_started
    │
    ├─ Persisted on grandchild's run (uuid.1.1, seq_N)
    │
    └─ Coordinator wraps as CoreEvent::DelegationEvent { child_id, inner: <payload> }
       and persists on every ancestor:
         ├─ → uuid.1  (workflow parent)
         └─ → uuid    (analytics root)
                │
                SSE stream for uuid sees delegation_event →
                processor unwraps inner → renders subrun_step_started UI block
```

The SSE handler on the root run transparently unwraps `delegation_event` payloads via the domain processor — frontend renders grandchild events as if they came directly from root.

## Thinking Blob Semantics

Extended-thinking content requires special handling:

- **`ThinkingStart` / `ThinkingToken` / `ThinkingEnd`** events stream through the pipeline like `LlmToken`, one block per thinking segment.
- **Encrypted thinking blobs** returned by the LLM are preserved **within a tool loop** (sent back on the next turn to continue the reasoning chain).
- **Never cross state boundaries** — discarded on `StateEnter` of a new state. The FSM state transition is the reasoning boundary.
- **Controlled by `ThinkingConfig`** from the agent YAML (`thinking_mode: auto | extended_thinking`), stored on `analytics_run_extensions.thinking_mode`.

## Recovery & Event Dedup

On crash recovery:

1. **Partial event cleanup**: Delete events after the last `step_end` / `done` / `error` marker on the interrupted run. Prevents duplicate state transitions on replay. Airway runs emit none of these, so nothing is deleted from them; an airway run whose events already carry `load_completed` is finished as `done` before this step and gets no marker at all.
2. **Event dedup**: `ON CONFLICT DO NOTHING` on `(run_id, seq)` means re-emitted events are silently dropped.
3. **Seq consistency**: `Coordinator::from_db()` uses `get_max_seq() + 1` — no seq collisions.
4. **`recovery_resumed` marker**: A lightweight event emitted once on recovery. The frontend shows no attempt boundary; the event is there for debugging only.
5. **Attempt counter NOT incremented**: `payload.attempt` stays at 0 for transparent recovery. Only explicit user-initiated retries (future) would increment.

See [COORDINATOR.md § Crash Recovery](COORDINATOR.md#crash-recovery) for the full recovery orchestration.

## Frontend Event Contract

UI blocks emitted by the `StreamProcessor`:

| Block kind | When | Content |
|------------|------|---------|
| `state_block` | On `StateExit` | State name + aggregated metadata (tools used, tokens, results) |
| `llm_delta` | Squashed `LlmToken` run | Incremental text |
| `thinking_delta` | Squashed `ThinkingToken` run | Incremental thinking text |
| `tool_call` | `ToolCall` | Tool name, serialized input |
| `tool_result` | `ToolResult` | Tool name, serialized output |
| `chart_proposal` | `ProposedChart` (analytics) | Chart type, config, result ref |
| `query_result` | `QueryExecuted` (analytics) | Rows, columns, metadata |
| `awaiting_input` | `AwaitingHumanInput` | Prompt + suggestions |
| `sub_spec_event` | Any event with `sub_spec_index` | Wrapped event tagged to a fan-out branch |
| `done` | `Done` | Final answer |
| `error` | `Error` | Error message |

Blocks are framework-agnostic JSON; the React client renders them via type-driven switches.

## Key Design Decisions

1. **Events are the contract between layers.** Solvers emit; bridge persists; SSE replays. No shared mutable state between the domain logic and the delivery pipeline.

2. **Batched writes over per-event flush.** 20 ms tick keeps LLM-token storms from saturating Postgres. Terminal and suspension events flush immediately to preserve latency on critical transitions.

3. **DB is the event queue.** SSE subscribers read from `agentic_run_events`, not from in-process channels. Scales to N subscribers without fan-out overhead and supports Last-Event-ID catch-up for free.

4. **Notify, don't push.** A single `Arc<Notify>` per run wakes all subscribers. They each do their own catch-up query. Decouples bridge throughput from subscriber count.

5. **Core vs Domain events split.** `CoreEvent` is framework; `DomainEvents` is per-domain. Runtime never imports domain types — domains register processors at startup via `EventRegistry`.

6. **Idempotent inserts.** `ON CONFLICT DO NOTHING` on `(run_id, seq)` makes recovery trivial. Partial-event cleanup prevents duplicate *state transitions* on replay, not just duplicate *rows*.

7. **Event bubbling through task tree.** Child events propagate to every ancestor as `delegation_event`. The SSE stream on the root run sees every grandchild event without cross-run subscription.

8. **Thinking blobs don't cross state boundaries.** State transitions are the reasoning boundary. Preserving thinking across Clarifying → Specifying would leak prior-stage context into prompts that shouldn't see it.

9. **UI blocks, not events, cross the wire.** The StreamProcessor transforms raw events into UI-shaped blocks. Frontend renders blocks; it never deserializes raw CoreEvent/DomainEvent variants. This lets the UI contract evolve independently from the FSM.

10. **`sub_spec_index` on every event.** Fan-out branches tag every event with their index, so the frontend can render N cards in parallel without the bridge tracking which events belong to which branch.
