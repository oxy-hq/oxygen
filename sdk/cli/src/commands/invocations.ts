/**
 * `oxyc invocations` — the read-back of what ran in an app's
 * environment: `GET /api/customer-apps/{id}/invocations[/{id}/held]`
 * (`admin/apps/{functions,invocations,held_writes}.rs`). A sandbox agent token
 * reads the same two handlers, for its own sandboxes only.
 *
 * `list <org>/<app>` reads `GET /api/customer-apps/{org}/{app}/invocations`
 * instead (`logs/invocations.rs`): the `/logs` route's gate, so an app admin —
 * staff inside an assume-role session, an App Operator in scope, the org's
 * own officers — reads it without the staff console, which refuses an assume
 * session. An app UUID and a sandbox agent token keep the console route.
 *
 * `held` is the list of writes the non-production policy did NOT perform —
 * what production would have done — so an agent iterating in a sandbox can
 * see what its own run skipped without re-deriving the policy by hand.
 */

import { parseJson, request } from "../api/request.js";
import { parseAppEnv } from "../apps/environment.js";
import {
  CUSTOMER_APPS_API,
  ensureOk,
  resolveApp,
  resolveAppSlugs,
  staffCreds,
  UUID_RE
} from "../apps/resolve.js";
import { requireOwnSandbox } from "../apps/sandbox-token.js";
import { isSandboxAgentToken } from "../auth/token-kind.js";
import type { Context } from "../context/resolve.js";

/**
 * Where `app`'s invocation list is read: the slug route for an
 * `<org>/<app>` argument from a person's credential, else the console route
 * by id.
 */
async function invocationsBase(
  creds: { target: string; bearer?: string; headers?: Record<string, string> },
  app: string
): Promise<string> {
  if (!isSandboxAgentToken(creds.bearer) && !UUID_RE.test(app)) {
    const { orgSlug, appSlug } = await resolveAppSlugs(creds, app);
    return `${CUSTOMER_APPS_API}/${orgSlug}/${appSlug}/invocations`;
  }
  const { appId } = await resolveApp(creds, app);
  return `${CUSTOMER_APPS_API}/${appId}/invocations`;
}

/**
 * One invocation listing, and — on the slug route — which environments it
 * covered: `"all"`, or the one name it was limited to. The console route by
 * id does not say, so `environments` is then absent.
 */
export interface InvocationListing {
  invocations: unknown[];
  environments?: string;
}

export async function readInvocations(
  ctx: Context,
  app: string,
  opts: { appEnv?: string; build?: string; fn?: string; limit?: number }
): Promise<InvocationListing> {
  const appEnv = opts.appEnv !== undefined ? parseAppEnv(opts.appEnv) : undefined;
  const creds = staffCreds(ctx);
  // Before any request: the listing a sandbox agent token may read is one
  // sandbox's, and the server requires it to name which.
  if (isSandboxAgentToken(creds.bearer)) requireOwnSandbox("oxyc invocations list", appEnv);
  const base = await invocationsBase(creds, app);

  const query = new URLSearchParams();
  if (opts.fn !== undefined) query.set("function", opts.fn);
  if (appEnv !== undefined) query.set("environment", appEnv);
  if (opts.build !== undefined) query.set("build", opts.build);
  if (opts.limit !== undefined) query.set("limit", String(opts.limit));
  const qs = query.toString();

  const response = await request({
    target: creds.target,
    path: `${base}${qs ? `?${qs}` : ""}`,
    method: "GET",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  const payload = parseJson(response.body) as
    | { invocations?: unknown[]; environments?: unknown }
    | undefined;
  const listing: InvocationListing = { invocations: payload?.invocations ?? [] };
  if (typeof payload?.environments === "string") listing.environments = payload.environments;
  return listing;
}

export async function invocationsList(
  ctx: Context,
  app: string,
  opts: { appEnv?: string; build?: string; fn?: string; limit?: number }
): Promise<unknown[]> {
  return (await readInvocations(ctx, app, opts)).invocations;
}

/**
 * A listing that named no environment and was still read from production
 * alone: the server limits a caller without non-production reach that way,
 * where a caller with reach reads every environment. Said on stderr so
 * stdout stays the rows.
 */
function limitedToProduction(listing: InvocationListing, appEnv: string | undefined): boolean {
  return appEnv === undefined && listing.environments === "production";
}

export async function runInvocationsList(
  ctx: Context,
  app: string,
  opts: { appEnv?: string; build?: string; fn?: string; limit?: number; json: boolean }
): Promise<void> {
  const listing = await readInvocations(ctx, app, opts);
  if (limitedToProduction(listing, opts.appEnv)) {
    process.stderr.write(
      "note: production only — this credential cannot read the app's non-production environments\n"
    );
  }
  if (opts.json) {
    process.stdout.write(`${JSON.stringify(listing)}\n`);
    return;
  }
  printInvocationsTable(listing.invocations);
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
    path: `${CUSTOMER_APPS_API}/${resolved.appId}/invocations/${encodeURIComponent(invocationId)}/held`,
    method: "GET",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
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
  namespace?: string;
  verb?: string;
  table?: string;
  note?: string;
  statements?: number;
  /** Set on a call that was sent to a provider sandbox rather than held. */
  path?: string | null;
  status?: number | null;
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
    process.stdout.write(`${heldLine(h)}\n`);
  }
}

/** A held call, or (it has a `path`) one that was sent to a provider sandbox. */
function heldLine(h: HeldWrite): string {
  if (h.path == null) {
    return `${h.op} (${h.plane}${h.table ? `, ${h.table}` : ""}) — ${h.note ?? "held"}`;
  }
  const count = (h.statements ?? 1) > 1 ? ` ×${h.statements}` : "";
  return `${h.op} ${h.verb ?? ""} ${h.namespace ?? ""}${h.path} → ${h.status ?? "no response"} (sent)${count}`;
}
