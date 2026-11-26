// leases: the Lease Desk's one function. It keeps a property manager's leases
// in the app's own OLTP store through `ctx.oltp`, one row per lease, always
// the lease as it stands now.
//
// oxy-app.json declares `leases` under `oltpChanges`, so the platform ships
// every change of that table to the workspace's Airhouse as well. The function
// writes a lease once; analytics get the lease's whole history (every rent it
// ever had) and its current state, with no second write and no event table to
// keep in step. `tenant_email` is excluded there: it stays in this store.
//
// POST bodies:
//   { "action": "sign", "leaseRef": "L-4B", "unit": "4B", "tenantName": "Ada Lovelace",
//     "tenantEmail": "ada@example.com", "rentCents": 185000, "startsOn": "2026-01-01" }
//   { "action": "set-rent", "leaseRef": "L-4B", "rentCents": 190000 }
//   { "action": "end", "leaseRef": "L-4B", "endedOn": "2026-12-31" }
//   { "action": "void", "leaseRef": "L-4B" }   a lease entered by mistake
//   { "action": "list" }                        every lease, as it stands
//
// Every value from the request is a bound parameter, never part of the SQL.

const LEASE = "lease_ref, unit, tenant_name, rent_cents, starts_on, ended_on";

const actions = {
  sign: (ctx, b) =>
    one(
      ctx,
      `INSERT INTO leases (lease_ref, unit, tenant_name, tenant_email, rent_cents, starts_on)
       VALUES ($1, $2, $3, $4, $5, $6) RETURNING ${LEASE}`,
      [b.leaseRef, b.unit, b.tenantName, b.tenantEmail ?? null, b.rentCents, b.startsOn]
    ),
  "set-rent": (ctx, b) =>
    one(ctx, `UPDATE leases SET rent_cents = $2 WHERE lease_ref = $1 RETURNING ${LEASE}`, [
      b.leaseRef,
      b.rentCents
    ]),
  end: (ctx, b) =>
    one(ctx, `UPDATE leases SET ended_on = $2 WHERE lease_ref = $1 RETURNING ${LEASE}`, [
      b.leaseRef,
      b.endedOn
    ]),
  void: (ctx, b) =>
    one(ctx, `DELETE FROM leases WHERE lease_ref = $1 RETURNING ${LEASE}`, [b.leaseRef]),
  list: async (ctx) =>
    Response.json({
      leases: await ctx.oltp.query(`SELECT ${LEASE} FROM leases ORDER BY lease_ref`)
    })
};

/** Run one write that names one lease, and answer that lease as it now is. */
async function one(ctx, sql, params) {
  const [lease] = await ctx.oltp.query(sql, params);
  if (!lease) return Response.json({ error: "no such lease" }, { status: 404 });
  return Response.json({ lease });
}

export default async (req, ctx) => {
  const body = JSON.parse(req.body || "{}");
  if (!Object.hasOwn(actions, body.action)) {
    return Response.json({ error: `unknown action: ${body.action}` }, { status: 400 });
  }
  return actions[body.action](ctx, body);
};
