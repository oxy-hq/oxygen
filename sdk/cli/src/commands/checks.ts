/**
 * `oxyc checks run <app>` — run every function an app marked `"check": true`
 * and wait for a verdict.
 *
 * This is what a release workflow calls once an environment serves a build: it
 * POSTs one run per check, polls each to a terminal status, and exits
 * `CHECK_FAILED` (9) the moment any of them did not pass — so a release gate
 * can branch on the exit code alone, the same contract every other `oxyc`
 * command keeps.
 *
 * Talks only to four routes (`GET /api/admin/apps`, `GET .../functions`,
 * `POST .../functions/{name}/runs`, `GET .../function-runs/{run_id}`); nothing
 * here decides whether a function IS a check — that is `FunctionSummary.check`,
 * which the server projects from the manifest flag.
 *
 * Two surfaces, same handlers. A human runs this with their own credential and
 * hits `/api/admin/apps/…`. CI has no credential to store: in a job with
 * `id-token: write` it exchanges a GitHub OIDC token for the same short-lived,
 * app-scoped publish token `oxyc publish` uses, and then hits
 * `/api/customer-apps/…` — the same three handlers, mounted where a publish
 * token may reach them. The app-listing route is NOT one of them (a publish
 * token may not enumerate apps), which is why the exchange returns the app id
 * rather than leaving the CLI to look one up.
 */

import { errorForResponse, parseJson, request } from "../api/request.js";
import type { Context } from "../context/resolve.js";
import { exchangeGithubOidc, githubOidcAvailable } from "../publish/server.js";
import { err } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";

/** The `--json` report: one object on stdout. */
export interface ChecksReport {
  app: string;
  appId: string;
  checks: CheckResult[];
}

export interface CheckResult {
  name: string;
  runId: string;
  status: string;
  passed: boolean;
  error?: string;
  durationMs: number;
}

interface AdminAppsPage {
  items?: { id: string; slug: string; org_slug: string }[];
  next_offset?: number | null;
}

interface FunctionSummary {
  name: string;
  check?: boolean;
}

type RunStatus = "queued" | "running" | "done" | "failed" | "cancelled" | "timed_out";

interface RunDetail {
  run_id: string;
  status: RunStatus;
  answer?: string | null;
  error?: string | null;
}

const TERMINAL: ReadonlySet<RunStatus> = new Set(["done", "failed", "cancelled", "timed_out"]);
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
/** Hard stop on the admin-apps walk, mirroring `api/paginate.ts`'s `MAX_PAGES`. */
const MAX_APP_PAGES = 100;

export async function runChecks(
  ctx: Context,
  app: string,
  opts: { json: boolean; timeoutSeconds: number; pollMs?: number }
): Promise<void> {
  // `main.ts` passes `Number(--timeout)`. A NaN deadline never passes, so the
  // poll would never end; zero or less times out every check before it runs.
  if (!Number.isFinite(opts.timeoutSeconds) || opts.timeoutSeconds <= 0) {
    throw usageError(
      "--timeout must be a positive number of seconds",
      "e.g. --timeout 60 (the default is 300)"
    );
  }
  const target = ctx.target();
  const machine = await resolveCredentials(ctx, target, app);
  const creds: Creds = {
    target,
    bearer: machine.bearer,
    headers: machine.apiKey ? { "X-API-Key": machine.apiKey } : undefined,
    surface: machine.surface
  };

  // `resolveApp` pages `/api/admin/apps`, which a publish token may not reach:
  // sending it there would 403 on a route the caller cannot be given. The OIDC
  // branch never gets here (the exchange hands back the id); a token supplied
  // through `OXY_TOKEN` does, and it needs the UUID.
  if (!machine.appId && creds.surface === MACHINE_SURFACE && !UUID_RE.test(app)) {
    throw usageError(
      "a publish token cannot resolve <org>/<app> — pass the app UUID",
      "resolving a slug means listing every app, which a publish token may not do; the id is on the app's admin page, and `oxyc publish --json` reports it"
    );
  }
  const { appId, label } = machine.appId
    ? { appId: machine.appId, label: app }
    : await resolveApp(creds, app);
  const checks = await listChecks(creds, appId);
  if (checks.length === 0) {
    throw new CliError(`${label} declares no checks (no function has "check": true)`, {
      code: ExitCode.FAILURE,
      hint: `mark a function "check": true in oxy-app.json to give it one`
    });
  }

  const results: CheckResult[] = [];
  for (const fn of checks) {
    const result = await runOneCheck(creds, appId, fn.name, opts);
    results.push(result);
    if (!opts.json) printResultLine(result);
  }

  if (opts.json) {
    const report: ChecksReport = { app: label, appId, checks: results };
    process.stdout.write(`${JSON.stringify(report)}\n`);
  }

  const failed = results.filter((r) => !r.passed);
  if (failed.length > 0) {
    throw new CliError(
      `${failed.length} of ${results.length} check(s) failed: ${failed.map((r) => r.name).join(", ")}`,
      { code: ExitCode.CHECK_FAILED }
    );
  }
}

/** The prefix `oxyc publish` mints and the exchange returns. */
const PUBLISH_TOKEN_PREFIX = "oxypublish_";
/** Where the three function routes live for each kind of credential. */
const ADMIN_SURFACE = "/api/admin/apps";
const MACHINE_SURFACE = "/api/customer-apps";

interface ResolvedCredentials {
  bearer?: string;
  apiKey?: string;
  surface: string;
  /** Set only when the exchange told us which app the token is scoped to. */
  appId?: string;
}

/**
 * A stored credential if there is one, a minted one if there is not.
 *
 * Bearer wins when one resolves; otherwise the API key. Neither resolving is
 * not yet an error in CI: a job holding `id-token: write` can mint, so the
 * exchange is tried before giving up. Only when that is unavailable too does
 * this throw the SAME `authError` every other command throws — reusing
 * `ctx.bearer()` for that throw keeps the message and the `oxyc login …` hint
 * defined in exactly one place (`context/resolve.ts`) rather than duplicated
 * here.
 *
 * A publish token — minted here or handed in through `OXY_TOKEN` — reads the
 * machine surface, because the admin one refuses it.
 */
async function resolveCredentials(
  ctx: Context,
  target: string,
  app: string
): Promise<ResolvedCredentials> {
  const bearer = ctx.maybeBearer();
  if (bearer) {
    return {
      bearer,
      surface: bearer.startsWith(PUBLISH_TOKEN_PREFIX) ? MACHINE_SURFACE : ADMIN_SURFACE
    };
  }
  const apiKey = ctx.apiKey();
  if (apiKey) return { apiKey, surface: ADMIN_SURFACE };

  if (githubOidcAvailable()) {
    const [orgSlug, ...rest] = app.split("/");
    const appSlug = rest.join("/");
    if (!orgSlug || !appSlug) {
      throw usageError(
        'trusted publishing needs <app> as "<org-slug>/<app-slug>"',
        "the exchange is keyed by slug; a UUID names an app it cannot verify a publisher for"
      );
    }
    const minted = await exchangeGithubOidc(target, orgSlug, appSlug);
    // Only this caller needs the id — `oxyc publish` takes the token and goes.
    // So the version check is here, not in the exchange: a deployment without
    // the field can still be published to.
    if (!minted.appId) {
      throw new CliError("the OIDC exchange returned no app_id", {
        code: ExitCode.UNAVAILABLE,
        hint: "this deployment predates trusted checks — upgrade it, or set OXY_TOKEN and pass the app UUID"
      });
    }
    return { bearer: minted.token, surface: MACHINE_SURFACE, appId: minted.appId };
  }

  // Throws — nothing resolved and nothing can be minted.
  ctx.bearer();
  return { surface: ADMIN_SURFACE };
}

interface Creds {
  target: string;
  bearer?: string;
  headers?: Record<string, string>;
  /** `/api/admin/apps` for a human, `/api/customer-apps` for a publish token. */
  surface: string;
}

function ensureOk(response: Awaited<ReturnType<typeof request>>): void {
  if (response.status < 200 || response.status >= 300) throw errorForResponse(response);
}

/**
 * Resolve `<org-slug>/<app-slug>` to an app id by paging
 * `GET /api/admin/apps` — skipped entirely when `app` is already a UUID.
 */
async function resolveApp(creds: Creds, app: string): Promise<{ appId: string; label: string }> {
  if (UUID_RE.test(app)) return { appId: app, label: app };

  const [orgSlug, ...rest] = app.split("/");
  const appSlug = rest.join("/");

  let offset = 0;
  for (let page = 0; page < MAX_APP_PAGES; page++) {
    const response = await request({
      target: creds.target,
      path: `/api/admin/apps?limit=100&offset=${offset}`,
      method: "GET",
      bearer: creds.bearer,
      headers: creds.headers
    });
    ensureOk(response);
    const payload = parseJson(response.body) as AdminAppsPage | undefined;
    const match = (payload?.items ?? []).find(
      (item) => item.org_slug === orgSlug && item.slug === appSlug
    );
    if (match) return { appId: match.id, label: `${orgSlug}/${appSlug}` };
    if (payload?.next_offset == null) break;
    offset = payload.next_offset;
  }

  throw new CliError(`no app "${app}"`, {
    code: ExitCode.NOT_FOUND,
    hint: '<app> is "<org-slug>/<app-slug>" or an app UUID'
  });
}

/** `GET .../functions`, filtered to `check: true` and sorted by name. */
async function listChecks(creds: Creds, appId: string): Promise<FunctionSummary[]> {
  const response = await request({
    target: creds.target,
    path: `${creds.surface}/${appId}/functions`,
    method: "GET",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response);
  const payload = (parseJson(response.body) as FunctionSummary[] | undefined) ?? [];
  return payload.filter((fn) => fn.check === true).sort((a, b) => a.name.localeCompare(b.name));
}

/** POST the run, poll it to a terminal status or `timeoutSeconds`, and score it. */
async function runOneCheck(
  creds: Creds,
  appId: string,
  name: string,
  opts: { timeoutSeconds: number; pollMs?: number }
): Promise<CheckResult> {
  const start = Date.now();
  const runId = await startRun(creds, appId, name);
  const detail = await pollUntilTerminal(creds, appId, runId, opts);
  const durationMs = Date.now() - start;

  const answerFailed = parsesToOkFalse(detail.answer);
  const passed = detail.status === "done" && !answerFailed;
  const error = passed ? undefined : failureMessage(detail, answerFailed, opts.timeoutSeconds);

  return { name, runId, status: detail.status, passed, error, durationMs };
}

async function startRun(creds: Creds, appId: string, name: string): Promise<string> {
  const response = await request({
    target: creds.target,
    path: `${creds.surface}/${appId}/functions/${encodeURIComponent(name)}/runs`,
    method: "POST",
    body: "{}",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response);
  const payload = parseJson(response.body) as { run_id?: string } | undefined;
  if (!payload?.run_id) {
    throw new CliError(`POST .../functions/${name}/runs did not return a run_id`, {
      code: ExitCode.UNAVAILABLE
    });
  }
  return payload.run_id;
}

async function getRunDetail(creds: Creds, appId: string, runId: string): Promise<RunDetail> {
  const response = await request({
    target: creds.target,
    path: `${creds.surface}/${appId}/function-runs/${runId}`,
    method: "GET",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response);
  return parseJson(response.body) as RunDetail;
}

/** Poll every `pollMs` (default 2000) until a terminal status, or the client-side timeout. */
async function pollUntilTerminal(
  creds: Creds,
  appId: string,
  runId: string,
  opts: { timeoutSeconds: number; pollMs?: number }
): Promise<RunDetail> {
  const deadline = Date.now() + opts.timeoutSeconds * 1000;
  for (;;) {
    const detail = await getRunDetail(creds, appId, runId);
    if (TERMINAL.has(detail.status)) return detail;
    if (Date.now() >= deadline) return { ...detail, status: "timed_out" };
    await sleep(opts.pollMs ?? 2000);
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** A check fails when its answer parses to an object whose `ok` is `false`. */
function parsesToOkFalse(answer: string | null | undefined): boolean {
  if (!answer) return false;
  const parsed = parseJson(answer);
  return (
    typeof parsed === "object" &&
    parsed !== null &&
    !Array.isArray(parsed) &&
    (parsed as Record<string, unknown>).ok === false
  );
}

function failureMessage(detail: RunDetail, answerFailed: boolean, timeoutSeconds: number): string {
  if (detail.error) return detail.error;
  if (detail.status === "timed_out") return `timed out after ${timeoutSeconds}s`;
  if (answerFailed) return `answer reported "ok": false`;
  if (detail.status === "cancelled") return "run cancelled";
  return `status ${detail.status}`;
}

function printResultLine(result: CheckResult): void {
  if (process.env.OXYC_QUIET) return;
  const seconds = `${(result.durationMs / 1000).toFixed(1)}s`;
  if (result.passed) {
    process.stderr.write(`${err.green("✓")} ${result.name} ${seconds}\n`);
    return;
  }
  const reason = (result.error ?? result.status).slice(0, 200);
  process.stderr.write(`${err.red("✗")} ${result.name} failed: ${reason}\n`);
}
