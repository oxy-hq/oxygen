# agentic-runtime

Transport-agnostic execution infrastructure for agentic pipelines. Provides run lifecycle management, event persistence, and streaming — used by both HTTP and CLI.

## Layout

Stage 1 of the airway/airform extraction split the crate into two sub-layers; pick the layer your code actually needs.

| Layer | Modules | Purpose |
| -------- | --------- | --------- |
| `lifecycle/` | `state`, `handle`, `bridge`, `event_registry`, `entity/{run,run_event,run_suspension}`, `crud/{runs,events,suspension,queries}` | The "what a run *is*" half: run row + event log + suspensions + SSE plumbing. Zero orchestrator deps. |
| `orchestrator/` | `coordinator/`, `worker`, `router/`, `transport/`, `circuit_breaker`, `background`, `entity/{task_queue,task_outcome}`, `crud/{queue,outcomes,recovery}` | The "how a run *executes*" half: durable task queue + coordinator + worker pool + transports. Built on top of lifecycle. |
| (root) | `migration` | Single SeaORM migrator covering both layers (`seaql_migrations_orchestrator` tracking table). |

The legacy flat paths (`agentic_runtime::coordinator`, `agentic_runtime::state`, `agentic_runtime::crud::*`, etc.) are still re-exported at the crate root for back-compat with the ~180 external callsites — new external code should prefer the canonical `lifecycle::…` / `orchestrator::…` paths so the layering shows up in `use` lists. Inside this crate the canonical paths are mandatory; the flat aliases are external-only.

## Rules

- **Never import domain crates** (analytics, builder, connector, llm). Only depends on `agentic-core`.
- **Never import HTTP types** (axum). This crate is transport-agnostic.
- Domain-specific behavior is injected via:
  - `serialize_fn: Fn(&Event<Ev>) -> (String, Value)` for bridge task serialization
  - `RowProcessor` closures registered in `EventRegistry` for deserialization
  - `OnResumeFn` callback for domain-specific resume logic
- **Owned queue writes are fenced on the claim, not the worker.** `worker_id` is process-stable, so a stale execution and this process's re-claim of the same row look identical to it. `agentic_task_queue.claim_epoch` (bumped by every claim/adopt, never decremented — unlike the `claim_count` retry budget) rides `TaskAssignment.claim_epoch` → `WorkerMessage::Outcome`/`Defer`, and every heartbeat/terminal/defer/release predicate carries it. Cancel tokens and heartbeat tickers are keyed by `(task_id, claim)`. `NO_CLAIM` (0) means unfenced: in-memory transports and virtual root workers. Don't add a task_id-keyed marker to paper over a missing epoch.
- **The worker cap is per process.** `Worker::new` (queue-claimed runs) draws from `process_slots(SlotPool::Queue)` and request-driven drives use `Worker::in_pool(.., SlotPool::Direct)` — each `OXY_WORKER_MAX_INFLIGHT` (default 32) wide, shared by every run in it. Keep request-driven drives out of the queue pool: their `scope_owned` rows can't move to another process. A transport whose receive claims a row must take the slot *before* the claim and hold none while idle (`WorkerTransport::recv_admitted`); a slot is released when the task ends or suspends. Don't give a production worker a pool of its own (`with_max_inflight` is for tests).
- The queue NOTIFY carries `task:<task_id>`; a scoped poller wakes only for its own tree (`router/wake.rs`). Anything unattributable must wake everyone — never drop a wake you can't parse.
- The `agentic_runs` table has no domain-specific columns — only `source_type` and `metadata` (JSONB).
- `PipelineHandle<Ev>` requires `Ev: DomainEvents` — domains provide the concrete type, runtime handles it generically.

## Key Types

```rust
// Transport-agnostic state — used by HTTP server and CLI
pub struct RuntimeState {
    pub notifiers: DashMap<String, Arc<Notify>>,  // wake SSE/CLI subscribers
    pub answer_txs: DashMap<String, Sender<String>>,  // HITL answers
    pub cancel_txs: DashMap<String, watch::Sender<bool>>,  // cancellation
    pub statuses: DashMap<String, RunStatus>,  // in-memory cache
}

// Domain-agnostic pipeline handle
pub struct PipelineHandle<Ev: DomainEvents> {
    pub events: Receiver<Event<Ev>>,
    pub outcomes: Receiver<PipelineOutcome>,
    pub answers: Sender<String>,
    pub cancel: CancellationToken,
    pub join: JoinHandle<()>,
}
```

## Integrating a new system

If you're standing up a new top-level system on top of this runtime
(airform, future ELT/transformation runners — airway is already on the
queue-driven pattern below), follow the walkthrough in
[`internal-docs/agentic-runtime-integration.md`](../../../internal-docs/agentic-runtime-integration.md).
It covers three patterns:

- **Pipeline-style** (analytics, builder) — `DomainSolver` +
  `PipelineHandle`. Lifecycle only; no orchestrator queue.
- **One-shot queue work** (per-workspace health eval) —
  `TaskSpec::Custom { kind }` + a `CustomTaskRegistry` executor registered by
  the host. One durable unit, no FSM, no resume.
- **Queue-driven** (automation and airway; airform still to come) — dedicated
  `TaskSpec` variants drained off the durable task queue; coordinator does the
  fan-out + resume. **Currently a stub** in that doc — read
  `crates/agentic/automation/` and `crates/agentic/airway/` for now.

The integration doc has the API surface for both layers, the
contributor checklist, and the rules around extension tables and
migrators.

## Testing

- Unit tests: `cargo nextest run -p agentic-runtime`
- Integration tests: `OXY_DATABASE_URL=... cargo nextest run -p agentic-runtime --test integration -E 'test(integration_tests)'` (requires PostgreSQL — use testcontainers, never the dev DB)
