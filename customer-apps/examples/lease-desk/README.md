# Lease Desk: write a record once, get its history in Airhouse

A property manager's lease desk. The app keeps one row per lease in its own
OLTP store and changes it in place: a lease is signed, its rent changes, it
ends. That is all the app does. It keeps no history table, and it publishes no
events.

Because `oxy-app.json` declares `leases` under `oltpChanges`, the platform
ships every change of that table to the workspace's Airhouse. Analytics get two
things from the one write:

- **Every version of every lease**, so a question like "what was the rent of
  4B on 30 June?" has an answer.
- **The leases as they stand now**, the same rows the app sees.

## The declaration

```json
"migrations": { "dir": "migrations" },
"oltpChanges": {
  "tables": {
    "leases": { "exclude": ["tenant_email"] }
  }
}
```

- `tables` names the tables to capture, in the app's own schema
  (`app_lease_desk`). The table needs a primary key: it is the row's identity
  in Airhouse.
- `exclude` lists columns that never leave the store. A tenant's email is
  personal data: it is in the app's `leases`, and in neither Airhouse table
  below.

[`migrations/0001_leases.sql`](migrations/0001_leases.sql) creates the table,
and [`functions/leases.js`](functions/leases.js) writes it with `ctx.oltp`
(`sign`, `set-rent`, `end`, `void` and `list`).

## Setup

1. The org operator provisions the store once:

   ```sh
   oxyc oltp provision --org <org> --writer app:lease_desk
   ```

2. Publish and promote:

   ```sh
   oxyc publish --env production --promote
   ```

   The promote applies the migration and installs capture on `leases`. It
   copies the rows already there, so the first batch Airhouse receives is the
   table as it stood when capture began.

The change stream ships production writes only, and only for an org with the
`oltp_changes` flag on.

## What lands in Airhouse

In the schema `oltp_lease_desk`:

| Table | What it holds |
| --- | --- |
| `leases__changes` | Every change to a lease: its captured columns, then `_op` (`insert`, `update`, `delete`, or `snapshot` for a row the platform copied as it stood), `_generation` and `_seq` (together, the order), `_changed_at`, `_actor` (who made the change) and `_invocation_id` (the function run that made it). |
| `leases` | A view: each lease as it stands now. A voided lease is gone from it. |

The platform copies a table's rows as `snapshot`s when capture begins, and
again after a repair or a restore of the store (which also starts a new
`_generation`). A history query reads `insert` and `update` for what the app
did, and orders by `_generation, _seq`.

The rent roll today:

```sql
SELECT unit, tenant_name, rent_cents
  FROM oltp_lease_desk.leases
 WHERE ended_on IS NULL
 ORDER BY unit;
```

Every rent a lease has had, and who set it:

```sql
SELECT _changed_at, rent_cents, _actor
  FROM oltp_lease_desk.leases__changes
 WHERE lease_ref = 'L-4B' AND _op IN ('insert', 'update')
 ORDER BY _generation, _seq;
```

Each lease as it stood on a date:

```sql
SELECT lease_ref, unit, rent_cents
  FROM oltp_lease_desk.leases__changes
 WHERE _changed_at < TIMESTAMPTZ '2026-07-01'
QUALIFY row_number() OVER (PARTITION BY lease_ref ORDER BY _generation DESC, _seq DESC) = 1
    AND _op <> 'delete';
```

## Tested end to end

`crates/api-custom-apps/tests/integration/oltp_changes_relay/lease_desk.rs`
publishes this directory's manifest, migration and function. It then calls the
function through the serve route and runs the relay. It checks the history,
the current state, the excluded column, and the app beside a second app in the
same org.
