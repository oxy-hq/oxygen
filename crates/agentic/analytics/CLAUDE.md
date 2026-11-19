# agentic-analytics

Analytics domain implementation. Turns natural-language questions into structured SQL queries via a multi-stage FSM pipeline.

## Pipeline Stages

```
Clarifying → Specifying → Solving → Executing → Interpreting → Done
     ↑            ↑          ↑          ↑            ↑
     └────────────┴──────────┴──────────┴────────────┘
                      Diagnosing (back-edges)
```

| Stage | What it does | Tools available |
| ------- | ------------- | ---------------- |
| Clarifying | Triage question type, resolve metrics/dimensions | `search_catalog`, `search_automations`, `ask_user` |
| Specifying | Ground intent into query spec, resolve joins | `get_valid_dimensions`, `get_column_range` |
| Solving | Generate SQL (semantic model or LLM) | `explain_plan`, `dry_run` |
| Executing | Run SQL against connector, validate results | — |
| Interpreting | Convert results to natural language + charts | `render_chart` |

**Semantic shortcut:** If the semantic model can compile the query directly in Clarifying, Specifying and Solving are skipped.

## Extension Table

Domain-specific run data in `analytics_run_extensions`:

| Column | Type | Purpose |
| -------- | ------ | --------- |
| `run_id` | TEXT PK | FK to `agentic_runs.id` |
| `agent_id` | TEXT | Which `.agentic.yml` config was used |
| `spec_hint` | JSONB | Prior query structure for cross-turn continuity |
| `thinking_mode` | TEXT | `"auto"` or `"extended_thinking"` |

Migrator: `AnalyticsMigrator` with tracking table `seaql_migrations_analytics`.

## Key Exports

- `AnalyticsSolver` / `build_analytics_handlers()` — domain solver + state handlers
- `AnalyticsEvent` — domain-specific events (schema_resolved, intent_clarified, query_generated, etc.)
- `start_pipeline(PipelineParams)` → `PipelineHandle<AnalyticsEvent>`
- `event_handler()` → `DomainHandler` for EventRegistry registration
- `AgentConfig` / `BuildContext` — config loading and solver building
- `SchemaCatalog` / `SemanticCatalog` — data catalog implementations
- `extension::AnalyticsMigrator` — extension table migrations
- `extension::crud::*` — extension table CRUD

## Rules

- Must NOT import any other domain crate (`agentic-automation`)
  or upward crates (`agentic-http`, `agentic-pipeline`).
- May import `agentic-core`, `agentic-runtime`, `agentic-connector`, `agentic-llm`.
- Cross-domain subrun delegation goes through `agentic_core::subrun::SubrunRunner`.
  `agentic-pipeline` injects a concrete impl (workflow's `OxyAutomationRunner`)
  via `AnalyticsSolver::with_subrun_runner`. Subrun lifecycle events
  (`SubrunStarted`, `SubrunCompleted`, `SubrunStepStarted`, `SubrunStepCompleted`)
  arrive on the analytics event stream when delegation runs.
- Domain events implement `DomainEvents` trait from core.
- `is_accumulated()` on `AnalyticsEvent` controls which events appear in `StepEnd` metadata.
