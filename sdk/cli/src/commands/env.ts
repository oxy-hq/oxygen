/**
 * `oxyc env` — the sandbox environments of a custom app: `dev-<handle>`
 * slots with their own build pointer, storage silo, secrets and Airhouse
 * sibling (`internal-docs/custom-app-sandboxes.md`).
 *
 * All four verbs talk to the staff console surface,
 * `/api/customer-apps/{id}/environments[/{name}]`
 * (`crates/app/src/server/api/custom_apps_sandboxes/`) — never the machine
 * surface a publish token reaches. `staffCreds` refuses one outright: every
 * sandbox operation does (D22).
 *
 * A sandbox agent token (`oxy_sbx_…`) uses the same routes, held to the apps
 * it was minted for and the sandboxes it created.
 */

import { parseJson, request } from "../api/request.js";
import { parseAppEnv, requireSandboxName } from "../apps/environment.js";
import { type Creds, ensureOk, resolveApp, staffCreds } from "../apps/resolve.js";
import { isSandboxAgentToken } from "../auth/token-kind.js";
import type { Context } from "../context/resolve.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import {
  applyDestinations,
  applyFetchHosts,
  describePlanesSet,
  type PlaneValue,
  parseDestinationFlags,
  parseFetchHosts,
  parsePlaneFlags,
  requireForkAt,
  requirePreset,
  validatePlaneMap
} from "./env-plane-args.js";
import { confirm } from "./oltp-client.js";

/** The sandbox-management surface's wire shape — snake_case, printed verbatim. */
export interface Environment {
  name: string;
  kind: string;
  status: string;
  build_id: string | null;
  build_uuid: string | null;
  semantic_revision_id: string | null;
  owner: { user_id: string; email: string } | null;
  created_at: string;
  updated_at: string;
  last_activity_at: string | null;
  expires_at: string | null;
  url: string | null;
  /**
   * A sandbox's own schema on the org's OLTP staging branch, once a publish
   * has queued it; `null` otherwise, and absent from a server older than it.
   */
  oltp_schema?: OltpSchema | null;
  /**
   * A sandbox's own OLTP branch (`oltp=branch` or `oltp=empty`); `null` on
   * `schema` mode, production and staging, and absent from a server older
   * than it.
   */
  oltp_branch?: OltpBranch | null;
  /**
   * The environment this one branched from, when it is one — `null` for
   * production, and absent from a server older than branches.
   */
  parent?: string | null;
  /** RFC 3339: when this environment forked from `parent`. */
  fork_at?: string | null;
  /**
   * Each plane's mode (`build=own`, `oltp=schema`, …), keyed by plane. `null`
   * for production, and when `planes_error` says they could not be read.
   */
  planes?: Record<string, string> | null;
  /**
   * Only the planes someone set explicitly, as stored (`{}` when none) — what
   * `env update` merges into. `null` for production, and absent from a server
   * older than it.
   */
  planes_set?: Record<string, unknown> | null;
  /** Why `planes` could not be read; `null` when it could. */
  planes_error?: string | null;
}

/** `ctx.oltp`'s home in a sandbox: its schema, and whether it can be used yet. */
export interface OltpSchema {
  schema: string;
  /** `stale`: the org's staging branch was reset since; publish again. */
  status: "seeding" | "ready" | "failed" | "stale" | string;
  seeded_at: string | null;
  tables: number;
  /** Tables copied empty from staging: over the seed's size cap. */
  structure_only: string[];
  /**
   * What the copy still uses of staging's schema (a type, a function): while
   * the sandbox exists a staging migration cannot drop these.
   */
  staging_dependencies?: string[];
  error: string | null;
}

/** A sandbox's own Neon branch of the org's OLTP database. */
export interface OltpBranch {
  /** `branch`: a copy of production as of `fork_at`. `empty`: its schema, no rows. */
  mode: "branch" | "empty" | string;
  /** Only `ready` serves `ctx.oltp`. */
  status: "cutting" | "migrating" | "ready" | "resetting" | "failed" | string;
  fork_at: string;
  /** When the provider reaps it and the sandbox is torn down; `null` for `empty`. */
  expires_at: string | null;
  error: string | null;
}

interface Deleting {
  name: string;
  status: string;
  teardown_run_id?: string;
}

export function environmentsPath(appId: string, name?: string): string {
  const base = `/api/customer-apps/${appId}/environments`;
  return name === undefined ? base : `${base}/${encodeURIComponent(name)}`;
}

export function printEnvironment(env: unknown, asJson: boolean): void {
  if (asJson) {
    process.stdout.write(`${JSON.stringify(env)}\n`);
    return;
  }
  const e = env as Environment;
  const build = e.build_id ? `build ${e.build_id}` : "no build yet";
  process.stdout.write(`${e.name} (${e.kind}) — ${e.status}, ${build}\n`);
  if (e.url) process.stdout.write(`  ${e.url}\n`);
  const planes = describePlanes(e);
  if (planes) process.stdout.write(`  ${planes}\n`);
  const set = describePlanesSet(e.planes_set);
  if (set) process.stdout.write(`  set: ${set}\n`);
  if (e.oltp_schema) process.stdout.write(`  ${describeOltpSchema(e.oltp_schema)}\n`);
  if (e.oltp_branch) process.stdout.write(`  ${describeOltpBranch(e.oltp_branch)}\n`);
}

/**
 * The order `describePlanes` prints planes in. The server sends them
 * alphabetically (a map), which is no way to read them; this is the order a
 * developer thinks in. A later phase's new plane is added here.
 */
export const PLANE_ORDER: readonly string[] = [
  "build",
  "semantic",
  "reads",
  "oltp",
  "airhouse",
  "warehouse",
  "storage",
  "secrets",
  "messages",
  "fetch",
  "schedules",
  "workspace-actions"
];

/**
 * One line for the human output: where the environment branched from and each
 * plane's mode. `null` when the server sent nothing (production, or an older
 * server).
 */
export function describePlanes(
  env: Pick<Environment, "parent" | "fork_at" | "planes" | "planes_error">
): string | null {
  if (env.planes_error) {
    const from = env.parent ? `from ${env.parent} · ` : "";
    return `${from}planes: unreadable — ${env.planes_error}`;
  }
  const planes = env.planes;
  if (!planes) return null;
  const known = PLANE_ORDER.filter((key) => key in planes);
  const extra = Object.keys(planes)
    .filter((key) => !PLANE_ORDER.includes(key))
    .sort();
  const modes = [...known, ...extra].map((key) => `${key}=${planes[key]}`).join(" ");
  if (!modes) return env.parent ? `from ${env.parent}` : null;
  return env.parent ? `from ${env.parent} · ${modes}` : modes;
}

/** One line for the human output: the schema, its state, and what to do. */
export function describeOltpSchema(oltp: OltpSchema): string {
  const head = `OLTP ${oltp.schema} — ${oltp.status}`;
  switch (oltp.status) {
    case "ready": {
      const empty = oltp.structure_only.length
        ? `, copied empty (over the size cap): ${oltp.structure_only.join(", ")}`
        : "";
      const uses = oltp.staging_dependencies?.length
        ? `; still uses staging's ${oltp.staging_dependencies.join(", ")}`
        : "";
      return `${head} (${oltp.tables} table${oltp.tables === 1 ? "" : "s"} from staging${empty})${uses}`;
    }
    case "failed":
      return `${head}: ${oltp.error ?? "no reason recorded"}; publish to the sandbox again`;
    case "stale":
      return `${head}: ${oltp.error ?? "the org's OLTP staging branch was reset"}; publish to the sandbox again`;
    default:
      return head;
  }
}

/** One line for the human output: the branch, its state, and what to do. */
export function describeOltpBranch(branch: OltpBranch): string {
  const head = `OLTP branch (${branch.mode}) — ${branch.status}`;
  const forked = `forked ${branch.fork_at}`;
  switch (branch.status) {
    case "ready": {
      const ends = branch.expires_at
        ? `, expires ${branch.expires_at} (the sandbox goes with it)`
        : ", no expiry";
      return `${head} (${forked}${ends})`;
    }
    case "failed":
      return `${head}: ${branch.error ?? "no reason recorded"}; oxyc env reset <app> <env> --plane oltp cuts it again`;
    case "cutting":
    case "migrating":
    case "resetting":
      return `${head} (${forked}); ctx.oltp is refused until it is ready`;
    default:
      return head;
  }
}

/**
 * The request-building core of each verb, with no printing — shared by the
 * `oxyc env` CLI wrapper below and the `oxyc mcp` sandbox tools, so neither is
 * a second implementation of the request or its validation.
 */
/** What `env create` may carry beyond the name: where to branch from, and how. */
export interface EnvCreateOptions {
  /** The environment to branch from: production, staging, or an active sandbox. */
  from?: string;
  /** Branch from the parent as of this instant (any date `Date` reads). */
  at?: string;
  preset?: string;
  /** Plane key to mode, or `{mode, …settings}` — wins over the preset's. */
  planes?: Record<string, PlaneValue>;
  /** Warehouse overrides (production database to the one this environment writes). */
  destinations?: Record<string, string>;
  /** The fetch plane's host list (registered provider sandboxes); implies `fetch=allow`. */
  fetchHosts?: readonly string[];
}

/** The POST body for a create: `name`, and each option only when given. */
export function createBody(sandbox: string, opts: EnvCreateOptions): Record<string, unknown> {
  const body: Record<string, unknown> = { name: sandbox };
  if (opts.from !== undefined) body.from = parseAppEnv(opts.from);
  if (opts.at !== undefined) body.at = requireForkAt(opts.at);
  if (opts.preset !== undefined) body.preset = requirePreset(opts.preset);
  if (opts.planes !== undefined) {
    const planes = validatePlaneMap(opts.planes);
    if (Object.keys(planes).length > 0) body.planes = planes;
  }
  const overrides = opts.destinations ?? {};
  if (Object.keys(overrides).length > 0) {
    const planes = applyDestinations((body.planes as Record<string, unknown>) ?? {}, {
      set: overrides,
      clear: []
    });
    body.planes = planes;
  }
  const planes = (body.planes as Record<string, unknown> | undefined) ?? {};
  const withHosts = applyFetchHosts(planes, opts.fetchHosts ?? []);
  if (withHosts !== planes) body.planes = withHosts;
  return body;
}

export async function envCreate(
  ctx: Context,
  app: string,
  name: string,
  opts: EnvCreateOptions = {}
): Promise<Environment> {
  const sandbox = requireSandboxName(name);
  const body = createBody(sandbox, opts);
  const creds = staffCreds(ctx);
  const resolved = await resolveApp(creds, app);
  const response = await request({
    target: creds.target,
    path: environmentsPath(resolved.appId),
    method: "POST",
    body: JSON.stringify(body),
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  return parseJson(response.body) as Environment;
}

export async function envList(ctx: Context, app: string): Promise<Environment[]> {
  const creds = staffCreds(ctx);
  const resolved = await resolveApp(creds, app);
  const response = await request({
    target: creds.target,
    path: environmentsPath(resolved.appId),
    method: "GET",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  const payload = parseJson(response.body) as { environments?: Environment[] } | undefined;
  return payload?.environments ?? [];
}

export async function envShow(ctx: Context, app: string, name: string): Promise<Environment> {
  const envName = parseAppEnv(name);
  const creds = staffCreds(ctx);
  // `env list` shows production and staging to a sandbox agent token; the
  // detail route answers only for a sandbox the token created.
  if (isSandboxAgentToken(creds.bearer) && !envName.startsWith("dev-")) {
    throw usageError(
      `a sandbox agent token cannot show ${envName}`,
      "it reads only the dev-<handle> sandboxes it created — `oxyc env list <app>` has production and staging's rows"
    );
  }
  const resolved = await resolveApp(creds, app);
  return fetchEnvironment(creds, resolved.appId, envName);
}

/**
 * `GET …/environments/{name}` for an app already resolved — `env show`'s
 * request, for a caller that holds the credential and the id already (the
 * browser check). The route requires non-production reach for every name,
 * `production` included.
 */
export async function fetchEnvironment(
  creds: Creds,
  appId: string,
  name: string
): Promise<Environment> {
  const response = await request({
    target: creds.target,
    path: environmentsPath(appId, name),
    method: "GET",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  return parseJson(response.body) as Environment;
}

export async function runEnvCreate(
  ctx: Context,
  app: string,
  name: string,
  opts: {
    json: boolean;
    from?: string;
    at?: string;
    preset?: string;
    plane?: string[];
    destination?: string[];
    fetchHost?: string[];
  }
): Promise<void> {
  const planes = opts.plane?.length ? parsePlaneFlags(opts.plane) : undefined;
  const destinations = parseDestinationFlags(opts.destination ?? [], []).set;
  const fetchHosts = parseFetchHosts(opts.fetchHost ?? []);
  const created = await envCreate(ctx, app, name, {
    from: opts.from,
    at: opts.at,
    preset: opts.preset,
    planes,
    destinations,
    fetchHosts
  });
  printEnvironment(created, opts.json);
}

export async function runEnvList(
  ctx: Context,
  app: string,
  opts: { json: boolean }
): Promise<void> {
  const environments = await envList(ctx, app);
  if (opts.json) {
    process.stdout.write(`${JSON.stringify({ environments })}\n`);
    return;
  }
  if (environments.length === 0) {
    process.stderr.write("no environments\n");
    return;
  }
  for (const e of environments) {
    const build = e.build_id ?? "no build";
    process.stdout.write(
      `${e.name.padEnd(14)} ${e.kind.padEnd(11)} ${e.status.padEnd(9)} ${build}\n`
    );
  }
}

export async function runEnvShow(
  ctx: Context,
  app: string,
  name: string,
  opts: { json: boolean }
): Promise<void> {
  printEnvironment(await envShow(ctx, app, name), opts.json);
}

function validateWaitSeconds(waitSeconds: number | undefined): void {
  if (waitSeconds === undefined) return;
  if (!Number.isFinite(waitSeconds) || waitSeconds <= 0) {
    throw usageError(
      "--wait must be a positive number of seconds",
      "e.g. --wait 60 (the default is 120)"
    );
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** Poll `GET .../environments/{name}` every `pollMs` until it 404s, or the deadline passes. */
async function waitForTeardown(
  creds: { target: string; bearer?: string; headers?: Record<string, string> },
  path: string,
  sandbox: string,
  waitSeconds: number,
  pollMs: number
): Promise<void> {
  const deadline = Date.now() + waitSeconds * 1000;
  for (;;) {
    const poll = await request({
      target: creds.target,
      path,
      method: "GET",
      bearer: creds.bearer,
      headers: creds.headers
    });
    if (poll.status === 404) return;
    ensureOk(poll, creds);
    if (Date.now() >= deadline) {
      throw new CliError(`${sandbox} did not finish deleting within ${waitSeconds}s`, {
        code: ExitCode.UNAVAILABLE,
        hint: "the teardown runs on the worker fleet — retry `oxyc env delete … --wait`, or check `oxyc env show` later"
      });
    }
    await sleep(pollMs);
  }
}

/**
 * Delete a sandbox, waiting for teardown when `waitSeconds` is set. No
 * printing — `runEnvDelete` below does that; the `oxyc mcp` delete tool calls
 * this directly. `opts.yes` is required when there is no TTY to confirm on
 * (`confirm()` refuses off one), which is always true inside an MCP server.
 */
export async function envDelete(
  ctx: Context,
  app: string,
  name: string,
  opts: { yes?: boolean; waitSeconds?: number; pollMs?: number }
): Promise<Deleting | { name: string; status: string }> {
  const sandbox = requireSandboxName(name);
  validateWaitSeconds(opts.waitSeconds);
  const creds = staffCreds(ctx);
  const resolved = await resolveApp(creds, app);
  const path = environmentsPath(resolved.appId, sandbox);

  if (!opts.yes) {
    await confirm(`Delete ${sandbox}?`, {
      verb: "a sandbox delete",
      why: "it tears down the sandbox's storage silo, secrets and Airhouse sibling",
      declined: "not deleted — the confirmation was declined"
    });
  }

  const response = await request({
    target: creds.target,
    path,
    method: "DELETE",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  const deleting = parseJson(response.body) as Deleting | undefined;

  if (opts.waitSeconds === undefined) {
    return deleting ?? { name: sandbox, status: "deleting" };
  }

  await waitForTeardown(creds, path, sandbox, opts.waitSeconds, opts.pollMs ?? 2000);
  return { name: sandbox, status: "deleted" };
}

export async function runEnvDelete(
  ctx: Context,
  app: string,
  name: string,
  opts: { yes?: boolean; waitSeconds?: number; json: boolean; pollMs?: number }
): Promise<void> {
  const result = await envDelete(ctx, app, name, opts);
  const sandbox = result.name;
  if (opts.waitSeconds === undefined) {
    if (opts.json) process.stdout.write(`${JSON.stringify(result)}\n`);
    else process.stdout.write(`${out.yellow("deleting")} ${sandbox}…\n`);
    return;
  }
  if (opts.json) process.stdout.write(`${JSON.stringify(result)}\n`);
  else process.stdout.write(`${out.green(sandbox)} deleted\n`);
}
