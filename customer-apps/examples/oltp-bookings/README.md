# OLTP Bookings — `ctx.oltp` example

A minimal custom app that reads and **writes** its own per-org OLTP store through
`ctx.oltp`. It's the smallest complete demonstration of the app-writer path: a
booking form that inserts rows the app owns, and lists them back.

## Why `ctx.oltp` and not `ctx.warehouse`

`ctx.warehouse` targets the databases the *project* analyses. For a
`postgres_managed` database it resolves the org's **read-only analyst**, so:

- a write authenticates and then fails `permission denied`, and
- a read sees the org's whole warehouse — the `raw_*` Toast/QuickBooks extracts
  included, which is wider than an app should have.

`ctx.oltp` resolves the app's own **writer** role instead. Its DML rights are
scoped to the single `app_<writer>` schema (`app_oltp_bookings` here) and nothing
else — narrower on reads, and finally writable. The app never names a database:
its own store is implicit.

## The capability

`oxy-app.json` declares it on the function, fail-closed:

```json
"functions": {
  "bookings": { "route": true, "oltp": { "enabled": true } }
}
```

`enabled` is a pure **gate** — it does not name a schema. The target is derived
from the app's own slug host-side (`oltp-bookings` → `app_oltp_bookings`), so a
manifest can never point `ctx.oltp` at another app's data. Omit the capability
and every `ctx.oltp` call is rejected before any connection is opened.

## Setup

1. **The org operator provisions the store once** (mints the
   `app_oltp_bookings_rw` role + schema — the writer name is the app slug with
   hyphens as underscores):

   ```sh
   oxyc oltp provision --org <org> --writer app:oltp_bookings
   ```

2. **Publish the app:**

   ```sh
   oxyc publish
   ```

Per-org OLTP is always on, so step 1 is the only precondition: until the store
is provisioned, `ctx.oltp` fails with an error naming the missing database or
writer rather than reaching anything.

## Using it

[`functions/bookings.ts`](functions/bookings.ts) handles one route:

- `{ "action": "list" }` → the 100 most recent bookings.
- `{ "action": "create", "name": "Ada", "partySize": 4 }` → inserts one row,
  returns it (`INSERT … RETURNING`), and tells the hosts (below).

From the app's frontend:

```ts
const { invoke } = useFunction("bookings");
await invoke({ action: "create", name: "Ada", partySize: 4 });
const { bookings } = await invoke({ action: "list" });
```

## Telling people about a booking — `ctx.notify`

A created booking is announced to whoever holds the **Host** position and to the
person who took it:

```ts
await ctx.notify({
  to: [{ role: "Host" }, { userId: ctx.user.id }],
  kind: "booking_created",
  title: `New booking: ${name}, party of ${partySize}`,
  link: "/",
  email: true,
  idempotencyKey: `booking:${row.id}`,
});
```

The booking is committed before the notify, so the function catches a refused
notify and still answers `201`; a client retrying a `500` would book twice.

The function names people, never addresses. The platform resolves them (the
app's audience only), writes each one an inbox row in Oxygen, and pushes or
emails them where it can — including frontline crew, who have no email. The
capability declares the kind, which is what a person can mute:

```json
"notify": { "kinds": { "booking_created": {} } }
```

The notification's content is kept in the org's OLTP store, so the same
`oxyc oltp provision` step above covers it. A role nobody holds comes back in the
response's `skipped` instead of failing the booking. Full reference:
`internal-docs/customer-apps-functions.md` (`ctx.notify`).

## Security note

Every value from the request is passed as a **bound parameter** (`$1`, `$2`, …),
never concatenated into SQL. A booking form is exactly the surface that takes
end-user input; placeholders are the only thing that makes it safe. `ctx.oltp`
runs each statement in a one-shot transaction, so a rejected write rolls back
rather than leaving a partial row.
