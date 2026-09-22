// Every gated `ctx` area, in a function whose manifest entry declares nothing.
export default async function sync(_req: unknown, ctx: any) {
  await ctx.secrets.set("SYNC_TOKEN", "t");
  await ctx.email.send({ to: "a@b.c", subject: "s", html: "<p/>" });
  const people = await ctx.org.people();
  const url = await ctx.storage.getUploadUrl({ pathname: "x" });
  await ctx.storage.put("x", "y");
  await ctx.storage.get("x");
  await ctx.storage.copy("x", "z");
  await ctx.oltp.query("SELECT 1");
  await ctx.airhouse.append("t", [{ a: 1 }]);
  return Response.json({ people, url });
}
