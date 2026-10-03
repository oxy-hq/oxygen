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
 */

import { parseJson, request } from "../api/request.js";
import { parseAppEnv, requireSandboxName } from "../apps/environment.js";
import { ensureOk, resolveApp, staffCreds } from "../apps/resolve.js";
import type { Context } from "../context/resolve.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
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

interface Deleting {
  name: string;
  status: string;
  teardown_run_id?: string;
}

function environmentsPath(appId: string, name?: string): string {
  const base = `/api/customer-apps/${appId}/environments`;
  return name === undefined ? base : `${base}/${encodeURIComponent(name)}`;
}

function printEnvironment(env: unknown, asJson: boolean): void {
  if (asJson) {
    process.stdout.write(`${JSON.stringify(env)}\n`);
    return;
  }
  const e = env as Environment;
  const build = e.build_id ?? "no build";
  process.stdout.write(`${e.name} (${e.kind}) — ${e.status}, build ${build}\n`);
  if (e.url) process.stdout.write(`  ${e.url}\n`);
  if (e.oltp_schema) process.stdout.write(`  ${describeOltpSchema(e.oltp_schema)}\n`);
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

/**
 * The request-building core of each verb, with no printing — shared by the
 * `oxyc env` CLI wrapper below and the `oxyc mcp` sandbox tools, so neither is
 * a second implementation of the request or its validation.
 */
export async function envCreate(ctx: Context, app: string, name: string): Promise<Environment> {
  const sandbox = requireSandboxName(name);
  const creds = staffCreds(ctx);
  const resolved = await resolveApp(creds, app);
  const response = await request({
    target: creds.target,
    path: environmentsPath(resolved.appId),
    method: "POST",
    body: JSON.stringify({ name: sandbox }),
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response);
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
  ensureOk(response);
  const payload = parseJson(response.body) as { environments?: Environment[] } | undefined;
  return payload?.environments ?? [];
}

export async function envShow(ctx: Context, app: string, name: string): Promise<Environment> {
  const envName = parseAppEnv(name);
  const creds = staffCreds(ctx);
  const resolved = await resolveApp(creds, app);
  const response = await request({
    target: creds.target,
    path: environmentsPath(resolved.appId, envName),
    method: "GET",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response);
  return parseJson(response.body) as Environment;
}

export async function runEnvCreate(
  ctx: Context,
  app: string,
  name: string,
  opts: { json: boolean }
): Promise<void> {
  printEnvironment(await envCreate(ctx, app, name), opts.json);
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
    ensureOk(poll);
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
  ensureOk(response);
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
