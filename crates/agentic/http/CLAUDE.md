# agentic-http

Axum HTTP routes for the agentic pipeline. This is the **transport layer** — thin handlers that map HTTP requests to `agentic-pipeline` and `agentic-runtime` operations.

## Routes

| Method | Path | Handler | Description |
| -------- | ------ | --------- | ------------- |
| POST | `/runs` | `create_run` | Start a pipeline (analytics or builder) |
| GET | `/runs/:id/events` | `stream_events` | SSE stream with Last-Event-ID catch-up |
| POST | `/runs/:id/answer` | `answer_run` | Deliver HITL answer to suspended run |
| POST | `/runs/:id/cancel` | `cancel_run` | Cancel running pipeline |
| PATCH | `/runs/:id/thinking_mode` | `update_thinking_mode` | Update thinking mode on completed run |
| GET | `/threads/:thread_id/run` | `get_run_by_thread` | Latest run for a thread |
| GET | `/threads/:thread_id/runs` | `list_runs_by_thread` | All runs for a thread |

## Dependencies

```
agentic-http depends on:
  agentic-pipeline  (facade — the ONLY domain entry point)
  agentic-runtime   (CRUD/state — domain-agnostic)
  oxy-auth          (AuthenticatedUserExtractor — the only oxy-* import)
```

**Zero `oxy::*` imports.** The host app opens its SeaORM `DatabaseConnection`
once at startup and passes it to `AgenticState::new`. Per-request workspace
state arrives as `Extension<Arc<agentic_pipeline::platform::OxyProjectContext>>`
from the app's `workspace_middleware`.

**Zero direct imports of:** analytics, builder, connector, llm, core, entity, workflow.

## Rules

- **Never import domain crates directly.** All domain access goes through `agentic-pipeline`.
- **Never import `oxy::*`.** Host-project state comes via `Extension<Arc<OxyProjectContext>>`;
  the DB connection lives on `AgenticState::db`.
- **Never import `entity` crate.** Thread ownership queries go through `agentic-pipeline::get_thread_owner`.
- `db.rs` is pure re-exports from runtime + pipeline — no custom logic.
- `sse.rs` is domain-agnostic — `UiEvent`, `squash_deltas`, `is_terminal` only.
- `state.rs` uses `agentic_pipeline::AnalyticsSchemaCatalog` and `BuilderTestRunnerTrait` re-exports.
- `routes.rs` uses `PipelineBuilder` for all pipeline creation — no inline config/solver setup.
- **A caller-supplied `thread_id` is checked before a run starts on it** (`run_scope::ensure_thread_access`):
  404 for a missing thread or one in another workspace, 403 for another user's. The pipeline reads
  the thread's history into the run and appends the run to it, so an unchecked id is a read of
  someone else's conversation and a write into it. Unowned (auto-provisioned) threads stay open to
  the workspace.

## State Management

`AgenticState` wraps `RuntimeState` (via `Deref`) and adds:

- `db` — shared SeaORM `DatabaseConnection` (pool internally), cloned per handler
- `schema_cache` — shared analytics schema introspection cache
- `builder_test_runner` — injected test runner for builder copilot
- `event_registry` — domain event processors for SSE streaming

## SSE Streaming

`stream_events` uses the `EventRegistry` from state to process raw DB rows into frontend events. The registry picks the right domain processor based on the run's `source_type`. No domain event types are imported — the registry handles deserialization internally via registered `RowProcessor` closures.
