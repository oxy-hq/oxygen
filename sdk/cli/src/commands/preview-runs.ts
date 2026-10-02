/**
 * `oxyc preview run` / `oxyc preview runs` — held dry runs of a previewed
 * branch (`internal-docs/workspace-previews.md`). `POST /previews/runs`
 * accepts only `kind: "procedure"` and `kind: "airway_sample"` — the server
 * refuses anything else with `400 bad_request` (`server::previews::runs::
 * submit::submit`). `transform_build` and `compare` runs are queued by the
 * server's own change check and never started here; `runPreviewRunsList` /
 * `runPreviewRunShow` read them back the same as any other kind.
 *
 * All routes answer `404 preview_runs_disabled` while the deployment has not
 * set `OXY_PREVIEW_RUNS` — surfaced like any other server refusal, not
 * special-cased here (`errorForResponse` already carries the server's own
 * `code` on the thrown `CliError`).
 */

import { parseJson, request } from "../api/request.js";
import { ensureOk } from "../apps/resolve.js";
import type { Context } from "../context/resolve.js";
import { previewsPath } from "../previews/resolve.js";
import { resolveDataInput } from "../util/data-input.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";

const SUBMITTABLE_KINDS = ["procedure", "airway_sample"] as const;
export type RunKind = (typeof SUBMITTABLE_KINDS)[number];

/** `procedure` or `airway_sample` — the only kinds `POST /previews/runs` accepts. */
export function requireRunKind(value: string): RunKind {
  if (value === "procedure" || value === "airway_sample") return value;
  throw usageError(
    `"${value}" is not a run kind oxyc can start`,
    "procedure or airway_sample — transform_build and compare runs are queued automatically " +
      "by the server's change check; read one back with `oxyc preview runs show <run-id>`"
  );
}

export interface RunWindow {
  from?: string;
  to?: string;
}

export interface SubmitRunParams {
  branch: string;
  kind: RunKind;
  ref: string;
  variables?: unknown;
  readLiveOnly?: boolean;
  window?: RunWindow;
  resources?: string[];
}

export interface Submitted {
  run_id: string;
  state: string;
}

/** `GET /previews/runs?branch=` item. */
export interface RunSummary {
  run_id: string;
  branch: string;
  /** `procedure` | `transform_build` | `compare` | `airway_sample`. */
  kind: string;
  target_ref: string | null;
  parent_run_id: string | null;
  revision_id: string;
  /** `queued` | `running` | `finished`. */
  state: string;
  /** `succeeded` | `failed` | `cancelled`, once finished. */
  outcome: string | null;
  held_count: number;
  requested_by: string | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
}

/** `GET /previews/runs/{run_id}`. */
export interface RunDetail extends RunSummary {
  agentic_run_id: string | null;
  error: string | null;
  steps: unknown[];
  /** A `transform_build`'s compare with live, or a `compare` run's own. */
  compare: unknown | null;
  /** An `airway_sample`'s ask and result. */
  sample: unknown | null;
}

/**
 * Whether a (terminal) run is a failure rather than a success — the ONE
 * place that decision is made, so `runPreviewRun`/`runPreviewRunShow` (exit
 * code) and `oxy_preview_run`/`oxy_preview_run_show` (`mcp.ts`'s `isError`)
 * can never disagree, and `cancelled` (e.g. the preview was deleted mid-run)
 * counts as a failure everywhere, not just where someone remembered it.
 */
export function runFailed(detail: RunSummary): boolean {
  return detail.outcome === "failed" || detail.outcome === "cancelled";
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
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

/** `POST /previews/runs` — record the run and start it if the workspace's queue is free. */
export async function previewSubmitRun(ctx: Context, params: SubmitRunParams): Promise<Submitted> {
  const body = {
    branch: params.branch,
    kind: params.kind,
    ref: params.ref,
    variables: params.variables,
    read_live_only: params.readLiveOnly ?? false,
    window: params.window,
    resources: params.resources ?? []
  };
  const response = await request({
    target: ctx.target(),
    path: previewsPath(ctx, "/runs"),
    method: "POST",
    body: JSON.stringify(body),
    bearer: ctx.bearer()
  });
  ensureOk(response);
  return parseJson(response.body) as Submitted;
}

export async function previewRunsList(ctx: Context, branch: string): Promise<RunSummary[]> {
  const response = await request({
    target: ctx.target(),
    path: `${previewsPath(ctx, "/runs")}?branch=${encodeURIComponent(branch)}`,
    method: "GET",
    bearer: ctx.bearer()
  });
  ensureOk(response);
  return (parseJson(response.body) as RunSummary[] | undefined) ?? [];
}

async function fetchRunDetail(ctx: Context, runId: string): Promise<RunDetail> {
  const response = await request({
    target: ctx.target(),
    path: previewsPath(ctx, `/runs/${encodeURIComponent(runId)}`),
    method: "GET",
    bearer: ctx.bearer()
  });
  ensureOk(response);
  return parseJson(response.body) as RunDetail;
}

/**
 * Poll from an already-fetched snapshot (`initial`) to a terminal state.
 * Checks `initial` FIRST, with no network call — the caller already paid for
 * that fetch — and only calls the server again after sleeping, so a waited
 * call makes exactly one fetch per real poll interval rather than one wasted
 * extra fetch up front that (having had no time to change) almost always
 * just re-confirms what `initial` already said.
 */
async function pollRunToTerminal(
  ctx: Context,
  runId: string,
  initial: RunDetail,
  waitSeconds: number,
  pollMs: number
): Promise<RunDetail> {
  const deadline = Date.now() + waitSeconds * 1000;
  let detail = initial;
  for (;;) {
    if (detail.state === "finished") return detail;
    if (Date.now() >= deadline) {
      throw new CliError(`preview run ${runId} did not finish within ${waitSeconds}s`, {
        code: ExitCode.UNAVAILABLE,
        hint: "the run executes on the worker fleet — retry `oxyc preview runs show … --wait`, or check it later"
      });
    }
    await sleep(pollMs);
    detail = await fetchRunDetail(ctx, runId);
  }
}

/**
 * One run's detail. With `waitSeconds`, blocks until `state` is `finished`
 * rather than returning a `queued`/`running` snapshot.
 */
export async function previewRunGet(
  ctx: Context,
  runId: string,
  opts: { waitSeconds?: number; pollMs?: number } = {}
): Promise<RunDetail> {
  validateWaitSeconds(opts.waitSeconds);
  const detail = await fetchRunDetail(ctx, runId);
  if (opts.waitSeconds === undefined || detail.state === "finished") return detail;
  return pollRunToTerminal(ctx, runId, detail, opts.waitSeconds, opts.pollMs ?? 2000);
}

/**
 * `-`/`@file`/literal JSON, same grammar `fn call --data` uses. Exported so
 * `oxy_preview_run` (mcp.ts) validates `variables` the same way this verb
 * does, rather than through the generic `parseJson` (`api/request.ts`),
 * which SWALLOWS a parse error and returns `undefined` — silently dropping
 * the variables from a real held run instead of refusing.
 */
export function parseVariables(raw: string | undefined): unknown {
  if (raw === undefined) return undefined;
  const text = resolveDataInput(raw);
  try {
    return JSON.parse(text);
  } catch (cause) {
    throw usageError(`--variables is not valid JSON: ${(cause as Error).message}`);
  }
}

function printSubmitted(result: Submitted, asJson: boolean): void {
  if (asJson) {
    process.stdout.write(`${JSON.stringify(result)}\n`);
    return;
  }
  process.stdout.write(`run ${result.run_id} — ${result.state}\n`);
}

function printRunDetail(detail: RunDetail, asJson: boolean): void {
  if (asJson) {
    process.stdout.write(`${JSON.stringify(detail)}\n`);
    return;
  }
  const outcome = detail.outcome ? ` (${detail.outcome})` : "";
  process.stdout.write(`${detail.run_id} (${detail.kind}) — ${detail.state}${outcome}\n`);
  if (detail.error) process.stdout.write(`  error: ${detail.error}\n`);
}

export interface RunFlags {
  variables?: string;
  readLiveOnly?: boolean;
  windowFrom?: string;
  windowTo?: string;
  resource?: string[];
  waitSeconds?: number;
  pollMs?: number;
  json: boolean;
}

export async function runPreviewRun(
  ctx: Context,
  branch: string,
  kindRaw: string,
  ref: string,
  opts: RunFlags
): Promise<void> {
  const kind = requireRunKind(kindRaw);
  const window =
    opts.windowFrom !== undefined || opts.windowTo !== undefined
      ? { from: opts.windowFrom, to: opts.windowTo }
      : undefined;
  const params: SubmitRunParams = {
    branch,
    kind,
    ref,
    variables: parseVariables(opts.variables),
    readLiveOnly: opts.readLiveOnly,
    window,
    resources: opts.resource
  };

  const submitted = await previewSubmitRun(ctx, params);
  if (opts.waitSeconds === undefined) {
    printSubmitted(submitted, opts.json);
    return;
  }
  const detail = await previewRunGet(ctx, submitted.run_id, {
    waitSeconds: opts.waitSeconds,
    pollMs: opts.pollMs
  });
  printRunDetail(detail, opts.json);
  if (runFailed(detail)) {
    throw new CliError(`preview run ${detail.run_id} ${detail.outcome}`, {
      code: ExitCode.FAILURE
    });
  }
}

export async function runPreviewRunsList(
  ctx: Context,
  branch: string,
  opts: { json: boolean }
): Promise<void> {
  const runs = await previewRunsList(ctx, branch);
  if (opts.json) {
    process.stdout.write(`${JSON.stringify({ runs })}\n`);
    return;
  }
  if (runs.length === 0) {
    process.stderr.write("no runs\n");
    return;
  }
  for (const r of runs) {
    process.stdout.write(
      `${r.run_id} ${r.kind.padEnd(16)} ${r.state.padEnd(9)} ${r.outcome ?? ""}\n`
    );
  }
}

export async function runPreviewRunShow(
  ctx: Context,
  runId: string,
  opts: { waitSeconds?: number; json: boolean; pollMs?: number }
): Promise<void> {
  const detail = await previewRunGet(ctx, runId, opts);
  printRunDetail(detail, opts.json);
  // Only on the AWAITED path: a plain (non-waited) show is a read-back of
  // whatever a run's history already is, same as `oxyc env show` or
  // `invocations held` — it must not start failing on old, already-finished
  // runs just because someone looked at them.
  if (opts.waitSeconds !== undefined && runFailed(detail)) {
    throw new CliError(`preview run ${runId} ${detail.outcome}`, { code: ExitCode.FAILURE });
  }
}
