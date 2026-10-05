---
name: oxy-compile-boundary
description: Use when adding a new YAML entity type to Oxy (e.g. a new file extension under `crates/oxy-compile/src/walker.rs` like `.foo.yml`), when introducing a new runtime read site that walks the workspace filesystem, when wiring a new handler that calls `ConfigManager::resolve_*` or `fs::read_to_string(workspace_path...)`, or any time someone proposes a feature that "just reads from the workspace dir." Also triggers on phrases like "new file extension", "add a YAML config", "load this from disk", "scan the workspace for", "read the YAML file" — the compile boundary expects every NEW workspace artifact to be a row in Postgres, not a per-request FS read.
---

# Compile every workspace artifact to Postgres

Oxy's runtime no longer walks the workspace filesystem on customer-facing requests. PR #2460 (the compile boundary work) made every YAML entity addressable as a `*_definitions` row keyed by `revision_id`. A compile promotes a revision and every read site serves from Postgres until the next compile. The boundary is always on — there are no feature flags; a reader falls through to the filesystem only on a node that owns the files.

**Where this is going (decided 2026-10-05):** the server-side working copy is being removed. Compile becomes its own service that fetches a pushed commit; the filesystem fall-through disappears; the IDE goes. So "it is only read in the IDE" is no longer a reason to skip the boundary — a file the runtime needs and that is not compiled will have nowhere to be read from. Plan: `internal-docs/factory-retirement.md`.

This skill is the rule for **anyone adding a new file type** the runtime needs to read.

## The contract: when you add a new `.foo.yml`

You owe **all five of these** before the feature ships. Skipping any one of them means the new file walks FS on every customer request — exactly what we just stopped doing.

1. **Walker** — add a `FileKind::Foo` variant in `crates/oxy-compile/src/walker.rs` and a glob in `discover()` so a compile pass finds the file. Root-only files go into the explicit `if path.is_file()` block (see `Config` and `MonitorConfig` as templates). Glob patterns go through `push_glob`.

   Your kind inherits two skip rules, and **the working-copy lister must inherit the same two or the file resolves in the IDE and 404s on serve**: any path COMPONENT that is dot-prefixed or `target`/`node_modules`/`dist`/`build` prunes at any depth (`is_skipped`), and any FILE NAME containing `.test.` drops as a fixture. The working-copy half of the second rule is `list_entity_files` in `crates/core/src/config/storage.rs` — a new `list_foos` goes through it, not through `list_by_sub_extension` directly. Both rules and why the walker wins ties: `internal-docs/compile-boundary.md` § "What a workspace enumerates".

2. **Compile output** — add `CompiledRow::Foo(CompiledFoo { ... })` in `crates/oxy-compile/src/compile.rs`, a match arm in `compile_one()`, and a `row_dedupe_key` entry (or `None` if the file is a singleton like `config.yml` / `.monitor.yml`). For named entities (agents, views, topics) use `compile_named_yaml` — it handles the `name`/`file_path`/`definition` shape uniformly. For singletons, write a small `compile_foo()` that parses and emits one row.

3. **Schema** — append the new table to the squashed migration `crates/migration/src/m20260606_000002_create_compile_boundary.rs` (BOTH `up()` and the reverse-order `down()`). Add a `MonitorConfigs`-style `DeriveIden` enum at the end of the file. The FK convention is `from(Foo::Table, Foo::RevisionId).to(Revisions::Table, Revisions::RevisionId).on_delete(ForeignKeyAction::Cascade)`. Add the matching entity file under `crates/entity/src/foo_definitions.rs` and register it in `crates/entity/src/lib.rs`.

4. **Writer** — add a `CompiledRow::Foo(f) => foos.push(...)` arm in `crates/oxy-compile/src/writer.rs`, declare the `let mut foos = Vec::new()` near the top of the function, and add the bulk-insert call near the end. The order mirrors the existing kinds.

5. **Reader + handler wiring** — add the read to `ConfigManager` (`crates/core/src/config/manager.rs`), **not** to `compiled_reader.rs`. `ConfigManager` matches on the request's `Origin` once (the compiled revision, or the disk through `ConfigManager::disk()` on a node that owns files) and returns a typed `ArtifactError` instead of an empty list; handlers call it and never choose a backend. `crates/app/tests/platform/compiled_reader_is_not_a_back_door.rs` fails the build if a handler reaches `compiled_reader` directly, and `artifact_reads_reach_the_disk_through_one_door` (`crates/core/tests/config_manager_fs_boundary.rs`) does the same for a read that reaches the working copy outside `disk()`. Then wire the runtime handler:
   - If the handler accepts a string / struct: ask `ConfigManager`, deserialise from the JSONB definition. "Not compiled yet" must answer **retryable** (503 + a lazy compile), distinct from not-found.
   - If the handler accepts a `Path` (e.g. an external library like `airlayer` or `oxy_metric_monitoring`): use the materialiser in `crates/core/src/config/scan.rs` (called from `crates/app/src/server/api/semantic_scan.rs`) — it writes the compiled rows to a `tempfile::TempDir`, hands over that path, and you hold the guard until the call returns.

The revision a request reads is resolved once by the workspace middleware and pinned for the whole request — you do not re-resolve it per surface. Full rules: `internal-docs/workspace-source.md`.

### S3 blob storage (semantic views / topics only)

Large semantic view / topic bodies move to S3 when `OXY_COMPILE_BLOB_S3_BUCKET` is set. The compile worker uploads each body to `s3://<bucket>/workspaces/<workspace_id>/{semantic_views,semantic_topics}/<name>-<sha[..32]>.yml` and stores the key in `semantic_views.compiled_sql_blob_key` / `semantic_topics.compiled_sql_blob_key`. The materialiser (`crates/app/src/server/api/semantic_scan.rs`) prefers the S3 blob over the in-row JSONB. When the env var is unset, blob_key is NULL and the in-row `definition` is the canonical body — Postgres-only deployments work unchanged.

If you add a new entity type whose definition routinely tops tens of KB (the way semantic views do), follow the same shape: add a nullable `compiled_sql_blob_key` column, wire the upload in `oxy-compile`'s writer, and extend `oxy_compile::blob_store::BlobKind`. Small definitions (apps, agents, automations) stay JSONB-only — the S3 round-trip costs more than the row bloat at typical sizes.

## Why this matters

The original FS read pattern was the dominant runtime cost at any meaningful workspace count. Serve and worker pods mount no volume, so they must never read workspace files; the one pod that still has them (the Factory) is being retired. Every new file type you skip the compile boundary on is a new failure mode under multi-instance, and a new reason the Factory cannot be removed.

## What's NOT in scope

Skip this contract only when:
- The artifact is a generated build product (charts, parquet caches, custom-app bundles) — those belong in S3, not the compile boundary.
- The read happens exactly once at server startup, not per-request (startup walks of `OXY_STATE_DIR` are fine).

If you find yourself adding a `glob::glob(workspace_path)` or `fs::read_to_string(workspace_path.join(...))` in a request handler and the path is not already a compiled entity, you are on the wrong path. Open this skill and add the file type to the compile boundary first.

### Absent is not empty (and two rules that keep it that way)

A fallback that reads a filesystem which is not there must fail, not return nothing. Every enumeration that returned `[]` for a missing workspace root is how a platform-side miss got reported as the customer's configuration — both shipped incidents are that sentence.

1. **A new lister goes through `require_root()`** (`crates/core/src/config/storage.rs`). It turns a missing workspace root into an `Err`. The seven existing listers call it; an eighth that forgets is a silent regression, not a compile error.
2. **Nothing on a manager-construction path may create a directory.** `WorkingCopy::new` once resolved `<root>/.oxy_state` through a resolver that creates what it resolves, so merely building a manager brought the workspace root into existence. That defeated every other guard here, including ones written specifically to catch it — by the time anything stat-ed the root, it was there. Resolve with `oxy_shared::state_dir::state_dir_path`; create only where you write.

`crates/app/tests/platform/walker_storage_divergence.rs` pins both. Full account: `internal-docs/compile-boundary.md` § "Why an absent working copy used to be undiagnosable".

## Related skills + design docs

- `oxy-scaling-design` — broader multi-instance context.
- `oxy-task-spec-default` — long-running work goes on the worker fleet, not in handlers.
- `internal-docs/compile-boundary.md` — operator runbook (flags, kill switch, read routing, code map).
- `internal-docs/factory-retirement.md` — the plan to compile from git and remove the working copy; `internal-docs/workspace-source.md` — the rules for where a read may come from.
- The compile worker entry point: `crates/app/src/server/compile_worker.rs`; the dispatcher: `crates/app/src/agentic_wiring/compile_dispatcher.rs`.
- The one read door: `ConfigManager` in `crates/core/src/config/manager.rs`. `crates/app/src/server/api/compiled_reader.rs` only resolves which revision a request is pinned to; handlers do not call it.
- The materialiser: `crates/core/src/config/scan.rs`, called from `crates/app/src/server/api/semantic_scan.rs`.
