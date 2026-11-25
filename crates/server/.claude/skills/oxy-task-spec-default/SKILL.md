---
name: oxy-task-spec-default
description: Use when adding or modifying code in `crates/server/src/` (the router, the drivers in `src/drivers/`), a `crates/api-*` surface, or any HTTP handler that involves long-running compute, periodic schedules, multi-step pipelines, or any work that must survive instance death. Triggers include "spawn a background task", "schedule periodically", "run async after returning", "long-running operation", "fire and forget", "tokio::spawn", "background job", "queue this", or PRs that add new work to HTTP handlers. Also triggers when designing features that touch LLM APIs, git clones, embedding builds, or any operation taking >5 seconds.
---

# Default long-running work to TaskSpec on the worker fleet

Per Oxy's scaling design (`internal-docs/multi-instance-fleet.md`, refinement H):

> If a feature has any of {long-running compute, periodic schedule, durability requirement, multi-step pipeline}, it should be a TaskSpec on the worker fleet, not a `tokio::spawn` in an HTTP handler.

## What this means concretely

- New background work **defaults to** enqueueing a `TaskSpec` in `agentic_task_queue` via the existing durable orchestrator in `crates/agentic/runtime/src/orchestrator/`.
- New `tokio::spawn` inside HTTP handlers requires written justification (sub-second fire-and-forget only).
- The worker fleet (`oxy worker`) drains the queue; the HTTP fleet (`oxy serve`) only enqueues.

## When to default to TaskSpec — quick checklist

1. **Does it take more than ~5 seconds?** → TaskSpec.
2. **Does it touch the network, LLM APIs, or external services?** → TaskSpec.
3. **Must it survive `oxy serve` restart?** → TaskSpec.
4. **Will the client poll for status?** → TaskSpec.
5. **Does it run on a schedule?** → TaskSpec.
6. **Could it be retried?** → TaskSpec.

If **any** of these is yes, default to TaskSpec.

## How to add a new TaskSpec variant

1. **Define the variant** in the `TaskSpec` enum (find it under `crates/agentic/runtime/src/`). Carry only what the executor needs; reference indirect data (e.g., `auth_token_ref` not the raw token).
2. **Implement the executor branch** — invoke the actual work, emit lifecycle events (`Started`, `Progress(_)`, `Done` / `Failed`).
3. **Wire the enqueue site** — replace the `tokio::spawn` with `transport.enqueue(TaskSpec::YourVariant{...})`.
4. **Status tracking** — domain-specific status columns are updated by the executor as the single writer. Never spawn AND update from the same call site.
5. **Idempotency** — if the same logical work is enqueued twice (e.g., same repo URL + target path), dedupe on a stable key before INSERT.
6. **Retry policy** — set a `TaskPolicy` with backoff. Don't retry terminal errors (auth expired, permission denied); do retry transient ones (network blip).
7. **Tests** — unit-test the executor happy path + at least one failure mode + idempotency.

## Counter-examples — when `tokio::spawn` is acceptable

- Sub-second fire-and-forget that genuinely doesn't need durability (e.g., emit a metric).
- Per-request streaming aggregation that ends when the SSE connection closes.
- Background work tied to a single SSE stream's lifecycle and reset on reconnect.

In all other cases, prefer TaskSpec.

## Where to look for existing examples

- **Workflow execution**: `TaskSpec::Workflow` — the most mature executor.
- **Airway ELT**: `TaskSpec::Airway` — heavy IO + retries + progress events.
- **Builder analytics**: `TaskSpec::Agent` — LLM-driven multi-step pipeline.
- **Preagg cycles**: `TaskSpec::Preagg` — periodic refresh.

## Refs

- Design doc: `internal-docs/multi-instance-fleet.md` (refinement H)
- Scope survey of current violations: `internal-docs/worker-fleet.md`
- Worker fleet guide: `internal-docs/worker-fleet.md`
- Backend architecture rules: `internal-docs/backend-architecture.md`
