Focus: where data is stored (Oxy's Postgres is the control plane only)

Read first: `.claude/skills/oxy-data-placement/SKILL.md` and `internal-docs/data-placement.md`.

Check:
- A new table or migration in Oxy's Postgres that holds org data (facts and history,
  records an app edits, files, events, webhook payloads) instead of the workspace's
  Airhouse, the org OLTP database or storage.
- A table a migrator creates that is not placed in
  `crates/workspace-runtime/tests/integration/data_placement.rs`.
- A write to a customer warehouse. Those are read-only to apps unless
  `customerWarehouseWrites` lists them.
- OLTP provisioning automated where it must stay manual.
- Airhouse (DuckLake) DDL with `CREATE INDEX`, `PRIMARY KEY` or `UNIQUE`: one such table
  makes the writer go inert.
