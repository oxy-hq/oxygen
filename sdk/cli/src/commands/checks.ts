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
 * Talks only to four admin routes (`GET /api/admin/apps`, `GET .../functions`,
 * `POST .../functions/{name}/runs`, `GET .../function-runs/{run_id}`); nothing
 * here decides whether a function IS a check — that is `FunctionSummary.check`,
 * which the server projects from the manifest flag.
 */

import { errorForResponse, parseJson, request } from "../api/request.js";
import type { Context } from "../context/resolve.js";
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
  const { bearer, apiKey } = resolveCredentials(ctx);
  const headers = apiKey ? { "X-API-Key": apiKey } : undefined;
  const creds = { target, bearer, headers };

  const { appId, label } = await resolveApp(creds, app);
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

/**
 * Bearer wins when one resolves; otherwise the API key. Neither resolving
 * throws the SAME `authError` every other command throws — reusing
 * `ctx.bearer()` for that throw keeps the message and the `oxyc login …` hint
 * defined in exactly one place (`context/resolve.ts`) rather than duplicated
 * here. `ctx.bearer()` itself is not called first because it throws the
 * moment nothing resolves, which would skip the API-key fallback below.
 */
function resolveCredentials(ctx: Context): { bearer?: string; apiKey?: string } {
  const bearer = ctx.maybeBearer();
  const apiKey = bearer ? undefined : ctx.apiKey();
  if (!bearer && !apiKey) ctx.bearer();
  return { bearer, apiKey };
}

interface Creds {
  target: string;
  bearer?: string;
  headers?: Record<string, string>;
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
    path: `/api/admin/apps/${appId}/functions`,
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
    path: `/api/admin/apps/${appId}/functions/${encodeURIComponent(name)}/runs`,
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
    path: `/api/admin/apps/${appId}/function-runs/${runId}`,
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
