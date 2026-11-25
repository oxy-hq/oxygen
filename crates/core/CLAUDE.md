# `oxy` — Core Platform Library (`crates/core`)

The published core library (crate name **`oxy`**, path `crates/core`, ~39k LOC). It is the
shared platform layer that the `oxy` binary (`oxy-server`) and most other crates build on:
config/project modeling, warehouse connectors, the execution engine, the semantic loader,
templating, and cross-cutting services (secrets, DB pool, observability). It is a **library
only** — no CLI, no Axum server (those live in `oxy-server` and the `api-*` surfaces).

## Layering

`oxy` sits below the transport/agentic layers and above `oxy-shared`:

```
oxy-server, api-*      ┐
agentic-pipeline/http   ┤── may import →  oxy  ── imports →  oxy-shared, entity, external
other domain crates     ┘
```

- **`oxy` must NOT import any `agentic-*` crate** (Platform → Agentic is banned; see
  `internal-docs/backend-architecture.md`). Agentic crates depend on `oxy`, never the reverse.
- Cross-cutting utilities and error types live in `oxy-shared` (re-exported here as
  `oxy::shared`). Prefer extending `oxy-shared` over adding another leaf crate.
- Use `OxyError` (`oxy_shared::errors`) + `thiserror`; `tracing` (`info!`/`warn!`/`debug!`),
  never `println!`; CLI-facing strings via the `StyledText` trait from `oxy_shared::theme`
  (re-exported as `oxy::theme`).

## Module map (`src/`, declared in `lib.rs`)

| Module | Owns |
| ------ | ---- |
| `config` (~6.6k) | Project/workspace config model + `ConfigManager` (`resolve_*`, `default_model`, path resolution). `config/model.rs` is the 3k-line schema — the biggest file in the crate. |
| `connector` (~5.1k) | Warehouse backends (DuckDB, Snowflake, BigQuery via connectorx, Postgres/MySQL, ClickHouse). Empty-result + result-cap short-circuits live here (see product-context gotchas). |
| `exec_runtime` (~1.3k) | Execution runtime the pre-agentic executor left behind: `ExecutionContext`, `renderer`, `writer`, `formatters`. The type vocabulary is the sibling `oxy::exec_types` (Phase 3). The `execute` module + its dead pipeline were fully retired (old-executor-retirement Phases 4a–4c). |
| `service` (~4.8k) | Cross-cutting services: `secret_manager.rs`, DB pool, project services. |
| `adapters` (~3.2k) | External-system adapters. |
| `semantic` (~2.5k) | Semantic-model file discovery + `loader.rs` (skips hidden/build dirs — stray copies cause "duplicate view" errors). Compilation itself is `oxy-semantic`/airlayer. |
| `metrics` (~1.3k) | Metrics plumbing. |
| `github` (~0.9k) | GitHub API helpers. |
| `observability` (~0.9k) | Core observability hooks (distinct from the `oxy-observability` crate). |
| `tools`, `types`, `execution_analytics`, `database`, `render`, `state_dir` | Tool defs, shared types, exec analytics, DB helpers, client renderer (`render_stream`), state-dir resolution. `theme` (`StyledText`) is `oxy-shared`'s since 2026-11-21, re-exported here. |

## Key entry points

- `oxy::config::ConfigManager` — resolve project files/paths, read `default_model`, mode detection.
- `oxy::exec_runtime::ExecutionContext` — event-sink/render context threaded through query + tool execution.
- `oxy::exec_types` — execution/result type vocabulary; the result-truncation flag is `is_result_truncated` (`exec_types/reference.rs`).
- `oxy::service::secret_manager` — project-scoped secret lookups (must filter by project — tenant isolation invariant).
- `oxy::render::{render_stream, ClientRenderer}` — streaming render surface.
- `oxy::state_dir::get_state_dir` — local state directory (used by the binary).

## Conventions

- Keep files ≤ ~400 lines and functions ≤ ~60 lines (`config/model.rs` and the connector
  files are legacy exceptions, not a license to add more). New surface → new module.
- Warehouse code paths must respect the DuckDB pool's serialized init (concurrent open →
  SIGSEGV) and each connector must short-circuit its empty-result shape.
- Run `cargo check -p oxy` after edits; the `oxy` binary (CLI and server) is `cargo check -p oxy-server`.
