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

## `react` module (the react runtime pilot)

`src/react/` is the analytics toolset for `agentic-runtime`'s react loop: `search` and
`read` over the agent's `context:` file snapshot, `query` (raw SQL or semantic) behind a
read-only gate, the system prompt, answer citations and table rendering. The loop is in
`agentic-runtime`; `agentic-pipeline::react_run` assembles a run. See
`internal-docs/agent-engine-runtime.md` (Phase 3 as built).

- **Leak boundary.** `search` and `read` reach only the files the agent's `context:`
  globs resolve (view, topic, `.sql`, `.md`; no `.test.` names, no `..` or symlink
  escapes), loaded once into `WorkspaceFiles`. That keeps eval gold SQL and notes out of
  the agent's reach; never add a tool that reads the workspace outside the snapshot.
- **The metric-tree and anomaly tools are the one exception, and return no file text.**
  `tools::MetricTreeTool` (`explain_metric`, `find_opportunities`, `metric_sensitivity`,
  `predict_impact`, the FSM's names) runs the FSM's executor over the host's
  `MetricTreeRunner`, which loads the whole compiled semantic model and runs SQL airlayer
  generates. Only the op's numbers come back. They are offered only with a runner
  (`ReactAnalytics::with_metric_tree`), as on the FSM.
  `tools::AnomalyTool` (`list_anomalies`, `detect_anomalies`, `explain_anomaly`) does the
  same over the host's `AnomalyStore` and that runner: it reads anomaly rows (numbers and
  member ids, scoped to the platform's workspace) and writes only to that store. It is
  offered only with both ports (`ReactAnalytics::with_anomalies`), because the FSM's
  executor needs both.
- **Layering.** Analytics knowledge stays here. `agentic-runtime` must not learn
  anything about analytics: a tool talks to the loop only through `ToolReply` and opaque
  `Artifact { event_type, payload }` rows, and never through an analytics type.
- **The read-only gate fails closed; it is not the guarantee.** `query` runs nothing
  that `read_only::gate` has not accepted as one read statement. A parse error is a
  refusal, never a fallback. The gate is the fast refusal with a hint. The guarantee is
  the connector's `execute_query_read_only` (a read-only Postgres transaction with a
  role check, a locked DuckDB), which `query` calls and nothing else; it must hold with
  the gate deleted. Never route `query` through `execute_query`. A gate relaxation
  still needs a test that the fake connector never received the write.
- **A rollup read is the one query that skips the connector.** `tools::rollup` answers
  a covered `semantic` query from a live pre-aggregation's Parquet in an in-memory
  DuckDB (`preagg_exec::execute_rollup`, the FSM's executor). It is safe because the SQL
  is Oxy's (`generate_reagg_sql`), wrapped as `SELECT * FROM (…) LIMIT n`, and reads
  only files the rebuild wrote. A raw `sql` or `verified` query must never reach it.
  The `is_preagg` flag on the row names the tier that answered, so a fallback to the
  warehouse reports `false`.

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
