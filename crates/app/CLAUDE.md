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
  importing `agentic-analytics`/`agentic-connector`/`agentic-llm` directly.
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
| `cli/` | Command surface. `cli/commands/` has one file per one-shot command: `run.rs`, `compile.rs`, `migrate*.rs` (`migrate.rs` also holds the migration runner `serve`/`worker` call), `admin.rs`, … `mod.rs` declares them as `SubCommand`. `oxy dev seed` is not here: it went up into `oxy-server`'s `dev::seed` (behind `dev-tools`) in decompose SD (2026-11-24), and `surface.rs`, the prelude the surfaces took from this crate, went with its last entry. The long-lived `serve` / `start` / `worker` are `oxy-server`'s (`crates/server/src/commands/`), reaching oxy-app through `host.rs`. HTTP-client commands (`publish`, `login`, `proxy`, `assume`, `api`) are not here — they are `oxyc` (`sdk/cli`). |
| `server/` | The Axum server (~80k LOC) — see breakdown below. |
| `agentic_wiring/` | Adapts `oxy` state into the shape `agentic-pipeline` needs. Has its own `CLAUDE.md`. It is going down into `oxy-workspace-runtime` (`crates/workspace-runtime`): the leaves (`airhouse_pool`, `app_airhouse`, `metric_sink`, `string_literal`, `thread_owner`, with `runtime_artifact` and `typed_stream`, whose `server::` re-exports went in decompose R1) moved there on 2026-11-21; `project_ctx`'s connector half, `sql_exec`, `sql_http` and `preagg_workspace` (its `server::` re-export also went in R1) followed on 2026-11-22 (W1b), and `metric_tree_runner`, `metric_tree_exec` and `scan_source` the same day (W2), and `project_ctx` (`OxyProjectContext`), `compile_dispatcher`, `server::preagg_executor` and the `WorkspaceContextCache` out of `server::router::workspace_cache` the same day too (W4). The last of it, the preview context (`preview_ctx`; `preview_airhouse` went down in W3), followed the same day with `server::previews::runtime` (W5), so `agentic_wiring/mod.rs` now only re-exports, and only what the CLI and `tests/` still name: `airhouse_pool`, `project_ctx`, `compile_dispatcher`, `OxyProjectContext` and `thread_owner`'s `OxyThreadOwnerLookup` (decompose SHM, 2026-11-23, deleted the rest). |
| `integrations/` | External integrations (QuickBooks OAuth, the OAuth provider). The Slack bot is `oxy-api-slack`; the eval context (`oxy dev test`, its event handler, the reporters and the engine) is `oxy-api-eval` (`crates/api-eval`). |
| — | Transactional email (SES) is `oxy_app_core::emails` (the `crate::emails` alias went in decompose RPT, 2026-11-24); the magic-link template is `oxy-api-identity`'s `sign_in/magic_link.hbs`. Custom-app scaffold templates are in `oxy-api-custom-apps`. |

### `server/` internals

| Path | Owns |
| ---- | ---- |
| `server/api/**` | One file (or dir) per resource: `projects/`, `admin/` (leaving section by section for the surfaces that own each context — `internal-docs/domain-boundaries.md` § The admin split), `billing/`, `custom_apps_*` (the serve + data hot path). The session plumbing is `oxy_app_core::session` (down 2026-11-21; the `api::auth` re-export went in decompose SHM, as did every `custom_apps_*` re-export but the bundle cache). Threads, agents, run artifacts and org chat are `oxy-api-threads` (`crates/api-threads`) since 2026-11-16; the workspaces surface (an org's workspace list, rename and delete, `/meta`, members, the Oxy-staff lockdown, the app integrations) is `oxy-api-workspaces` (`crates/api-workspaces`) the same day, and the semantic surface (the semantic explorer, the Dev Portal's `/sql` execute, the pre-aggregation status and rebuild, the revision reads) `oxy-api-semantic` (`crates/api-semantic`), Wave D2. |
| ~~`server/router/`~~ | **Gone (decompose LIFT, 2026-11-23):** the router assembly (`public.rs`, `protected.rs`, `workspace.rs`, `global.rs`, `entry.rs`, `openapi.rs`, `ROUTES.md`), its router-only middlewares (`api/middlewares/{api_key_query,app_grant_scope,app_publish_token_scope,local_context,request_id,timeout,token_grant_scope,token_usage}`), the local `/setup` handlers (`api/local_setup.rs`) and the admin console shell (`api/admin/mod.rs`) are `oxy-server`'s (`crates/server/src/{router,router/middlewares,admin}`), as the background drivers were since decompose BL1 (`crates/server/src/drivers/`). |
| ~~`server/role_manifest.rs`~~ (code in `crates/app-core/src/pods/role_manifest.rs`; the test installer that builds the real router, and `role_manifest_tests.rs`, went up into `oxy-server`'s `src/` with the router in decompose LIFT) | **Routing authority** for the split fleet — classifies every route `IdeOnly` vs `FleetOk`. FS/`.git`/state-dir routes MUST be `IdeOnly`; persisted-data reads MUST stay `FleetOk`. Invoke `/crates/server:oxy-route-classification` before touching routes. |
| `preagg_*`, `compile_*` (the worker's runtime, health listener and metrics went up into `oxy-server` on 2026-11-22, decompose UP2) | Durable-task worker fleet + pre-aggregation + compile-boundary workers. The pre-aggregation runtime's domain half (`freshness`, `rebuild`, `retract`, `generation`, and the post-promote nudge `promote`, re-exported at `server::preagg_promote`) is `oxy-preagg` (`crates/preagg`) since 2026-11-20, and the cycle's body (`run_preagg_task` and its request types) followed it as `oxy_preagg::cycle`; the executor shell (`PreaggTaskExecutor`, `build_task_config`, the `CycleWorkspace` impl for `OxyProjectContext`) is `oxy_workspace_runtime::preagg_executor` since 2026-11-22 (W4), and the workspace manager is `oxy_workspace_runtime::preagg_workspace` (W1b, the same day; its `server::preagg_workspace` re-export went in decompose R1). The compile runtime's leaves (`compile_git`, `compile_config_gate`, `compile_oltp`, `compile_maintenance`) are `oxy-compile-runtime` (`crates/compile-runtime`, as `git`, `config_gate`, `oltp`, `maintenance`) since 2026-11-21, `compile_request` followed as `request` on 2026-11-22, `compile_reconcile` (with the automatic compile enqueue) as `reconcile` the same day, and `compile_worker` as `worker` the same day. Only `server::compile_{git,config_gate,oltp,request,reconcile}` are still re-exported (the CLI and `tests/` name them); for anything else import the lower crate. |
| ~~`server::ide_proxy`~~ (the re-export of `oxy_app_core::pods::ide_proxy` went in decompose SHM; `server/role_middleware.rs`, which re-exported `pods::role_middleware` and kept the tests that build the real router, went up into `oxy-server` in decompose LIFT; code in `crates/app-core/src/pods/`. `server::servability`'s re-export went in decompose R1: import `oxy_app_core::pods::servability`) | Whether a replica can serve a workspace without the working copy, self-routing reverse proxy (the `IdeOnly` routes left → ide upstream; `?branch=` escalates nothing since the IDE was removed). Admission control (`OXY_MAX_INFLIGHT_*`), the `/metrics` listener and the span store's boot bridge are `oxy-server`'s (`crates/server/src/{admission,metrics_server,observability_boot}.rs`). `ServeMode` itself now lives in `oxy-app-core` — see below. |

**Five modules now live in `oxy-app-core` (`crates/app-core`), not here** — `audit`,
`serve_mode`, `org_host_dispatch`, `custom_apps_host_dispatch`, `member_authz`. They moved
so the per-surface crates can share them without depending on each other. Import them as
`oxy_app_core::<module>`; no re-export was left behind, so a stale `crate::server::…` path
is a compile error, and an in-flight branch using one will merge clean and then fail to build.

**The experiments context left (2026-11-08):** `server/experiment`, `server/simulation`,
`api/experiment` and `api/simulation` are `oxy-api-experiment` (`crates/api-experiment`),
mounted by `oxy-server` through `SurfaceSeams::workspace`. Nothing here names them: the five
task kinds arrive through `SurfaceSeams::tasks` and the experiment tick through
`SurfaceSeams::global_ticks`, which the global driver (`oxy-server`'s `drivers/recovery.rs`) runs beside its
own schedule ticks.

## Key entry points

- `cli::run` / `cli::prepare` — oxy-app's subcommands (`cli::SubCommand`), registered with the `oxy-cli-shim` parser by `crates/server/src/lib.rs` (`cli_owners`), whose thin `main.rs` owns argv parsing and `--version` (S10 in `internal-docs/domain-boundaries.md`); log format auto-detects local vs cloud (JSON).
- `oxy serve` (`crates/server/src/commands/serve.rs`) — production HTTP path; `--enterprise` is the default
  shape for tests/demos, `--local` is the single-user/no-auth laptop mode only.
- `crates/server/src/router/entry.rs` (`oxy-server`'s since decompose LIFT) — top-level router assembly.
- `crates/app-core/src/pods/role_manifest.rs` — the map every new route must be classified in (the test installer is `crates/server/src/role_manifest.rs`).

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
  god files (`admin/apps/handlers.rs`). Prefer a new module over growing one; see
  `internal-docs/domain-boundaries.md`.
