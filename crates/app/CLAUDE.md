# `oxy-app` — CLI + HTTP Server (`crates/app`)

The CLI + server **library** lives here (crate name **`oxy-app`**, path `crates/app`, ~221k
LOC under `src/` on 2026-11-08, down from a 321k peak when the custom-apps serving half
moved to `oxy-api-custom-apps` and 250k before the experiments context left for
`oxy-api-experiment`; still the largest crate; lib-only). The `oxy` **binary** is the thin `oxy-server` crate
(`crates/server`, the workspace default member), which mounts the API surfaces and calls into
this library. It wires everything together: the CLI
command surface, the Axum HTTP server (`server/`, ~80k LOC), the fleet/role machinery, the
worker runtime, and integration glue (Slack). Business logic should live in `oxy` and
the domain crates — this crate is the platform runtime plus the contexts not yet extracted.

**Do not start a new context here (rule S8 in `internal-docs/domain-boundaries.md`).** A new
feature, bounded context or integration goes in its own `crates/api-<context>` surface over a
domain crate, mounted by `oxy-server`. Growing a module already here is fine; adding a
top-level module to `src/`, `src/server/` or `src/server/api/` fails
`tests/routing/app_growth.rs`. Every edit here pays whole-crate work (about half of the
recompile: metadata, the 656 MB rlib, macro expansion) that grows with the crate, so a smaller
`oxy-app` is a faster edit loop for everyone.

## Layering & where logic belongs

`oxy-app` sits at the top of the platform stack and is the only crate allowed to depend on
nearly everything (`oxy`, agentic-http/pipeline, integrations). Keep it thin:

- **HTTP handlers are transport** — parse/validate → call `oxy` or `agentic-pipeline` →
  serialize. A handler past ~30 lines is leaking domain logic (see `backend-architecture.md`).
- **Enter the agentic subsystem only through `agentic-http`/`agentic-pipeline`**, never by
  importing `agentic-analytics`/`agentic-builder`/`agentic-connector`/`agentic-llm` directly.
- Long-running work (spawns, periodic loops, LLM calls, >5s clones) belongs in a `TaskSpec`
  on the durable queue, **not** a `tokio::spawn` in a handler — invoke the
  `oxy-task-spec-default` skill.

**The queue is the only background primitive, and a test holds the line.**
`tests/routing/handler_spawns.rs` scans `src/server/api/**` and every `crates/api-*/src/**`
for `tokio::spawn(` / `tokio::task::spawn(` outside test code and **fails the build** on a
file its `ALLOWED` list does not name — the same allowlist-as-backlog shape as
`tests/authz/authz_boundaries.rs`. Each entry says what the spawn does: a queue executor's
own task, a spawn that only pumps **this request's SSE stream** (admitted, but the reason
must say so), or `backlog:` work that should be a `TaskSpec`. A listed file that stops
spawning fails as a stale entry, so shrinking the list is the migration. Add an entry only
after the skill's checklist says the work really is request-scoped.

## Module map (`src/`, declared in `lib.rs`)

| Module | Owns |
| ------ | ---- |
| `cli/` | Command surface. `cli/commands/` has one file per one-shot command: `run.rs`, `compile.rs`, `migrate*.rs` (`migrate.rs` also holds the migration runner `serve`/`worker` call), `admin.rs`, `clean.rs`, … `mod.rs` declares them as `SubCommand`. The long-lived `serve` / `start` / `worker` are `oxy-server`'s (`crates/server/src/commands/`), reaching oxy-app through `host.rs`. HTTP-client commands (`publish`, `login`, `proxy`, `assume`, `api`) are not here — they are `oxyc` (`sdk/cli`). |
| `server/` | The Axum server (~80k LOC) — see breakdown below. |
| `agentic_wiring/` | Adapts `oxy` state into the shape `agentic-pipeline` needs (`project_ctx.rs` is a 1.8k-line god file — a decomposition target). Has its own `CLAUDE.md`. |
| `integrations/` | External integrations, notably the Slack bot (`integrations/slack/`). |
| — | Transactional email (SES) is `oxy_app_core::emails`, re-exported here as `crate::emails`; the magic-link template is `server/api/auth/magic_link.hbs`. Custom-app scaffold templates are in `oxy-api-custom-apps`. |
| `observability_boot.rs` / `observability_setup.rs` | Tracing/observability bootstrap for the binary. |

### `server/` internals

| Path | Owns |
| ---- | ---- |
| `server/api/**` | One file (or dir) per resource: `threads`, `apps`, `automation`, `projects/`, `admin/`, `billing/`, `custom_apps_*` (the serve + data hot path), `world_model_graph.rs` (4.8k — the single biggest file, a decomposition target), `workspaces.rs`, `onboarding.rs`, `auth.rs`. |
| `server/router/` | Route mounting: `public.rs`, `protected.rs`, `workspace.rs`, `global.rs`, `entry.rs`, `openapi.rs`. `ROUTES.md` documents the surface. |
| `server/role_manifest.rs` (~230 lines; roles travel as `RouteRoleDecl` next to each route) | **Routing authority** for the split fleet — classifies every route `IdeOnly` vs `FleetOk`. FS/`.git`/state-dir routes MUST be `IdeOnly`; persisted-data reads MUST stay `FleetOk`. Invoke `oxy-route-classification` before touching routes. |
| `server/{worker_runtime,worker_health,worker_metrics}.rs`, `preagg_*`, `compile_*` | Durable-task worker fleet + pre-aggregation + compile-boundary workers. |
| `server/{servability,admission,ide_proxy,role_middleware}.rs` | Whether a replica can serve a workspace without the working copy, self-routing reverse proxy (`IdeOnly` → ide upstream), admission control. `ServeMode` itself now lives in `oxy-app-core` — see below. |
| `server/service/` | Server-side services (api keys, app, eval, project, formatters). |

**Five modules now live in `oxy-app-core` (`crates/app-core`), not here** — `audit`,
`serve_mode`, `org_host_dispatch`, `custom_apps_host_dispatch`, `member_authz`. They moved
so the per-surface crates can share them without depending on each other. Import them as
`oxy_app_core::<module>`; no re-export was left behind, so a stale `crate::server::…` path
is a compile error, and an in-flight branch using one will merge clean and then fail to build.

**The experiments context left (2026-11-08):** `server/experiment`, `server/simulation`,
`api/experiment` and `api/simulation` are `oxy-api-experiment` (`crates/api-experiment`),
mounted by `oxy-server` through `SurfaceSeams::workspace`. Nothing here names them: the five
task kinds arrive through `SurfaceSeams::tasks` and the experiment tick through
`SurfaceSeams::global_ticks`, which the global driver (`router/recovery.rs`) runs beside its
own schedule ticks.

## Key entry points

- `cli::run` / `cli::prepare` — oxy-app's subcommands (`cli::SubCommand`), registered with the `oxy-cli-shim` parser by `crates/server/src/main.rs`, which owns argv parsing and `--version` (S10 in `internal-docs/domain-boundaries.md`); log format auto-detects local vs cloud (JSON).
- `oxy serve` (`crates/server/src/commands/serve.rs`) — production HTTP path; `--enterprise` is the default
  shape for tests/demos, `--local` is the single-user/no-auth laptop mode only.
- `server/router/entry.rs` — top-level router assembly.
- `server/role_manifest.rs` — the map every new route must be classified in.

## Conventions & pitfalls

- Deployment mode drives almost every behavior: **Local** (`oxy start`), **Remote**
  (`oxy serve --local`), **Cloud** (`oxy serve`). Bug triage starts with "which mode?"
- SSE streams (Builder/analytics/workflow) MUST emit a terminal event (`done`/`error`/
  `cancelled`) even on early failure, or the frontend hangs forever.
- Multi-tenant scoping is a correctness invariant — orchestrator/pre-agg endpoints filter by
  `workspace_id`; secret lookups filter by project.
- After CLI changes: `cargo build` then exercise `./target/debug/oxy <command>`.
- Check both packages after edits: `cargo check -p oxy-app` **and** `cargo check -p oxy`.
- The file/function size limits bite hardest here — this crate holds the workspace's worst
  god files (`world_model_graph.rs`, `role_manifest.rs`, `admin/apps/handlers.rs`,
  `project_ctx.rs`, `workspaces.rs`). Prefer a new module over growing one; see
  `internal-docs/domain-boundaries.md`.
