# Agentic Subsystem

## Architecture

Three-layer design — each layer has strict dependency rules:

```
          domains (analytics, builder)
               │
          pipeline (facade / composition)
               │
          runtime (execution infrastructure)
               │
          core (pure FSM)
```

| Layer | Crates | May depend on | Must NOT depend on |
| ------- | -------- | --------------- | ------------------- |
| **Core** | `agentic-core` | External only (serde, tokio) | Any `agentic-*` crate |
| **Runtime** | `agentic-runtime` | `core` | analytics, builder, workflow, connector, llm, pipeline, http |
| **Infrastructure** | `agentic-connector`, `agentic-llm` | `core` | analytics, builder, workflow, runtime, pipeline, http |
| **Domains** | `agentic-analytics`, `agentic-builder`, `agentic-automation` | `core`, `runtime`, `connector`, `llm` | Each other, pipeline, http |
| **Pipeline** | `agentic-pipeline` | All agentic crates, `oxy` | `http` |
| **HTTP** | `agentic-http` | `pipeline`, `runtime`, `oxy`, `oxy-auth` | analytics, builder, workflow, connector, llm, core, entity |

`agentic-automation` is a sibling domain alongside analytics/builder — no
domain imports another. The cross-domain "subrun" contract
(`SubrunRunner`, `SubrunStep`, `OxyCommentBlock`,
`parse_oxy_comment_block`) lives in `agentic-core::subrun` so any
delegating domain can discover and invoke any executor without taking
a dep on it. `agentic-pipeline` is the only place that wires a
concrete `dyn SubrunRunner` (automation's `OxyAutomationRunner`) into
the analytics solver.

## Crate Responsibilities

| Crate | What it owns | Key types |
| ------- | ------------- | ----------- |
| `core` | FSM framework + cross-domain subrun contract | `Domain`, `DomainSolver`, `Orchestrator`, `ProblemState`, `CoreEvent`, `UiBlock`, `SubrunRunner`, `SubrunStep`, `OxyCommentBlock` |
| `runtime` | Two sub-layers: [`lifecycle`] (run row, events, suspensions, SSE plumbing — `RuntimeState`, `PipelineHandle`, `EventRegistry`) and [`orchestrator`] (durable task queue, coordinator, worker pool, transports). Top-level re-exports keep legacy flat paths working. | `lifecycle::state::RuntimeState`, `lifecycle::handle::PipelineHandle`, `lifecycle::event_registry::EventRegistry`, `orchestrator::coordinator::Coordinator`, `orchestrator::worker::Worker` |
| `pipeline` | Pipeline setup, config resolution, type erasure | `PipelineBuilder`, `StartedPipeline`, `ThinkingMode` |
| `analytics` | Analytics solver, semantic model, extension table | `AnalyticsSolver`, `AnalyticsEvent`, `SchemaCatalog`, `AnalyticsMigrator` |
| `builder` | Builder solver, file tools (write/edit/delete), HITL suspensions | `BuilderSolver`, `BuilderEvent`, `BuilderTestRunner`, `BuilderAppRunner` |
| `connector` | Database backends | `DatabaseConnector`, `ConnectorConfig`, `SchemaInfo` |
| `llm` | LLM provider abstraction | `LlmClient`, `LlmProvider`, `ThinkingConfig` |
| `http` | Axum route handlers | `AgenticState`, `router()`, route handlers |
| `automation` | Sibling domain: stateless workflow runner + procedure execution + extension table. Implements `agentic_core::subrun::SubrunRunner` via `OxyAutomationRunner` (alias `OxyProcedureRunner`). | `WorkflowDecider`, `WorkflowRunState`, `commit_decision`, `WorkflowMigrator`, `OxyAutomationRunner`, `WorkflowEventBridge` |

## Migration Strategy

Four independent SeaORM migrators with separate tracking tables:

| Migrator | Tracking table | Location | Owns |
| ---------- | --------------- | ---------- | ------ |
| Central | `seaql_migrations` | `crates/migration/` | Platform + conversation tables |
| Runtime | `seaql_migrations_orchestrator` | `agentic-runtime` | `agentic_runs`, `agentic_run_events`, `agentic_run_suspensions` |
| Workflow | `seaql_migrations_workflow` | `agentic-automation` | `agentic_workflow_state` (incl. prior-cache snapshot columns) |
| Analytics | `seaql_migrations_analytics` | `agentic-analytics` | `analytics_run_extensions` |

Startup order: Central -> Runtime -> Workflow -> Analytics.

## Domain Extension Pattern

Domain-specific run data lives in extension tables, not on the generic `agentic_runs` table:

```
agentic_runs (runtime)          analytics_run_extensions (analytics domain)
├── id (PK)                     ├── run_id (PK, FK → agentic_runs.id)
├── question                    ├── agent_id
├── status                      ├── spec_hint (JSONB)
├── answer                      └── thinking_mode
├── source_type
├── metadata (JSONB)
└── ...
```

New domains add their own extension table with their own migrator. The runtime table stays generic.

## Adding a New Domain

For a quick FSM-style domain that fits the analytics/builder shape:

1. Create `crates/agentic/<domain>/` implementing `DomainSolver` from `core`
2. Add `start_pipeline()` returning `runtime::lifecycle::handle::PipelineHandle`
3. Register event handler via `event_handler()` returning `DomainHandler`
4. (Optional) Add extension table with own migrator
5. Wire into `agentic-pipeline`: add to `PipelineBuilder` + `ErasedHandle` + `build_event_registry()`
6. **No changes needed** to `runtime`, `core`, or `http`

For a heavier integration (queue-driven, multi-step, like `agentic-automation` —
the pattern airway/airform will follow), see the full integration reference
in [`internal-docs/agentic-runtime-integration.md`](../../internal-docs/agentic-runtime-integration.md).
That doc covers both patterns (pipeline-style vs queue-driven), the
lifecycle/orchestrator API surface, and a step-by-step checklist.

## Key Rules

- **Runtime is transport-agnostic** — no axum, no HTTP types. Works from HTTP, CLI, gRPC, or tests.
- **Entities are domain-private** — `agentic-http` has zero `entity` crate imports.
- **Cross-domain references are loose** — plain UUID columns, no FK constraints. Application-level cleanup.
- **Events are serialized as `(event_type, payload JSON)`** — the `EventRegistry` handles domain-specific deserialization at read time via registered `RowProcessor` callbacks.
- **Spawn through `agentic_core::hub_task`, never bare `tokio::spawn` / `spawn_blocking`** — `spawn_with_hub`, `spawn_blocking_with_hub`, or `bind_current_hub` on a future handed to a `JoinSet` (`agentic_runtime::hub_task` from `agentic-http`). A bare spawn drops the Sentry hub, and with it the custom-app tag that keeps a tenant's errors out of Sentry; `oxy-app`'s `every_agentic_spawn_carries_a_hub` fails the build on one. Only a boot-time loop with no request hub to inherit, or a task that outlives the request that started it (a memoized connection's driver), goes in its allowlist.
