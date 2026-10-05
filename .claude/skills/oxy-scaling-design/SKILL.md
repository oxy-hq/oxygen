---
name: oxy-scaling-design
description: Use when the user asks about Oxy's multi-instance scaling, the split fleet, worker fleet, horizontal scaling, high availability, how the serve/ide/worker roles divide work, or retiring the Factory / running without a server-side working copy. Triggers include "scale Oxy", "scale oxygen", "multi-instance", "split fleet", "worker fleet", "horizontal scaling", "high availability", "OXY_ROLE", "stateful vs HA", "compile boundary", "stateless serving", "durable execution", "shard workspaces", "ephemeral environments", "internal jobs admin", "retire the Factory", "remove the IDE", "compile service", "compile from git", "no filesystem", "diskless".
---

# Oxy multi-instance scaling — quick reference

**The current architecture in one line:** Oxy runs as a **split fleet** keyed by `OXY_ROLE` — a single stateful `ide` (the Factory: working copy + `.git` + compile + chat-run execution), a horizontally-scaled stateless `serve` fleet (reads Postgres + S3 only), and a `worker` queue drainer. This is a Postgres primary/read-replica shape: the `ide` is the primary, the serve fleet is the read replicas, and the **compile boundary** (compiled `*_definitions` rows + S3 blobs, keyed by `revision_id`) is the replicated read model.

**The direction (decided 2026-10-05): the Factory is being retired, not scaled.** No IDE; content is authored in git and pushed; compile becomes its own service that fetches a commit and serves previews and sandboxes; chat and every other surface read the compiled revision only; workspaces with no git remote go. Design new work so it needs no server-side working copy — a change that adds a reason to keep the Factory is going the wrong way. Plan, production data and open decisions: `internal-docs/factory-retirement.md`.

**Read these first when grounding a decision:**
- `internal-docs/multi-instance-fleet.md` — how the fleet works today (roles, the stateful-vs-HA matrix, route classification, the `super_read_only` guard, graceful degradation, code map). **The primary reference.**
- `internal-docs/factory-retirement.md` — the forward plan: phases, exit tests, and the existing statements it overturns.
- `internal-docs/multi-instance-fleet.md` — the original phase ledger + the rejected-alternatives list.
- `internal-docs/compile-boundary.md` — operator runbook.

## What is built (the current reality)

- **Split fleet via `OXY_ROLE`** (`ide | serve | worker | all`). One knob selects the topology; everything else derives from it. A `serve` replica derives in-process workers + the periodic global driver OFF; every other role ON. The legacy flags (`OXY_DISABLE_INPROCESS_WORKERS`, `OXY_INPROC_GLOBAL_WORKER`, `--no-workers`) survive only as two-directional overrides.
- **Compile boundary (compile-complete stateless serving).** The `ide` compiles the working copy → Postgres `*_definitions` rows + S3 blobs per `revision_id` → promotes via `workspaces.current_revision_id`. The serve fleet reads the promoted revision and **never walks the workspace FS**. This is why the read path needs no working copy — and therefore no per-request clone.
- **Self-routing.** `role_manifest::classify` is the single routing authority; a serve replica reverse-proxies `IdeOnly` requests to `OXY_IDE_UPSTREAM` (`ide_proxy`). Replaces the drift-prone external route table that caused three outages.
- **Route classification + HA carve-out.** Every route states its pod at the mount — `route_ide` (FS/exec/live-stream) or `route_fleet` (Postgres/S3 reads stay HA) — and the two take differently-typed handlers, so a working-copy handler cannot compile onto a fleet route. Plus drift tests and the `super_read_only` runtime guard. See the `oxy-route-classification` skill.
- **Runtime-artifact S3 mirror** (`runtime_artifact.rs`) — charts/results/app-data mirror to the compile-boundary bucket so any replica serves them; ide-down charts degrade to the S3 mirror.
- **Schedules/monitors fire without a leader.** The periodic global driver runs on every eligible node; `tick_schedules`/`tick_monitor_schedules` CAS-advance `next_run_at` so firing is exactly-once across replicas. (Leader election was tried and removed — the CAS already guarantees it, and running on all eligible nodes is better HA.)
- **Backpressure** — admission control (global ceiling + per-tenant fairness, 503 + `Retry-After`), worker HPA on outstanding work (queued + claimed) against capacity — see the recipe in `internal-docs/worker-fleet.md`; queue depth alone goes absent when the fleet keeps up.
- **Migrations** — a dedicated migrate Job owns the schema; `serve`/`compile`/`worker` honour `OXY_SKIP_MIGRATIONS`; a startup advisory lock serialises co-booting nodes.
- **Main traffic off the Factory (October 2026).** Custom-app function calls run on any replica, with a handler-level replay to the Factory for a workspace with nothing compiled or a database that is a file in the checkout. Airway start, single-window backfill, cancel, live stream and both resets are served by any replica. Custom-app procedure runs execute on the task queue, at most once, with an execution heartbeat. `OXY_IDE_DEFER_QUEUE_WORK` (off by default) makes the Factory drive only compiles and take anything else after 30 s unclaimed.

## What still needs the Factory

128 routes on 2026-10-05, inventoried by feature in `internal-docs/factory-retirement.md` §1. In short: file editing, git and the rest of the IDE; compile, and creating or refreshing a preview; **starting a chat** (the largest remaining piece of real traffic); Data App runs; workspaces whose database is a file in the repo; and workspaces with no git remote, for which the Factory's volume is the only copy.

## What is pending

- **The seven phases of `factory-retirement.md`**: compile from git on a compile pool, previews and sandboxes on the same service, chat off the filesystem and onto the queue, every other surface reading the revision or being removed, the IDE removed, no remote-less workspaces, the Factory deleted.
- **Durable execution** — replay-deterministic agentic-runtime orchestrator (independent).
- **Generated artifacts → S3** — charts/results/app-data already mirror via `runtime_artifact`; pre-aggregation rollups and the enum index are still written locally first.

Superseded, do not build: the `WorkspaceFs` crate reorg, the serve-binary split, and the pool / per-workspace environment tiers of `ephemeral-workspace-environments.md`. There will be no server-side working copy to put behind a port.

## Hard constraints (don't violate)

1. **Code-first is sacred** — definitions are files in a git repository (like dbt). Agents/automations/apps/semantic views live as YAML in git. **Never introduce a parallel source of truth** (S3 snapshots, DB-backed file storage of definitions). Generated artifacts are different — those go to S3.
2. **Git is the source of truth** — GitHub origin in cloud, a local directory in single-process mode. What compiles is a pushed commit. The Factory's local disk is a cache of origin while the Factory still exists; nothing new may treat it as a place to author or keep content.
3. **HTTP is stateless beyond the request** — anything a serve replica needs must be in Postgres, S3, or reconstructable from origin. Long-running work is a row on the task queue, not a spawn in a handler.
4. **One fencing primitive for the Factory:** the StatefulSet `replicas: 1` at-most-one guarantee. There is **no workspace-ownership lease** (it was built and reverted 2026-06-14 — at replicas=1 it guards a multi-producer race that can't occur) and **no leader election** (the `next_run_at` CAS gives exactly-once). The task-claim lease in `agentic_task_queue` is the worker's, and is unrelated. Compile needs no fence: the unique index on `(workspace_id, git_sha)`, the `superseded` status and the promote compare-and-set already make several compilers safe.

## What was explicitly rejected (don't re-debate)

- **Smart cloning** for the serve/worker read path (partial/shallow/sparse clone + LRU clone cache + mirror updater). The compile boundary makes the read path need no working copy, so it needs no clone. The compile service does not reopen this: it downloads one commit as a tarball into a temporary directory and keeps no clone and no clone cache.
- Sourcegraph gitserver — license flipped proprietary; dead upstream.
- Gitaly + Praefect — assumes GitLab Rails as auth source; nobody runs it standalone.
- Mononoke (Meta) — GPL-2.0, no outside production deployments, exotic build.
- libgit2 / git2-rs — not adopted. (No `gix` is in the tree either; git access is the `git` CLI in `crates/git` plus the GitHub REST API, and `crates/git` goes away with the IDE.)
- Apalis / Hatchet / Temporal / River / pgmq — Oxy has its own orchestrator; no parallel queue framework.
- S3 snapshot as workspace truth — violates code-first; sync nightmare with git.
- EFS/NFS shared filesystem — git over network FS is fragile.

## When this skill applies

- "How do we scale Oxy?" / "is it HA?" → this skill + `multi-instance-fleet.md`.
- A change that touches role split, the worker fleet, the compile boundary, or workspace ownership → ground it here, then read the relevant doc before implementing.
- "Why does X exist / why not Y?" → trace to the constraints + rejected list above.
- A route/handler change → defer to the `oxy-route-classification` skill (IdeOnly vs FleetOk vs the HA carve-out).
- Long-running/background work → the `oxy-task-spec-default` skill (TaskSpec on the worker fleet, not `tokio::spawn` in a handler).

## Refs

- Fleet guide (primary): `internal-docs/multi-instance-fleet.md`
- Forward plan: `internal-docs/factory-retirement.md` (supersedes `ephemeral-workspace-environments.md`)
- Phase ledger + rejected alternatives: `internal-docs/multi-instance-fleet.md`
- Operator runbook: `internal-docs/compile-boundary.md`
- Worker fleet dev guide: `internal-docs/worker-fleet.md`; scope survey: `internal-docs/worker-fleet.md`
- Backend architecture rules: `internal-docs/backend-architecture.md`
