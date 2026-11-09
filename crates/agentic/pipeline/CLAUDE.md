# agentic-pipeline

High-level facade for starting and driving agentic pipelines. This is the **composition layer** — the only crate that knows about all domains. Both `agentic-http` and the CLI depend on this crate instead of importing domain crates directly.

## Key Types

```rust
// Callers assemble the platform adapter + builder bridges in their host
// crate (for Oxy: `app::agentic_wiring`) and pass them in.
let project_ctx = Arc::new(OxyProjectContext::new(workspace_manager));
let platform: Arc<dyn PlatformContext> = project_ctx.clone();
let bridges = build_builder_bridges(project_ctx);

// Fluent builder for pipeline setup
PipelineBuilder::new(platform.clone())
    .with_builder_bridges(bridges)   // required for builder domain
    .analytics("agent_id")            // or .builder(model)
    .question("What is revenue?")
    .thread(thread_uuid)
    .thinking_mode(ThinkingMode::Auto)
    .start(&db).await?                // → StartedPipeline

// Type-erased handle — hides domain event type
started_pipeline.drive(db, state, answer_rx, cancel_rx).await;
```

## What This Crate Provides

| Function | What it does |
| ---------- | ------------- |
| `PipelineBuilder::start()` | Config loading + connector resolution + solver building + DB insert + pipeline startup |
| `StartedPipeline::drive()` | Type-erased `drive_pipeline()` call — works for any domain |
| `build_event_registry()` | Creates `EventRegistry` with all domains pre-registered |
| `insert_run()` | Runtime run insert + analytics extension insert |
| `update_run_done()` | Runtime update + analytics spec_hint extension |
| `update_run_thinking_mode()` | Analytics extension update |
| `get_thread_history()` | Runtime query + analytics extension join |
| `get_analytics_extension[s]()` | Extension table queries |
| `run_agentic_eval()` | Headless pipeline execution for eval/testing |
| `platform::resolve_connectors()` | Resolve database names → `ConnectorConfig` via `ProjectContext` |
| `platform::build_llm_client()` | Map a `ResolvedModelInfo` to a concrete `LlmClient` |

## Config Path Resolution

`agent_definition::read_analytics_agent` (used by `load_analytics_agent`,
`resume_analytics` and the headless eval) refuses an `agent_id` that is not a
relative path inside the workspace (absolute, `..`, empty), since it comes from
HTTP bodies and queued runs. It then asks `resolve_agent_yaml`, and on `None` reads the working copy
with extension fallback:

1. Try literal path: `base_dir/agent_id`
2. If not found, try: `base_dir/agent_id.agentic.yml`
3. If neither exists, return the original path (produces a clear error)

A path that exists but resolves outside `base_dir` once symlinks are followed is
refused too. The CLI (`oxy agentic run`) turns an absolute path inside the
workspace into the relative name before it calls the builder.

## Platform Boundary

This crate has **zero `oxy::*` dependencies**. All access to host-project
state goes through three traits declared in [`platform`]:

| Trait | Purpose |
| ------- | --------- |
| `platform::ProjectContext` | `resolve_connector`, `resolve_model`, `resolve_secret` |
| `platform::ThreadOwnerLookup` | `thread_owner(thread_id)` — used by HTTP for auth |
| `platform::PlatformContext` | Supertrait: `ProjectContext + agentic_automation::WorkspaceContext`. Anywhere pipeline needs both, take `Arc<dyn PlatformContext>`. |

Plus a bundle for the builder domain:

| Type | Contents |
| ------ | ---------- |
| `platform::BuilderBridges` | The four `Arc<dyn Builder{Database,SchemaProvider,SemanticCompiler,ProjectValidator}>` impls the builder pipeline needs |

Adapters live in the host. For Oxy they live in
[`app::agentic_wiring`](../../app/src/agentic_wiring/) — `OxyProjectContext`
implements `PlatformContext`, `OxyThreadOwnerLookup` implements
`ThreadOwnerLookup`, and `build_builder_bridges()` assembles the bridges.
Platform-refactor work happens there, not here.

When adding a new platform touchpoint, add a method to the relevant port
trait in this crate and an impl in the host adapter. Do not add `oxy` as a
dependency to this crate.

## Rules

- This crate MAY import all agentic crates — it's the composition layer.
- **`src/` MUST NOT import** `oxy`, `oxy-shared`, or any other platform crate
  besides `entity`. It **does** depend on `entity` (a real `Cargo.toml`
  dependency) — imported with `use entity::` in exactly two files,
  `airway_config.rs` (`entity::airway_source_config`) and `backfill.rs`
  (`entity::backfill_checkpoints`, `entity::backfill_ranges`). The stage-2
  airway admission resolver (`airway_config::resolve_admission`) lives here
  specifically because this crate, unlike `agentic-airway`, is allowed to
  depend on it. It stays the **only** merge implementation: the automation
  domain reaches it through `airway_config::PipelineAirwayAdmissionResolver`,
  this crate's impl of the `agentic_automation::AirwayAdmissionResolver` port.
  Do not move the merge into `agentic-airway` to "share" it — that crate is
  barred from `entity`, and `agentic-automation` importing it would be a
  domain→domain edge. The returned type (`ResolvedAdmission`) lives in
  `agentic-core`, beside the `TaskSpec::Airway` fields it fills, so every
  layer can name the shape without taking a DB dependency.
  `recovery.rs`, `retry.rs`, `scheduler.rs`, and `executor/mod.rs` also
  reference something spelled `entity::`, but it's `agentic_runtime::entity`
  — a same-named, unrelated module local to `agentic-runtime`
  (`crates/agentic/runtime/src/lib.rs:52`), which has no `entity` crate
  dependency at all. Don't conflate the two when auditing this crate's
  `entity` usage — check the qualified path, not the substring.
  This prohibition governs `src/` only. Test binaries here **do** run the
  central migrator — `airway_source_config` is a central-migrator table, not an
  agentic-owned one — but they reach it through
  `oxy_test_utils::migration::migrate_shared_test_db`, which hard-codes central
  and takes the runtime and domain migrators as type parameters. No test names
  the `migration` crate, so it is not in `[dev-dependencies]`; it still arrives
  transitively (with `oxy-shared` behind it) for test binaries only, and never
  touches `src/`. The helper is generic precisely so that `oxy-test-utils`, a
  platform crate that `oxy` and `oxy-app` also dev-depend on, needs no
  `agentic-*` dependency of its own.
- Consumers (`http`, `app/cli`) should import `agentic-pipeline`, NOT domain crates.
- Re-exports domain types that consumers need: `ThinkingMode`, `SchemaCatalog`, `BuilderTestRunner`.

## Testing

- Integration tests: `OXY_DATABASE_URL=... cargo nextest run -p agentic-pipeline --test integration -E 'test(integration_tests)'` (8 tests)
- The workflow-recovery test provides its own `FakePlatform` impl of `PlatformContext` rather than constructing a real `WorkspaceManager` — the test deliberately exercises only the DB-driven executor arm.
