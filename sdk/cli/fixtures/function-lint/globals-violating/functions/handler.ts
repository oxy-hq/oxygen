// One value use of each global the isolate does not provide.
export default async function handler(_req: unknown, ctx: any) {
  const a = Buffer.from("x").toString("base64");
  const b = new TextEncoder().encode("x");
  const c = new TextDecoder().decode(b);
  const d = new Blob([a]);
  const e = new File([d], "f.txt");
  const f = new FormData();
  const g = await crypto.subtle.digest("SHA-256", b);
  const h = process.env.API_KEY;
  return Response.json({ a, c, e, f, g, h, ctx });
}
