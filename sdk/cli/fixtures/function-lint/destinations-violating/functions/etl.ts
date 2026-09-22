// A write with no `destinations` at all: the empty allowlist refuses it.
export default async function etl(_req: unknown, ctx: any) {
  await ctx.warehouse.insert("analytics", "events", [{ a: 1 }]);
  return Response.json({ ok: true });
}
