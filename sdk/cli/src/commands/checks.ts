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
 * Talks only to four routes (`GET /api/customer-apps`, `GET .../functions`,
 * `POST .../functions/{name}/runs`, `GET .../function-runs/{run_id}`); nothing
 * here decides whether a function IS a check — that is `FunctionSummary.check`,
 * which the server projects from the manifest flag.
 *
 * One surface for every credential: `/api/customer-apps/…`. A human runs this
 * with their own credential. CI has no credential to store: in a job with
 * `id-token: write` it exchanges a GitHub OIDC token for a short-lived token —
 * a service account's, or the app-scoped publish token `oxyc publish` falls
 * back to. The app-listing route is the one a machine token may not call (it
 * may not enumerate apps), which is why the app id comes from the credential
 * rather than a lookup. Which credential, and in
 * what order: `checks-credentials.ts`.
 */

import { parseJson, request } from "../api/request.js";
import { parseAppEnv } from "../apps/environment.js";
import {
  type Creds as BaseCreds,
  ensureOk,
  type ResolvedApp,
  resolveApp
} from "../apps/resolve.js";
import { githubOidcAvailable } from "../auth/oidc.js";
import { isSandboxAgentToken } from "../auth/token-kind.js";
import type { BrowserCheckResult } from "../checks/browser.js";
import { refuseMachineBrowser, runBrowserStep } from "../checks/browser-step.js";
import type { Context } from "../context/resolve.js";
import { err } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { APPS_API, resolveCredentials, UUID_RE } from "./checks-credentials.js";

/** The `--json` report: one object on stdout. */
export interface ChecksReport {
  app: string;
  appId: string;
  /** The function checks, then — with `--browser` — the browser check (`kind: "browser"`). */
  checks: (CheckResult | BrowserCheckResult)[];
  /** Only present with `--app-env` — absent, the report is today's shape. */
  environment?: string;
}

export interface ChecksOptions {
  timeoutSeconds: number;
  pollMs?: number;
  appEnv?: string;
  /** Also load the app in a browser, in `appEnv` (`checks/browser.ts`). */
  browser?: boolean;
  /** With `browser`: a CSS selector that must become visible once the app is ready. */
  expectSelector?: string;
}

export interface CheckResult {
  name: string;
  runId: string;
  status: string;
  passed: boolean;
  error?: string;
  durationMs: number;
  /** From the run detail; only present once the run has started. */
  invocationId?: string;
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
  /** New with sandboxes: null until the run starts. */
  invocation_id?: string | null;
}

const TERMINAL: ReadonlySet<RunStatus> = new Set(["done", "failed", "cancelled", "timed_out"]);

/**
 * Run every check and return the report — no printing, and no throw for a
 * failed check: that decision (and the CLI's streaming `✓`/`✗` lines) belongs
 * to the caller. `onResult`, called as each check lands, is how `runChecks`
 * keeps its streaming feedback without a second loop; the `oxyc mcp` tool
 * passes none and just reads the returned report.
 */
export async function runChecksCore(
  ctx: Context,
  app: string,
  opts: ChecksOptions,
  onResult?: (result: CheckResult | BrowserCheckResult) => void
): Promise<ChecksReport> {
  // `main.ts` passes `Number(--timeout)`. A NaN deadline never passes, so the
  // poll would never end; zero or less times out every check before it runs.
  if (!Number.isFinite(opts.timeoutSeconds) || opts.timeoutSeconds <= 0) {
    throw usageError(
      "--timeout must be a positive number of seconds",
      "e.g. --timeout 60 (the default is 300)"
    );
  }
  if (opts.expectSelector !== undefined && !opts.browser) {
    throw usageError("--expect-selector needs --browser", "it is what the browser check waits for");
  }
  // Usage, before any request: a malformed name, same grammar every verb uses.
  const appEnv = opts.appEnv !== undefined ? parseAppEnv(opts.appEnv) : undefined;
  if (opts.browser) refuseMachineBrowser(ctx, githubOidcAvailable());

  const target = ctx.target();
  const machine = await resolveCredentials(ctx, target, app, appEnv);
  const creds: Creds = {
    target,
    bearer: machine.bearer,
    headers: machine.apiKey ? { "X-API-Key": machine.apiKey } : undefined,
    machine: machine.machine
  };

  // `resolveApp` pages `/api/customer-apps`, which a machine token may not list:
  // sending it there would 403 on a route the caller cannot be given. A token
  // that names its own app (the publisher exchange's `app_id`, a service
  // account's `app_publish` grant) never gets here; one that does not needs
  // the UUID.
  if (!machine.appId && creds.machine && !UUID_RE.test(app)) {
    throw usageError(
      "this token cannot resolve <org>/<app> — pass the app UUID",
      "resolving a slug means listing every app, which a publish or service-account token may not do; the id is on the app's admin page, and `oxyc publish --json` reports it"
    );
  }
  // `--browser` needs the slugs for the page's URL. A sandbox agent token's
  // app came off its own list (`machine.appId`, no slugs), and `resolveApp`
  // reads the same remembered list — no listing, no request.
  const needsSlugs = opts.browser && isSandboxAgentToken(machine.bearer);
  const resolved: ResolvedApp | { appId: string; label: string } = needsSlugs
    ? await resolveApp(creds, app)
    : machine.appId
      ? { appId: machine.appId, label: app }
      : creds.machine && UUID_RE.test(app)
        ? // Never touch `/api/customer-apps/{id}` with a machine token — it is
          // refused there just as the listing is, and the id already names the
          // app, so there is nothing left for `resolveApp` to add.
          { appId: app, label: app }
        : await resolveApp(creds, app);
  const { appId, label } = resolved;
  const checks = await listChecks(creds, appId, appEnv);
  // With `--browser` the page is itself a check, so an app with no check
  // functions still has one to run.
  if (checks.length === 0 && !opts.browser) {
    throw new CliError(`${label} declares no checks (no function has "check": true)`, {
      code: ExitCode.FAILURE,
      hint: `mark a function "check": true in oxy-app.json to give it one, or pass --browser`
    });
  }

  const results: (CheckResult | BrowserCheckResult)[] = [];
  for (const fn of checks) {
    const result = await runOneCheck(creds, appId, fn.name, appEnv, opts);
    results.push(result);
    onResult?.(result);
  }
  if (opts.browser) {
    // `refuseMachineBrowser` kept every machine token out, and a sandbox
    // agent token was resolved by `resolveApp` above, so the app carries its
    // slugs. Should a new path skip that, say so rather than report a run with
    // no browser check.
    if (!("orgSlug" in resolved)) {
      throw new CliError(`cannot load ${label} in a browser: its slugs were not resolved`, {
        code: ExitCode.USAGE,
        hint: "--browser needs a user credential: oxyc login, OXY_TOKEN set to a user token, or an API key"
      });
    }
    const result = await runBrowserStep(creds, resolved, { ...opts, appEnv });
    results.push(result);
    onResult?.(result);
  }

  return { app: label, appId, checks: results, environment: appEnv };
}

export async function runChecks(
  ctx: Context,
  app: string,
  opts: ChecksOptions & { json: boolean }
): Promise<void> {
  const report = await runChecksCore(ctx, app, opts, opts.json ? undefined : printResultLine);

  if (opts.json) {
    process.stdout.write(`${JSON.stringify(report)}\n`);
  }

  const failed = report.checks.filter((r) => !r.passed);
  if (failed.length > 0) {
    throw new CliError(
      `${failed.length} of ${report.checks.length} check(s) failed: ${failed.map((r) => r.name).join(", ")}`,
      { code: ExitCode.CHECK_FAILED }
    );
  }
}

interface Creds extends BaseCreds {
  /** A publish, service-account or sandbox agent token: it may not list apps. */
  machine: boolean;
}

/** `?environment=<name>`, or nothing — appended to all three check routes. */
function environmentQuery(appEnv: string | undefined): string {
  return appEnv === undefined ? "" : `?environment=${encodeURIComponent(appEnv)}`;
}

/** `GET .../functions`, filtered to `check: true` and sorted by name. */
async function listChecks(
  creds: Creds,
  appId: string,
  appEnv: string | undefined
): Promise<FunctionSummary[]> {
  const response = await request({
    target: creds.target,
    path: `${APPS_API}/${appId}/functions${environmentQuery(appEnv)}`,
    method: "GET",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  const payload = (parseJson(response.body) as FunctionSummary[] | undefined) ?? [];
  return payload.filter((fn) => fn.check === true).sort((a, b) => a.name.localeCompare(b.name));
}

/** POST the run, poll it to a terminal status or `timeoutSeconds`, and score it. */
async function runOneCheck(
  creds: Creds,
  appId: string,
  name: string,
  appEnv: string | undefined,
  opts: { timeoutSeconds: number; pollMs?: number }
): Promise<CheckResult> {
  const start = Date.now();
  const runId = await startRun(creds, appId, name, appEnv);
  const detail = await pollUntilTerminal(creds, appId, runId, appEnv, opts);
  const durationMs = Date.now() - start;

  const answerFailed = parsesToOkFalse(detail.answer);
  const passed = detail.status === "done" && !answerFailed;
  const error = passed ? undefined : failureMessage(detail, answerFailed, opts.timeoutSeconds);

  return {
    name,
    runId,
    status: detail.status,
    passed,
    error,
    durationMs,
    invocationId: detail.invocation_id ?? undefined
  };
}

async function startRun(
  creds: Creds,
  appId: string,
  name: string,
  appEnv: string | undefined
): Promise<string> {
  const response = await request({
    target: creds.target,
    path: `${APPS_API}/${appId}/functions/${encodeURIComponent(name)}/runs${environmentQuery(appEnv)}`,
    method: "POST",
    body: "{}",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  const payload = parseJson(response.body) as { run_id?: string } | undefined;
  if (!payload?.run_id) {
    throw new CliError(`POST .../functions/${name}/runs did not return a run_id`, {
      code: ExitCode.UNAVAILABLE
    });
  }
  return payload.run_id;
}

async function getRunDetail(
  creds: Creds,
  appId: string,
  runId: string,
  appEnv: string | undefined
): Promise<RunDetail> {
  const response = await request({
    target: creds.target,
    path: `${APPS_API}/${appId}/function-runs/${runId}${environmentQuery(appEnv)}`,
    method: "GET",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  return parseJson(response.body) as RunDetail;
}

/** Poll every `pollMs` (default 2000) until a terminal status, or the client-side timeout. */
async function pollUntilTerminal(
  creds: Creds,
  appId: string,
  runId: string,
  appEnv: string | undefined,
  opts: { timeoutSeconds: number; pollMs?: number }
): Promise<RunDetail> {
  const deadline = Date.now() + opts.timeoutSeconds * 1000;
  for (;;) {
    const detail = await getRunDetail(creds, appId, runId, appEnv);
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

function printResultLine(result: CheckResult | BrowserCheckResult): void {
  if (process.env.OXYC_QUIET) return;
  const seconds = `${(result.durationMs / 1000).toFixed(1)}s`;
  if (result.passed) {
    process.stderr.write(`${err.green("✓")} ${result.name} ${seconds}\n`);
    return;
  }
  const reason = (result.error ?? result.status).slice(0, 200);
  process.stderr.write(`${err.red("✗")} ${result.name} failed: ${reason}\n`);
}
