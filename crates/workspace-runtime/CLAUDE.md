# oxy-workspace-runtime

The Oxy-side adapters that plug into the agentic stack. This crate bridges
Oxy's `WorkspaceManager`, `entity` and `oxy::config::*` types into the agentic
port traits declared in `agentic_pipeline::platform` (and
`agentic_automation::WorkspaceContext`). When the platform refactor changes
those types, the churn lands here.

It sits below `oxy-server` and the `api-*` surfaces and above `oxy-app-core`.
It has no routes and no migrator. `src/lib.rs` lists each module and when it
arrived. These rules came from `crates/app/src/agentic_wiring/CLAUDE.md`,
which was deleted with `oxy-app` (decompose DEL, 2026-11-25).

## Wiring at startup

`crates/server/src/router/entry.rs` builds the agentic state once:

```rust
let thread_owner: Arc<dyn ThreadOwnerLookup> =
    Arc::new(OxyThreadOwnerLookup::new(db.clone()));
let state = Arc::new(agentic_http::new_agentic_state(shutdown_token, db, thread_owner));
```

## Wiring per request

`workspace_middleware` (`src/request/workspace_context.rs`, in this crate)
builds the per-workspace adapter and inserts it as an extension. The
background drivers (`crates/server/src/drivers/`) build the same adapter
through `src/driver_ctx.rs`. `crates/server/src/router/middlewares/local_context.rs`
also builds one, but it is the legacy `--local` path: do not design around it.

```rust
let project_ctx = Arc::new(OxyProjectContext::new(workspace_manager.clone()));
let platform: Arc<dyn PlatformContext> = project_ctx.clone();
request.extensions_mut().insert(platform);
```

HTTP handlers in `agentic-http` extract `Extension<Arc<dyn PlatformContext>>`.
They never see `oxy::*` types.

## Rules

- **Production adapters live here.** This is the only crate that holds both
  `oxy::*` types and implementations of the agentic port traits
  (`ProjectContext`, `WorkspaceContext`, `ThreadOwnerLookup`,
  `CompileDispatcher`, `MetricTreeRunner`). Do not add adapters in other
  crates. Other crates may construct `OxyProjectContext`. Test doubles and
  no-op stubs are fine anywhere. An example is the `NoThreadOwner` in
  `oxy-server`'s `route_declarations()`, which builds a router only to read
  its route roles.
- **Keep `WorkspaceManager` out of the `PipelineBuilder` call chain.** Pass
  `Arc<dyn PlatformContext>` instead.
- **Do not query the platform `threads` table ad hoc.** If you need more of
  that kind of access, extend `ThreadOwnerLookup`, or add a similar trait in
  `agentic_pipeline::platform`.
