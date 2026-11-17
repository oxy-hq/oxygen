// The template's shape: the body cast, a Response returned — input only.
import type { OxyFunctionContext, OxyFunctionRequest } from "@oxy-hq/sdk";

interface NotifyBody {
  /** Who to greet. */
  name?: string;
}

export default async function notify(
  req: OxyFunctionRequest,
  ctx: OxyFunctionContext
): Promise<Response> {
  const { name } = JSON.parse(req.body || "{}") as NotifyBody;
  const re = /[{}]/g; // a regex literal with braces must not unbalance the body
  return Response.json({ ok: true, to: ctx.user.email, name: name?.replace(re, "") });
}
