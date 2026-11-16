# agentic_wiring

Oxy-specific adapters that plug into the agentic stack.

This module is the **only** place in the codebase that bridges Oxy's
`WorkspaceManager` / `entity` / `oxy::config::*` types into the agentic
port traits declared in [`agentic_pipeline::platform`]. When the platform
refactor lands, the churn lives in this module.

## Contents

| Path | Role |
| ------ | ------ |
| [`project_ctx.rs`](project_ctx.rs) | `OxyProjectContext`: implements `ProjectContext` + `agentic_automation::WorkspaceContext` (combined into `PlatformContext` via blanket impl) |
| [`builder_bridges/database_provider.rs`](builder_bridges/database_provider.rs) | `OxyBuilderDatabaseProvider` — resolves and builds DB connectors for the builder domain |
| [`builder_bridges/project_validator.rs`](builder_bridges/project_validator.rs) | `OxyBuilderProjectValidator` — validates workflow / agent / app / semantic files via oxy's `ConfigBuilder` |
| [`builder_bridges/schema_provider.rs`](builder_bridges/schema_provider.rs) | `OxyBuilderSchemaProvider` — generates JSON schemas for builder-copilot tools via `schemars::schema_for!` on `oxy::config::model` types |
| [`builder_bridges/semantic_compiler.rs`](builder_bridges/semantic_compiler.rs) | `OxyBuilderSemanticCompiler` — compiles semantic queries via `agentic_automation::semantic_bridge` |
| [`scan_source.rs`](scan_source.rs) | Where a semantic read finds the model (`resolve_query_scan_source`, `resolve_compiled_scan_source`, `QueryScanSource`) and the `{ message }` refusal envelope. Down from `api::semantic` (2026-11-16) |
| [`sql_exec.rs`](sql_exec.rs) | How a Dev Portal query runs (`run_via_agentic_connector`, `build_connector`, the 10k ad-hoc row cap). Down from `api::data` (2026-11-16) |
| [`sql_http.rs`](sql_http.rs) | The HTTP answer of a Dev Portal SQL execute (`SqlErrorResponse`, `agentic_error_response`, `execute_sql_response`: `400`/`409 preview_read_only`/`500`/`502`). Down from `api::data` (2026-11-16, Wave D2) |
| [`thread_owner.rs`](thread_owner.rs) | `OxyThreadOwnerLookup` — implements the auth-facing `ThreadOwnerLookup` trait against `entity::threads` |
| [`mod.rs`](mod.rs) | `build_builder_bridges(project_ctx)` — assembles the four builder bridges into `BuilderBridges` for `PipelineBuilder::with_builder_bridges` |

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
let bridges = build_builder_bridges(project_ctx);
request.extensions_mut().insert(platform);
request.extensions_mut().insert(bridges);
```

HTTP handlers in `agentic-http` extract `Extension<Arc<dyn PlatformContext>>`
and `Extension<BuilderBridges>` — they never see `oxy::*` types.

## Rules

- This module is the ONLY place in the app crate that owns both
  `oxy::*` types AND agentic port traits. Don't sprinkle adapters elsewhere.
- Don't leak `WorkspaceManager` into the `PipelineBuilder` call chain — go
  through `Arc<dyn PlatformContext>`.
- Don't add ad-hoc queries against the platform `threads` table; extend
  `ThreadOwnerLookup` (or add another similar trait in `agentic_pipeline::platform`)
  if you need more of that kind of access.
