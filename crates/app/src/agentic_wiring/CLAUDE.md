# agentic_wiring

Oxy-specific adapters that plug into the agentic stack.

This module is the **only** place in the codebase that bridges Oxy's
`WorkspaceManager` / `entity` / `oxy::config::*` types into the agentic
port traits declared in [`agentic_pipeline::platform`]. When the platform
refactor lands, the churn lives in this module.

## Contents

| Path | Role |
| ------ | ------ |
| `project_ctx`, `compile_dispatcher` | In `oxy-workspace-runtime` since 2026-11-22 (W4), re-exported here. `project_ctx`: `OxyProjectContext`, which implements `ProjectContext` + `agentic_automation::WorkspaceContext` (combined into `PlatformContext` via blanket impl); its connector half (`build_connector_for_db`, `try_database_to_connector_config`, the Airhouse wire/mint helpers) is `oxy_workspace_runtime::connector` (W1b). `compile_dispatcher`: `OxyCompileDispatcher`, the `CompileDispatcher` a `TaskSpec::Compile` runs through |
| `scan_source`, `metric_tree_runner`, `metric_tree_exec` | In `oxy-workspace-runtime` since 2026-11-22 (W2), re-exported here. `scan_source`: where a semantic read finds the model (`resolve_query_scan_source`, `resolve_compiled_scan_source`, `QueryScanSource`) and the `{ message }` refusal envelope, down from `api::semantic` (2026-11-16). `metric_tree_runner`: `OxyMetricTreeRunner` and the query executors. `metric_tree_exec`: the executor-facing core the metric-tree surfaces share |
| `sql_exec`, `sql_http` | In `oxy-workspace-runtime` since 2026-11-22 (W1b), re-exported here. `sql_exec`: how a Dev Portal query runs (`run_via_agentic_connector`, `build_connector`, the 10k ad-hoc row cap), down from `api::data` (2026-11-16). `sql_http`: its HTTP answer (`SqlErrorResponse`, `agentic_error_response`, `execute_sql_response`: `400`/`409 preview_read_only`/`500`/`502`), down from `api::data` (2026-11-16, Wave D2) |
| `preview_ctx` | In `oxy-workspace-runtime` since 2026-11-22 (W5), re-exported here, the last of the wiring: `PreviewPlatformContext`, the platform a workspace preview's dry run and Airway sample run on. Its run resolver, `server::previews::runtime`, went with it (`oxy_workspace_runtime::preview_runtime`) |
| `thread_owner`, `airhouse_pool`, `app_airhouse`, `metric_sink`, `string_literal` | In `oxy-workspace-runtime` (`crates/workspace-runtime/src/`) since 2026-11-21, re-exported here (`pub use`) so the paths did not change. `thread_owner`: `OxyThreadOwnerLookup`, the auth-facing `ThreadOwnerLookup` trait against `entity::threads`. `airhouse_pool`: the process-wide Airhouse connection pool. `app_airhouse`: `ctx.airhouse`'s connector |

## Wiring at app startup

```rust
let db = oxy::database::client::establish_connection().await?;
let thread_owner: Arc<dyn ThreadOwnerLookup> =
    Arc::new(OxyThreadOwnerLookup::new(db.clone()));
let agentic_state = Arc::new(AgenticState::new(shutdown, db, thread_owner));
```

## Per-request wiring

The Axum `workspace_middleware` builds the per-workspace adapters and
inserts them as extensions:

```rust
let project_ctx = Arc::new(OxyProjectContext::new(workspace_manager.clone()));
let platform: Arc<dyn PlatformContext> = project_ctx.clone();
request.extensions_mut().insert(platform);
```

HTTP handlers in `agentic-http` extract `Extension<Arc<dyn PlatformContext>>`
— they never see `oxy::*` types.

## Rules

- This module is the ONLY place in the app crate that owns both
  `oxy::*` types AND agentic port traits. Don't sprinkle adapters elsewhere.
- Don't leak `WorkspaceManager` into the `PipelineBuilder` call chain — go
  through `Arc<dyn PlatformContext>`.
- Don't add ad-hoc queries against the platform `threads` table; extend
  `ThreadOwnerLookup` (or add another similar trait in `agentic_pipeline::platform`)
  if you need more of that kind of access.
