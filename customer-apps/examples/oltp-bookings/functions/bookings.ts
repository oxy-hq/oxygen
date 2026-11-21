// bookings — an example Oxy Function that reads and WRITES the app's own
// per-org OLTP store through `ctx.oltp`.
//
// This is the half `ctx.warehouse` cannot give an app. For a `postgres_managed`
// database `ctx.warehouse` resolves the org's read-only ANALYST — so a write
// authenticates and then fails `permission denied`, and a read sees the org's
// whole warehouse (the `raw_*` Toast/QuickBooks extracts included). `ctx.oltp`
// resolves the app's own WRITER role instead: DML rights scoped to the single
// `app_oltp_bookings` schema, and nothing else. Narrower on reads, finally writable.
//
// Declared in oxy-app.json under `functions.bookings` with
// `"oltp": { "enabled": true }` — the fail-closed capability that PERMITS
// `ctx.oltp`. It does not name a schema: the target is derived from the app's
// own slug (`oltp-bookings` → `app_oltp_bookings`), so no manifest can point at
// another app's data. Without the capability the host rejects every call.
//
// PREREQUISITE — the org operator provisions the store once, before publishing.
// The writer name matches the app slug with hyphens as underscores:
//   oxyc oltp provision --org <org> --writer app:oltp_bookings
// That mints the `app_oltp_bookings_rw` role and its schema. `ctx.oltp` then
// resolves it on each call (no provider round-trip — the sealed credential is
// read from Oxy's control plane).
//
// SECURITY — every value that comes from the request is passed as a BOUND
// parameter ($1, $2, …), never concatenated into SQL. A booking form is exactly
// the surface that takes end-user input; placeholders are the only thing that
// makes it safe.
//
// TELLING PEOPLE — a new booking is announced with `ctx.notify`, which names
// PEOPLE, never addresses: whoever holds the "Host" position, plus the person
// who took the booking. The platform resolves who that is, writes each one an
// inbox row, and pushes / emails them where it can. Declared as
// `"notify": { "kinds": { "booking_created": {} } }` — a kind is what a person
// can mute in their notification settings.
//
// Invoke from the frontend with `useFunction("bookings")`:
//   const { invoke } = useFunction("bookings");
//   await invoke({ action: "create", name: "Ada", partySize: 4 });
//   const { bookings } = await invoke({ action: "list" });

import type { OxyFunctionContext, OxyFunctionRequest } from "@oxy-hq/sdk";

interface BookingsBody {
  action?: "list" | "create";
  name?: string;
  partySize?: number;
}

/**
 * Create the app's table on first use. The writer owns objects in its own
 * `app_oltp_bookings` schema, so it may run this DDL; `IF NOT EXISTS` makes it a
 * no-op afterwards. A real app would move this into a migration, but keeping it
 * here makes the example runnable the moment the writer is provisioned.
 */
async function ensureTable(ctx: OxyFunctionContext): Promise<void> {
  await ctx.oltp.exec(
    `CREATE TABLE IF NOT EXISTS bookings (
       id         bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
       name       text        NOT NULL,
       party_size int         NOT NULL,
       created_at timestamptz NOT NULL DEFAULT now()
     )`
  );
}

/**
 * Tell the floor about a new booking. A role nobody holds comes back in
 * `skipped` rather than failing the booking, and the idempotency key makes a
 * retried request tell nobody twice. The booking is already committed, so a
 * refused notify is logged, not returned as an error: a client that retried a
 * 500 would book the table twice.
 */
async function announce(
  ctx: OxyFunctionContext,
  row: { id: number },
  name: string,
  partySize: number
) {
  try {
    return await ctx.notify({
      to: [{ role: "Host" }, { userId: ctx.user.id }],
      kind: "booking_created",
      title: `New booking: ${name}, party of ${partySize}`,
      body: "Added from the bookings app.",
      // A path inside the app. This example has no pages, so it opens the app's
      // root; a real app links to the booking's own page.
      link: "/",
      email: true,
      idempotencyKey: `booking:${row.id}`
    });
  } catch (err) {
    console.error(`booking ${row.id} saved but not announced: ${err}`);
    return { recipients: 0, skipped: [] };
  }
}

export default async function bookings(
  req: OxyFunctionRequest,
  ctx: OxyFunctionContext
): Promise<Response> {
  const body = JSON.parse(req.body || "{}") as BookingsBody;
  await ensureTable(ctx);

  if (body.action === "create") {
    const name = (body.name ?? "").trim();
    const partySize = Number(body.partySize);
    if (!name || !Number.isInteger(partySize) || partySize < 1) {
      return Response.json(
        { error: "name and a positive integer partySize are required" },
        { status: 400 }
      );
    }
    // Parameterised INSERT … RETURNING — the created row without a second read.
    const [row] = await ctx.oltp.query(
      "INSERT INTO bookings (name, party_size) VALUES ($1, $2) RETURNING id, name, party_size, created_at",
      [name, partySize]
    );
    const notice = await announce(ctx, row as { id: number }, name, partySize);
    return Response.json({ booking: row, notified: notice.recipients, skipped: notice.skipped }, { status: 201 });
  }

  // Default: list the most recent bookings from the app's own schema.
  const rows = await ctx.oltp.query(
    "SELECT id, name, party_size, created_at FROM bookings ORDER BY created_at DESC LIMIT 100"
  );
  return Response.json({ bookings: rows });
}
