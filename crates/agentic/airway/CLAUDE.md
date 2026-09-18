# agentic-airway

Pattern B subsystem on `agentic-runtime`: queue-driven ELT pipeline
runtime that wraps the external [airway] engine. Sibling of
`agentic-automation` in shape and dependency posture.

[airway]: https://github.com/oxy-hq/airway-internal

## Status

Shipped. The `TaskSpec::Airway` variant, `AirwayEvent` taxonomy, source
and destination factories, the `airway_*` state store/aggregates, and
the real `AirwayWorker` (engine end-to-end, event bridging, load-audit
finalisation) are all in place and exercised by both the HTTP run path
and `oxy airway run`.

## Pipeline

```
TaskSpec::Airway → AirwayWorker.execute() → engine end-to-end → done|failed
```

Single queue row per run. No per-step decisions, no fan-out at the
coordinator. Resource-level fan-out happens inside
`airway::extract_parallel`.

## Aggregates owned

- **WorkspacePipelineState** (`airway_workspace_pipeline_state`) —
  incremental cursor + schema state, keyed by
  `(workspace_id, pipeline_name)`: the same key the lease takes, so a
  run's lease and its cursor can never name different things.
- **PipelineState** (`airway_pipeline_state`) — the legacy row, keyed by
  `pipeline_name` alone. Read once per workspace to adopt, then never
  written again. Retained until every workspace has adopted.
- **LoadAudit** (`airway_load_audit`) — per-extraction audit row,
  keyed by `load_id`, attributed to a workspace.
- **AirwayRunExtension** (`airway_run_extensions`) — per-run metadata
  extending `agentic_runs`.

Migrator: `AirwayMigrator`, tracking table `seaql_migrations_airway`.

## Rules

- Must NOT depend on `oxy`, `oxy-shared`, `entity`, or any other
  platform crate.
- Must NOT depend on `agentic-analytics`, `agentic-builder`,
  `agentic-automation`, `agentic-pipeline`, or `agentic-http`.
- May depend on `agentic-core`, `agentic-runtime`, `agentic-connector`
  (postgres feature, added in stage 3), and the external `airway`
  engine.
- Cross-aggregate refs (`airway_run_extensions.load_id` →
  `airway_load_audit.load_id`) are loose UUIDs — no DB FK to other
  aggregates.
- **`BoxedSourceConnector` must forward every defaulted `SourceConnector`
  method.** Rust does not auto-implement a trait for `Box<dyn Trait>`, so an
  unforwarded method silently resolves to the trait default — accepted,
  compiled, and wrong. It has shipped broken once: `9252a6f56` restored
  `table_name_mappings`, `key_propagation`, `partition_keys`, `sort_keys`,
  `excluded_tables` and `extract_all`. It was also caught pre-merge once, on
  this branch: `32cb6adef` restored 0.1.23's `contracts`, `sandbox_base_url`,
  `contract_for` and `check_contracts` before any of it shipped — that last
  pair would have *inverted* both admission checks, since an empty contract
  map makes `require_declared` refuse the very connectors that declare
  correctly. That it was caught before merge, not after, is evidence the
  check works. `BoxedDestination` carries the same masking class for
  `Destination`'s defaulted methods. When bumping airway, diff the trait's
  method list against `boxed.rs`.
- **Sources are admitted, not just built.** `run_pipeline` calls
  `Source::try_from_connector_with`, never `from_connector` — the latter is
  `-> Self` and so cannot refuse, which is why the policies were dark before
  0.1.23. `environment` is *checked* by admission and *applied* in the source
  factory; both halves are required, since airway resolves sandbox hosts from
  a process-wide global oxy does not install.

## Testing

- Unit tests: `cargo nextest run -p agentic-airway`
- Integration tests need a real Postgres — use testcontainers, never
  the dev DB.
