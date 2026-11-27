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
import {
  APPS_API,
  type AppByCredential,
  appForCredentials,
  resolveCredentials
} from "./checks-credentials.js";

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
  /** How the invocation failed, as stored with it. Absent on an older server. */
  error_kind?: string | null;
  /** The run's durable-queue entry. Absent from a deployment that predates it. */
  queue?: RunQueueEntry | null;
}

/**
 * `FunctionRunDetail.queue` — the queue entry the run's status came from.
 *
 * This is the only thing that distinguishes "the function failed and said
 * nothing" from "nothing ever ran the function". A task that exhausts
 * `max_claims` is dead-lettered, the server maps a `dead` entry to run status
 * `failed`, and the run row carries no error because no attempt ever reported
 * one. `failureMessage` reads it for exactly that case.
 */
interface RunQueueEntry {
  /** `queued` | `claimed` | `completed` | `dead`. */
  status?: string | null;
  claim_count?: number | null;
  max_claims?: number | null;
  queued_at?: string | null;
  last_heartbeat?: string | null;
  updated_at?: string | null;
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

  // `--browser` needs the slugs for the page's URL. A sandbox agent token's
  // app came off its own list (`machine.appId`, no slugs), and `resolveApp`
  // reads the same remembered list — no listing, no request. Every other case
  // is `appForCredentials`, which keeps a machine token off the app listing.
  const needsSlugs = opts.browser && isSandboxAgentToken(machine.bearer);
  const resolved: ResolvedApp | AppByCredential = needsSlugs
    ? await resolveApp(creds, app)
    : await appForCredentials(creds, machine, app);
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
    if (resolved.orgSlug === undefined) {
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

/**
 * Why a dead-lettered run has no error, or `undefined` if that is not what
 * happened.
 *
 * `status failed` on a run that recorded nothing is the one failure this
 * command used to report as a bare status, and it is the one where the status
 * is actively misleading: the function neither ran nor failed. Its task was
 * claimed, its worker stopped heartbeating, the reaper requeued it, and on the
 * `max_claims`-th time there was nothing left to retry with. Nobody can act on
 * that without the claim count and the two timestamps that bracket the
 * disappearance — the last heartbeat and the dead-letter — which is what a
 * release gate's one red line has to carry.
 *
 * It names the last attempt's invocation too, where the server reports one.
 * An abandoned attempt leaves a `running` invocation row behind, and that
 * row's call trace is the only record of what it was blocked on when its
 * heartbeat stopped — the difference between knowing the fleet lost the task
 * and knowing which host call hung.
 */
function deadLetterMessage(
  queue: RunQueueEntry | null | undefined,
  invocationId: string | null | undefined
): string | undefined {
  if (queue?.status !== "dead") return undefined;
  const claims =
    typeof queue.claim_count === "number" && typeof queue.max_claims === "number"
      ? `after ${queue.claim_count} of ${queue.max_claims} claims`
      : "after exhausting its claims";
  // Each one is the evidence for a different question: queued/dead-lettered is
  // how long it sat, last heartbeat is when the instance holding it went away.
  const stamps: [string, string | null | undefined][] = [
    ["queued", queue.queued_at],
    ["last heartbeat", queue.last_heartbeat],
    ["dead-lettered", queue.updated_at]
  ];
  const when = stamps.flatMap(([label, at]) => (at ? [`${label} ${at}`] : [])).join(", ");
  const detail = when ? ` (${when})` : "";
  const attempt = invocationId ? `; last attempt invocation ${invocationId}, read its calls` : "";
  return (
    `dead-lettered ${claims}: every worker that claimed it stopped heartbeating, so the ` +
    `function never ran to a verdict — this is the worker fleet, not the app${detail}${attempt}`
  );
}

/**
 * Why a check did not pass, in the order the reasons are worth reading: the
 * run's own message, then the two verdicts the status and the answer carry,
 * then what the queue can explain, then whatever is in hand.
 */
function failureMessage(detail: RunDetail, answerFailed: boolean, timeoutSeconds: number): string {
  if (detail.error) return detail.error;
  if (detail.status === "timed_out") return `timed out after ${timeoutSeconds}s`;
  if (answerFailed) return `answer reported "ok": false`;
  if (detail.status === "cancelled") return "run cancelled";
  // No error on the run is itself a fact about the run, so say which fact it
  // is. A dead-letter is the one fact the queue can explain by itself; past
  // that, name whatever is in hand, because the run that reaches this floor is
  // the one nobody can otherwise diagnose. `error_kind` only helps below the
  // dead-letter branch: a task the queue gave up on never ran to a verdict, so
  // its invocation carries no kind.
  const deadLettered = deadLetterMessage(detail.queue, detail.invocation_id);
  if (deadLettered) return deadLettered;
  const named = [
    detail.error_kind ? `kind ${detail.error_kind}` : undefined,
    `run ${detail.run_id}`,
    detail.invocation_id ? `invocation ${detail.invocation_id}` : undefined
  ].filter((part): part is string => part !== undefined);
  return `status ${detail.status} with no reason recorded (${named.join(", ")})`;
}

function printResultLine(result: CheckResult | BrowserCheckResult): void {
  if (process.env.OXYC_QUIET) return;
  const seconds = `${(result.durationMs / 1000).toFixed(1)}s`;
  if (result.passed) {
    process.stderr.write(`${err.green("✓")} ${result.name} ${seconds}\n`);
    return;
  }
  // 400, not the 200 this started at. The cap is there so a function that
  // throws a stack trace does not own the terminal; it is not there to clip a
  // reason this command built. The dead-letter message runs to ~280 with its
  // three timestamps, and truncating it drops exactly the part an operator
  // correlates against a worker rollout. A real function error over 400 is
  // still in `--json` and in the run's logs, and the cut is marked, so a
  // clipped reason cannot read as a complete one.
  const MAX_REASON = 400;
  const full = result.error ?? result.status;
  const reason = full.length > MAX_REASON ? `${full.slice(0, MAX_REASON)}…` : full;
  process.stderr.write(`${err.red("✗")} ${result.name} failed: ${reason}\n`);
}
