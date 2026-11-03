/**
 * `oxyc env outbox` — the email and notifications an environment captured
 * instead of sending (`GET …/environments/{name}/outbox`), newest first.
 *
 * An environment whose `messages` plane is `capture` (the `full` preset sets
 * it) renders each `ctx.email.send` and `ctx.notify`, sends nothing, and keeps
 * the message. This reads them back. Same door as the live tail: whoever may
 * open the environment, and a sandbox agent token for a sandbox it created.
 * Production has no outbox, which the server answers `409`.
 */

import { parseJson, request } from "../api/request.js";
import { parseAppEnv } from "../apps/environment.js";
import { ensureOk, resolveApp, staffCreds } from "../apps/resolve.js";
import type { Context } from "../context/resolve.js";
import { usageError } from "../util/errors.js";
import { environmentsPath } from "./env.js";

/** One captured message. `email` and `notify` carry different extras. */
export interface OutboxMessage {
  kind: "email" | "notify" | string;
  /** Who the call named — nobody was reached. */
  to: unknown;
  subject?: string;
  title?: string;
  body: string | null;
  invocation_id: string;
  function: string;
  captured_at: string;
  [extra: string]: unknown;
}

export interface EnvOutbox {
  messages: OutboxMessage[];
  /** There are older messages than these. */
  has_more: boolean;
}

/** The server answers at most this many; asking for more is not an error. */
export const OUTBOX_MAX_LIMIT = 200;

/** `GET …/outbox` for an environment. No printing. */
export async function envOutbox(
  ctx: Context,
  app: string,
  name: string,
  opts: { limit?: number } = {}
): Promise<EnvOutbox> {
  const environment = parseAppEnv(name);
  if (opts.limit !== undefined && !(Number.isInteger(opts.limit) && opts.limit >= 1)) {
    throw usageError(
      "--limit must be a whole number of at least 1",
      `at most ${OUTBOX_MAX_LIMIT} are answered`
    );
  }
  const creds = staffCreds(ctx);
  const { appId } = await resolveApp(creds, app);
  const query = opts.limit === undefined ? "" : `?limit=${opts.limit}`;
  const response = await request({
    target: creds.target,
    path: `${environmentsPath(appId, environment)}/outbox${query}`,
    method: "GET",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  const payload = parseJson(response.body) as Partial<EnvOutbox> | undefined;
  return { messages: payload?.messages ?? [], has_more: payload?.has_more ?? false };
}

function who(to: unknown): string {
  if (Array.isArray(to)) {
    return to.map((t) => (typeof t === "string" ? t : JSON.stringify(t))).join(", ");
  }
  return typeof to === "string" ? to : JSON.stringify(to);
}

/** One message, for the human output: a headline line, then the body indented. */
export function describeMessage(m: OutboxMessage): string[] {
  const headline = m.kind === "notify" ? (m.title ?? "") : (m.subject ?? "");
  const lines = [
    `${m.captured_at}  ${m.kind}  ${m.function}  → ${who(m.to)}`,
    `  ${headline}`.trimEnd()
  ];
  if (m.body) lines.push(...m.body.split("\n").map((l) => `    ${l}`));
  return lines;
}

export async function runEnvOutbox(
  ctx: Context,
  app: string,
  name: string,
  opts: { limit?: number; json: boolean }
): Promise<void> {
  const outbox = await envOutbox(ctx, app, name, { limit: opts.limit });
  if (opts.json) {
    process.stdout.write(`${JSON.stringify(outbox)}\n`);
    return;
  }
  if (outbox.messages.length === 0) {
    process.stderr.write("no captured messages\n");
    return;
  }
  const lines = outbox.messages.flatMap(describeMessage);
  if (outbox.has_more) lines.push("(older messages exist — raise --limit)");
  process.stdout.write(`${lines.join("\n")}\n`);
}
