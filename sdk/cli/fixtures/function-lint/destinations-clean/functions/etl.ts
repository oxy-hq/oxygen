// Every write names a database in `destinations`; the last names it through
// a variable, which only the host can resolve.
const DATABASE = "analytics";
export default async function etl(_req: unknown, ctx: any) {
  await ctx.warehouse.insert("analytics", "events", [{ a: 1 }]);
  await ctx.warehouse.exec("analytics", "DELETE FROM events WHERE a = 0");
  await ctx.warehouse.upsert("analytics", "events", [{ a: 1 }], ["a"]);
  const id = await ctx.tx("analytics", async (tx: any) => tx.exec("SELECT 1"));
  await ctx.warehouse.exec(DATABASE, "SELECT 1");
  const { rows } = await ctx.warehouse.query("analytics", "SELECT 1");
  return Response.json({ id, rows });
}
