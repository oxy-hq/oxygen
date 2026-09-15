---
name: oxy-data-placement
description: Use when deciding where data lives — adding a table or migration in crates/migration, storing anything an org's systems or people produce (webhook payloads, events, POS orders, app state, documents, uploads), adding a custom-app data surface, or fixing a failing data_placement test. Triggers include "new table", "add a migration", "store these events", "persist this", "where should this data go", "webhook writes", "ctx.oltp or airhouse", "control plane", "data plane".
---

# Data placement: the shape of the data picks the store

Oxy is the **control plane**; each org is its own **data plane**. Full rule,
enforcement and backlog: `internal-docs/data-placement.md`. This skill is the
decision, not the reference.

## Oxy's Postgres holds only what runs the platform

Identity and access, configuration (compiled definitions, secret references),
what Oxy is doing (queue, leases, cursors, schedules), billing, audit, and
signals that carry **ids, never content**.

Two questions decide a table:

1. If this org left, would they expect to take these rows? **Yes → data plane.**
2. Does Oxy need it before it knows which org is involved (routing, authz,
   scheduling)? **Yes → control plane.**

Both yes → split the row: the part Oxy routes on stays, the content moves.

## Everything else: four shapes, four stores

| Shape | Store | Custom-app surface | Platform (Rust) path |
| --- | --- | --- | --- |
| **Facts** — happened, won't change: an order, a completed checklist, a reading | Workspace **Airhouse** (DuckLake) | `ctx.airhouse.append / query / exec` into `app_<writer>`; tables via `airhouseMigrations` | Airway pipeline for external sources; a system-purpose Airhouse client for ingest (`crates/cameras/src/airhouse/mod.rs`) |
| **Records** an app edits — current state with constraints | Org **OLTP** Postgres | `ctx.oltp.query / exec / tx`; tables via `migrations` | `oxy_oltp` writers: `app:<slug>` → `app_<slug>`, `pipeline:<src>` → `raw_<src>` |
| **Files** — bytes | App storage (S3) | `ctx.storage` | object store, key in a record or fact |
| **Customer warehouse** — theirs | Theirs | `ctx.warehouse.query`; writes refused unless `customerWarehouseWrites` names the database with a reason | read only |

Picking: **bytes?** → storage. **Changes after it's written?** No → fact; yes →
record; both when history matters (record for "true now", fact for "what
changed"). **Already in the customer's warehouse?** → read it there; copy it
with Airway if it must join facts.

## Non-negotiables

- **A new table in `crates/migration` must be placed** in
  `crates/app/tests/platform/data_placement.rs`. `Control("why")` only when the
  rule above says so. `Backlog { holds, belongs }` is a reviewable admission
  that org data is landing in Oxy's database — challenge it, don't rubber-stamp it.
- **No automatic OLTP provisioning.** A store is a paid provider resource,
  provisioned per org by an operator (`oxyc oltp provision --org <org> --writer app:<slug>`).
  Code that meets a missing store fails with a message naming that step.
- **DuckLake has no PRIMARY KEY, UNIQUE, indexes or foreign keys.** A table
  carrying one fails and leaves its writer inert. Facts carry their source's id
  and `recorded_at`; readers keep one row per id
  (`QUALIFY row_number() OVER (PARTITION BY <id> ORDER BY recorded_at DESC) = 1`);
  corrections are new facts.
- **An app's stores are written as the app**, not the caller (`ctx.oltp`,
  `ctx.airhouse`), and confined to its own `app_<writer>` schema — by Postgres
  grants for OLTP, by `airhouse::sql_rules` on the host plus a scoped
  credential (`write_schemas`) for Airhouse.
- **`ctx.secrets` is for credentials, not state.** A cursor or counter is a record.

## Red flags

| Thought | Reality |
| --- | --- |
| "It's just a small events table next to the feature" | That is how `world_model_events` came to hold every org's POS orders. Facts → Airhouse. |
| "I'll add it as Backlog for now" | Backlog should only shrink. Put it in the data plane from day one. |
| "Store the payload so the UI can show it" | Store an id in the signal; read the content from the org's store. |
| "Upsert into Airhouse on the natural key" | No keys in DuckLake. Append, dedupe on read. |
| "Auto-provision the org's database when the first event arrives" | Rejected: provisioning stays manual and visible. |
| "Write it back to their ClickHouse, it's already connected" | Customer warehouses are read-only; an exception needs a written reason in the manifest. |

## References

- `internal-docs/data-placement.md` — rule, enforcement table, backlog
- `internal-docs/per-org-oltp-postgres.md` — OLTP writers, visibility, provisioning
- `internal-docs/airhouse-integration.md` — Airhouse credentials and the broker
- `sdk/cli/skills/oxy-custom-apps/SKILL.md` — the app author's version ("Where data goes")
