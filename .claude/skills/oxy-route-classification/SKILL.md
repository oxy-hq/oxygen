---
name: oxy-route-classification
description: Use when adding, moving, or renaming an HTTP route under `crates/app/src/server/router/` (any `.route(...)` / `.nest(...)` mount), or writing a request handler that reads the workspace working copy, `.git`, or the local state dir (`config_manager.workspace_path()`, `ConfigManager::resolve_*`, `resolve_state_dir()`, `fs::read*(workspace_path…)`, a `glob` of the workspace dir, a `GitClient`). Also triggers on "add an endpoint", "new API route", "421", "x-oxy-required-role", "served on serve but the file isn't there", "role_manifest", "RoleRouter", "route_ide", "route_fleet", "IdeState", "IdeOnly", "self-routing", and the HIGH-AVAILABILITY side: "high availability", "thread/conversation won't load when the ide is down", "a read is pinned to the singleton", "viewing needs the factory". Every route that touches node-local disk MUST be classified IdeOnly or it 404s/421s on the stateless serve fleet — and conversely, every persisted-data READ must stay FleetOk (any replica) or HA hinges on one instance.
---

# Classify every workspace-FS route IdeOnly

Oxy runs as a split fleet (see `internal-docs/multi-instance-fleet.md`):

- **ide** — a singleton StatefulSet that owns the workspace working copy + `.git` + the local state dir.
- **serve** — a stateless fleet of replicas reading Postgres + S3 only. **No working copy.**
- **worker** — a TaskSpec drainer.

**Direction (decided 2026-10-05): `IdeOnly` is debt, not a home.** The Factory is being retired (`internal-docs/factory-retirement.md`), so the count of `IdeOnly` routes has to reach zero. Before adding one, ask whether the handler can read the compiled revision instead (`oxy-compile-boundary` skill) or run as a queued task (`oxy-task-spec-default` skill). Mount `route_ide` only for something that is going away with the IDE, or that the plan has a phase for.

**A route states its role where it is mounted.** `RoleRouter` (`crates/app/src/server/router/role_router.rs`) has no plain `.route()`: every mount goes through `route_ide` or `route_fleet`, and the declaration it records is what `role_manifest::classify` reads at runtime. `enforce_role` then reverse-proxies an `IdeOnly` request that lands on a serve replica.

The two doors take differently-typed handlers, and that is the real guard. `WorkspaceManagerWorkingCopy` resolves only from `IdeState`, so a handler that asks for a working copy produces a `MethodRouter<IdeState>` and **will not compile** through `route_fleet`:

```
error[E0308]: expected `MethodRouter<FleetState>`, found `MethodRouter<IdeState>`
```

This replaced a hand-written path table where an unlisted route silently defaulted to `FleetOk`. That default is how `GET /apps/source/<file>` shipped: `get_source_file` reads `workspace_path()`, nobody added the entry, and it returned `404 File not found` from `serve@…` (no `x-oxy-forwarded-via`). A route can no longer be unlisted — there is no mount that does not declare.

## The rule

**If a handler reads or writes node-local disk, its route MUST be mounted with `route_ide`.** Node-local disk =

- the **workspace working copy** — `config_manager.workspace_path()`, `ConfigManager::resolve_*`, `fs::read_to_string(workspace_path…)`, `glob::glob(workspace_path…)`;
- **`.git`** — any `GitClient` / shelling out to git;
- the **local state dir** — `resolve_state_dir()` (the serve env strips `OXY_STATE_DIR`).

If the handler reads only Postgres / S3 / the compile boundary / an LLM, leave it `FleetOk` (the default) — that's what keeps the fleet horizontally scalable. **When unsure, choose IdeOnly**: a wasted hop to the ide beats a 404 on the fleet.

## The recipe

1. **Mount it through the right door.** `route_ide(path, method_router)` if the handler reaches node-local disk, `route_fleet(path, method_router)` if it reads only persisted data. Nothing else to write: the declaration is the mount, and the path is relative to the builder — nesting prefixes it.
2. **Let the compiler check you.** Guess `route_fleet` for a handler that takes `WorkspaceManagerWorkingCopy` and the build fails with the E0308 above. The reverse is not checked — `route_ide` compiles for anything — so an IdeOnly guess costs a wasted hop, which is why it is the safe guess when unsure.
3. **No external route table to touch.** A serve replica self-proxies an `IdeOnly` request to the Factory via `ide_proxy` (`OXY_IDE_UPSTREAM`). There is no ALB/Envoy `ideRoutes` list to keep in sync (that drift-prone table caused three outages and was removed).

### The two doors that are not those

- **`route_split(path, ide_method, ide, fleet_method, fleet)`** — one path whose verbs sit on opposite sides. `GET /databases` degrades without a working copy and the launcher calls it on every page load; `POST /databases` writes `config.yml`.
- **There is no `route_fleet_optional_working_copy` any more.** It used to admit a fleet handler that held a working copy for a fallback arm; all of them were converted, and `the_optional_working_copy_door_stays_shut` (`role_manifest_tests.rs`) fails the build if it comes back. A boundary-first handler takes `WorkspaceManagerReadOnly` and asks `ConfigManager`, whose `disk()` arm only answers on a node that owns files.

## The other half: reads must stay HA — even under an IdeOnly wildcard

The split exists so that **reads scale and survive the ide going down**; only writes + live execution are pinned to the singleton. So the rule has a second half:

**A handler that reads only persisted data (Postgres / S3 / compile boundary) MUST be served on the fleet (`FleetOk`) — even when it sits under a broad `IdeOnly` `{*rest}` wildcard.** Otherwise *viewing* data needs the singleton and dies when it restarts — the HA bug this design exists to prevent, in reverse.

Some surfaces are pinned IdeOnly with a wildcard for *execution* safety — `/analytics/{*rest}`, `/agentic-workflows/{*rest}`, `/agentic-airway/{*rest}` (a live run executes in-process on the ide and touches the FS). That wildcard also swallows the **run-history READS** sitting next to the execution endpoints (`list_runs_by_thread`, `get_workflow_run`, `list_runs_for_pipeline`, …), which are pure `state.db` reads. Loading a thread is *data*, not a live run — pinning it to the ide made conversations unviewable whenever the ide restarted.

**Recipe — carve the read back out to FleetOk:** a sub-router crate declares its own routes' roles beside them (`agentic_http::router_roles()` and its two siblings), and `oxy-app` mounts them with `nest_declared`, which prefixes the declarations exactly as it prefixes the routes. So the carve-out is a `RouteRoleDecl { method: "GET", path, role: RouteRole::FleetOk }` next to the route in the crate that owns it — not an entry written from outside.

**STRICT RULE (the const carries it too):** only carve out after verifying the handler touches **no** workspace FS / `.git` / local state dir and is **not** a live in-process stream — a pure Postgres / S3 read. Anything that executes, edits, or streams a live run stays `IdeOnly`. Mind the segment counts: the live `/runs/{id}/events` SSE has one MORE segment than a carved-out `/runs/{id}`, so it isn't shadowed — assert both directions in a test (see `agentic_run_history_reads_are_fleet_ok`).

**Mental model:** writes + execution + live streams → `IdeOnly` (the singleton). Reads of persisted data → `FleetOk` (any replica). If loading or *viewing* something needs the ide, that's an HA bug — find the read and carve it out. Full stateful-vs-HA matrix: `internal-docs/multi-instance-fleet.md` §4.

## The type parameter tells you which extractor — not which role

A handler's manager carries the same fact per *pod* that the manifest carries per *route*, and the two are easy to conflate.

- **Take `WorkspaceManagerReadOnly` by default.** It yields `WorkspaceManager<ReadOnly>`, so the compiler refuses `workspace_path()`, `resolve_state_dir()` and the file walks. If it compiles, the handler didn't need a disk and the manifest question is settled — leave the route `FleetOk`.
- **`WorkspaceManagerWorkingCopy` yields `WorkspaceManager<WorkingCopy>`.** Reaching for it is the signal to write the `IdeOnly` entry.
- **Check the bound before you accept it.** A `&ConfigManager<WorkingCopy>` in a callee is frequently over-constrained. If its body only calls methods on the generic `impl<S> ConfigManager<S>`, or needs `ResolveWorkspaceFile` (which has an impl for each capability), state the weaker bound and both satisfy it. Grep can't see a requirement that lives in a callee; the compiler can, and half of what it objects to is a bound nobody meant to write.

**Do not classify by signature.** It was measured: `takes WorkspaceManagerWorkingCopy ⇒ IdeOnly` gets 62 of 92 entries right, and fails **open** on the whole `/analytics` + `/agentic-*` surface because `agentic-http` sits below `oxy-app` and structurally cannot take the extractor. `WorkingCopy` is a *permission* type — `WorkspaceManagerWorkingCopy` resolves fine on a diskless replica — so it does not answer "which pod". Disk is also not the only reason to pin: `/events` correctly takes `WorkspaceManagerReadOnly` and must still be IdeOnly, because it subscribes to a process-local broadcaster. Classify by hand, and see `internal-docs/workspace-source.md` for the full count.

## Two counters that catch what this skill misses

A static list can only cover the routes someone remembered. Both of these are zero on a healthy fleet and asserted in `crates/app/tests/routing/fleet_canary.rs`:

- `oxy::workspace_fs_probe::leaks()` — a workspace path was resolved on a pod that owns no working copy.
- `compiled_reader::branch_hints_dropped()` — a request arrived with `?branch=` that a replica cannot honour. **A branch-aware route must be `IdeOnly`**: a replica skips the branch gate by design and answers with the promoted `main` revision, so the caller gets the wrong branch with no error. Non-zero names the misclassified route in the WARN. (`enforce_role` also escalates any `FleetOk` request carrying a non-empty `?branch=` to the Factory, previews excepted — so a client that sends the hint out of habit pins its own traffic there. Don't send it from a surface that is not the IDE.)

## What's NOT in scope

- A route that reads only the compile boundary / Postgres / S3 — `FleetOk` by design; do **not** pin it to the singleton. If it 404s on serve because the data isn't compiled, the fix is the compile boundary (`oxy-compile-boundary` skill), not IdeOnly. If it's a read inside a sub-router crate's IdeOnly surface, declare it FleetOk beside the route in that crate (above).
- Pure IDE-editor / git-write routes — already IdeOnly; nothing to do.
- Routes `oxy-app` does not mount itself. A merge at a tree's ROOT has no prefix to hang a declaration on, so `oxy-cameras` and `airhouse` go through `merge_undeclared` with a reason and fall to `classify`'s default. That is the one hole left; if you add a working-copy route there, nothing catches it.

## Related

- `internal-docs/multi-instance-fleet.md` — the fleet guide (model, the stateful-vs-HA matrix, self-routing, `super_read_only`, code map).
- `oxy-compile-boundary` skill — when the right fix is "compile it to Postgres" instead of "pin it to the ide".
- `internal-docs/factory-retirement.md` — the plan to take the `IdeOnly` count to zero, with the full inventory of what is pinned and why.
- `role_manifest.rs` module docs — the segment-by-segment matching semantics. Its tests live in `role_manifest_tests.rs`.
