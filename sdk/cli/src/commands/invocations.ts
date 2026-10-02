/**
 * `oxyc invocations` — the admin-only read-back of what ran in an app's
 * environment: `GET /api/admin/apps/{id}/invocations[/{id}/held]`
 * (`admin/apps/{functions,invocations,held_writes}.rs`).
 *
 * `held` is the list of writes the non-production policy did NOT perform —
 * what production would have done — so an agent iterating in a sandbox can
 * see what its own run skipped without re-deriving the policy by hand.
 */

import { parseJson, request } from "../api/request.js";
import { parseAppEnv } from "../apps/environment.js";
import { ensureOk, resolveApp, staffCreds } from "../apps/resolve.js";
import type { Context } from "../context/resolve.js";

function invocationsPath(appId: string, query: URLSearchParams): string {
  const qs = query.toString();
  return `/api/admin/apps/${appId}/invocations${qs ? `?${qs}` : ""}`;
}

export async function invocationsList(
  ctx: Context,
  app: string,
  opts: { appEnv?: string; build?: string; fn?: string; limit?: number }
): Promise<unknown[]> {
  const appEnv = opts.appEnv !== undefined ? parseAppEnv(opts.appEnv) : undefined;
  const creds = staffCreds(ctx);
  const resolved = await resolveApp(creds, app);

  const query = new URLSearchParams();
  if (opts.fn !== undefined) query.set("function", opts.fn);
  if (appEnv !== undefined) query.set("environment", appEnv);
  if (opts.build !== undefined) query.set("build", opts.build);
  if (opts.limit !== undefined) query.set("limit", String(opts.limit));

  const response = await request({
    target: creds.target,
    path: invocationsPath(resolved.appId, query),
    method: "GET",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response);
  const payload = parseJson(response.body) as { invocations?: unknown[] } | undefined;
  return payload?.invocations ?? [];
}

export async function runInvocationsList(
  ctx: Context,
  app: string,
  opts: { appEnv?: string; build?: string; fn?: string; limit?: number; json: boolean }
): Promise<void> {
  const invocations = await invocationsList(ctx, app, opts);
  if (opts.json) {
    process.stdout.write(`${JSON.stringify({ invocations })}\n`);
    return;
  }
  printInvocationsTable(invocations);
}

export interface Held {
  held?: HeldWrite[];
  function?: string;
  status?: string;
}

export async function invocationsHeld(
  ctx: Context,
  app: string,
  invocationId: string
): Promise<Held> {
  const creds = staffCreds(ctx);
  const resolved = await resolveApp(creds, app);
  const response = await request({
    target: creds.target,
    path: `/api/admin/apps/${resolved.appId}/invocations/${encodeURIComponent(invocationId)}/held`,
    method: "GET",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response);
  return (parseJson(response.body) as Held | undefined) ?? {};
}

export async function runInvocationsHeld(
  ctx: Context,
  app: string,
  invocationId: string,
  opts: { json: boolean }
): Promise<void> {
  const payload = await invocationsHeld(ctx, app, invocationId);
  if (opts.json) {
    process.stdout.write(`${JSON.stringify(payload)}\n`);
    return;
  }
  printHeld(payload);
}

interface HeldWrite {
  op: string;
  plane: string;
  table?: string;
  note?: string;
}

/** A table cell: the value when it is a string or number, else the fallback. */
function cell(value: unknown, fallback: string): string {
  if (typeof value === "string") return value;
  if (typeof value === "number") return String(value);
  return fallback;
}

/**
 * One line per row: its id (what `invocations held` takes), function,
 * environment and status. The keys are the server's `InvocationSummary` —
 * a row's id is `id`, not the `invocation_id` the held route answers with.
 */
function printInvocationsTable(invocations: unknown[]): void {
  if (invocations.length === 0) {
    process.stderr.write("no invocations\n");
    return;
  }
  for (const raw of invocations) {
    const row = raw as Record<string, unknown>;
    process.stdout.write(
      `${cell(row.id, "?").padEnd(38)} ${cell(row.function_name, "?").padEnd(20)} ${cell(row.environment, "production").padEnd(12)} ${cell(row.status, "?")}\n`
    );
  }
}

function printHeld(
  payload: { held?: HeldWrite[]; function?: string; status?: string } | undefined
): void {
  const held = payload?.held ?? [];
  if (held.length === 0) {
    process.stderr.write(`${payload?.function ?? "this invocation"} held nothing\n`);
    return;
  }
  for (const h of held) {
    process.stdout.write(
      `${h.op} (${h.plane}${h.table ? `, ${h.table}` : ""}) — ${h.note ?? "held"}\n`
    );
  }
}
