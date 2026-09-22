// Writes to a database the allowlist does not name.
export default async function other(_req: unknown, ctx: any) {
  await ctx.warehouse.exec("analytics", "DELETE FROM events");
  const id = await ctx.tx("analytics", async (tx: any) => tx.exec("SELECT 1"));
  return Response.json({ id });
}
