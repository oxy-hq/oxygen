// echo — answers `{ ok: true, echo: <request body> }`.
//
// The canary's UI invokes it through `useFunction` with a fresh nonce and checks
// the nonce comes back (src/checks.ts). That proves the route path a live app's
// page takes: session cookie → `/fn/echo` → isolate → the SSE result stream.

import type { OxyFunctionContext, OxyFunctionRequest } from "@oxy-hq/sdk";

export default async function echo(
  req: OxyFunctionRequest,
  _ctx: OxyFunctionContext
): Promise<Response> {
  const body: unknown = req.body ? JSON.parse(req.body) : {};
  return Response.json({ ok: true, echo: body });
}
