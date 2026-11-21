/**
 * `oxyc airway run <pipeline_ref>` — start a pipeline on the deployment and
 * stream it to the end.
 *
 *   POST /api/{workspace_id}/agentic-airway/runs             { pipeline_ref, resources }
 *   GET  /api/{workspace_id}/agentic-airway/runs/{id}/events  SSE
 *   GET  /api/{workspace_id}/agentic-airway/runs?pipeline_ref= the run's status, afterwards
 *
 * THE BEHAVIOUR CHANGE FROM `oxy airway run`: the server runs the pipeline it
 * has COMPILED for the workspace's served revision, queued for the worker
 * fleet, not a `.airway.yml` on this machine. To try an unpushed or unmerged
 * edit, push a branch and run `oxyc preview run <branch> airway_sample <ref>`.
 * Nothing from this shell's environment is sent. The Rust command filled a
 * pipeline's `{{ VAR }}` templates from it; here they render from `--variables`
 * alone (the route's `variables`, carried on the queued run and its metadata,
 * so never a secret: credentials go through `*_var` keys and the workspace's
 * secrets). A template with no value fails the run. Ctrl-C stops watching,
 * not the run — and no longer strands the single-flight lease for six hours.
 *
 * THE EXIT CODE IS THE RUN ROW'S, as it was in Rust: the stream's terminal
 * event says the stream is over, not how the run ended (`load_completed` ends
 * it on a run whose row is still being finalised). So once the stream ends the
 * row is read back, and only when it cannot be found is the event used.
 */

import { authHeaders, buildUrl, errorForResponse } from "../api/request.js";
import { withUserAgent } from "../api/user-agent.js";
import type { Context } from "../context/resolve.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { printable } from "../util/printable.js";
import { airwayCall, airwayPath, sleep } from "./airway-client.js";
import { SseParser } from "./logs-follow.js";
import { parseVariables } from "./preview-runs.js";

/** Run-row statuses that end the wait — the runtime's own terminal set. */
export const TERMINAL_STATUSES = ["done", "failed", "cancelled", "timed_out"] as const;

/**
 * The subset of {@link TERMINAL_STATUSES} that exits non-zero. A partial
 * failure is `done` on the row (`completed_with_errors` is a backfill-range
 * status, never a run's), so it exits 0, as it did in Rust.
 */
export const FAILED_STATUSES = ["failed", "cancelled", "timed_out"] as const;

export function isTerminal(status: string | undefined | null): boolean {
  return status != null && (TERMINAL_STATUSES as readonly string[]).includes(status);
}

export function isFailed(status: string): boolean {
  return (FAILED_STATUSES as readonly string[]).includes(status);
}

/**
 * The SSE events that end an airway stream — `agentic_http::sse::is_terminal`
 * for `source_type == "airway"`.
 */
const TERMINAL_EVENTS = [
  "done",
  "error",
  "cancelled",
  "load_completed",
  "pipeline_error",
  "task_failed"
];
const FAILURE_EVENTS = ["error", "cancelled", "pipeline_error", "task_failed"];

export function isTerminalEvent(event: string): boolean {
  return TERMINAL_EVENTS.includes(event);
}

/** Reconnects in a row with no event between them before giving up (exit 7). */
const MAX_RECONNECTS = 5;

export interface RunFlags {
  json: boolean;
  /** `--variables`: JSON, `@file` or `-`, as `oxyc preview run` takes it. */
  variables?: string;
  resources?: string[];
  /** Start, print the run id, and return without streaming. */
  detach?: boolean;
}

interface RunSummary {
  run_id: string;
  status: string;
}

/** One line of `--json` output: the shape `oxy airway run --json` printed. */
function emitJson(seq: number, eventType: string, payload: unknown): void {
  process.stdout.write(`${JSON.stringify({ seq, event_type: eventType, payload })}\n`);
}

const str = (v: unknown, fallback = "?") => (typeof v === "string" ? printable(v) : fallback);
const num = (v: unknown) => (typeof v === "number" ? v : 0);

/** The human rendering, `oxy airway run`'s `emit`. */
export function renderEvent(eventType: string, payload: Record<string, unknown>): string {
  switch (eventType) {
    case "load_started":
      return "▶ load started";
    case "extract_completed":
      return `  extracted ${str(payload.table)} (${num(payload.rows_extracted)} rows)`;
    case "normalize_completed":
      return `  normalized ${str(payload.table)}`;
    case "destination_load_started":
      return "  loading into destination…";
    case "load_completed":
      return `✓ load completed (${num(payload.duration_ms)} ms)`;
    case "schema_evolved":
      return "• schema evolved";
    case "pipeline_error":
      return `✗ pipeline error: ${str(payload.error)}`;
    case "task_failed":
    case "error":
      return `✗ ${eventType}: ${str(payload.message ?? payload.error)}`;
    case "cancelled":
      return "■ cancelled";
    default:
      return `  ${eventType}: ${JSON.stringify(payload)}`;
  }
}

function printEvent(json: boolean, seq: number, eventType: string, payload: unknown): void {
  if (json) {
    emitJson(seq, eventType, payload);
    return;
  }
  const data = payload && typeof payload === "object" ? (payload as Record<string, unknown>) : {};
  const line = renderEvent(eventType, data);
  const failed = FAILURE_EVENTS.includes(eventType);
  process.stdout.write(`${failed ? out.red(line) : line}\n`);
}

export async function runAirwayRun(
  ctx: Context,
  pipelineRef: string,
  flags: RunFlags
): Promise<void> {
  const ref = pipelineRef.trim();
  if (!ref) throw usageError("name the pipeline: oxyc airway run <path/to/x.airway.yml>");
  const variables = parseVariables(flags.variables);
  const started = await airwayCall<{ run_id: string }>(ctx, "POST", "/runs", {
    pipeline_ref: ref,
    resources: flags.resources ?? [],
    variables
  });
  if (!started?.run_id) {
    throw new CliError("POST …/agentic-airway/runs returned no run_id", { code: ExitCode.FAILURE });
  }
  const runId = started.run_id;
  if (flags.detach) {
    process.stdout.write(flags.json ? `${JSON.stringify(started)}\n` : `${runId}\n`);
    return;
  }
  if (!flags.json) process.stderr.write(`Airway run started: ${runId}\n`);

  const lastEvent = await stream(ctx, runId, flags.json);
  const status = await finalStatus(ctx, ref, runId);
  const failed = status !== undefined ? isFailed(status) : FAILURE_EVENTS.includes(lastEvent);
  if (failed) {
    throw new CliError(`airway run ${runId} ${status ?? lastEvent} — see the events above`, {
      code: ExitCode.FAILURE
    });
  }
}

/** Stream until a terminal event; returns its name. */
async function stream(ctx: Context, runId: string, json: boolean): Promise<string> {
  const path = airwayPath(ctx, `/runs/${encodeURIComponent(runId)}/events`);
  const bearer = await ctx.bearer();
  const base = withUserAgent({
    Accept: "text/event-stream",
    ...authHeaders({ target: ctx.target(), path, method: "GET", bearer })
  });
  const url = buildUrl(ctx.target(), path);
  let lastId: string | undefined;
  let failures = 0;
  for (;;) {
    const headers = lastId ? { ...base, "Last-Event-ID": lastId } : base;
    const ending = await streamOnce(url, headers, json, (id) => {
      lastId = id;
    });
    if (ending.terminal) return ending.terminal;
    failures = ending.progressed ? 1 : failures + 1;
    if (failures > MAX_RECONNECTS) {
      throw new CliError(`the event stream for run ${runId} kept closing; gave up`, {
        code: ExitCode.UNAVAILABLE,
        detail: "the run is queued on the worker fleet and carries on without this command",
        hint: "read its status with `oxyc api '{workspace}/agentic-airway/runs?pipeline_ref=<ref>'`"
      });
    }
    await sleep(Math.min(500 * 2 ** (failures - 1), 8_000));
  }
}

async function streamOnce(
  url: string,
  headers: Record<string, string>,
  json: boolean,
  seen: (id: string) => void
): Promise<{ terminal?: string; progressed: boolean }> {
  let response: Response;
  try {
    response = await fetch(url, { method: "GET", headers });
  } catch {
    return { progressed: false };
  }
  if (response.status >= 500) return { progressed: false };
  if (!response.ok) throw await refused(response, url);
  const reader = response.body?.getReader();
  if (!reader) return { progressed: false };
  const decoder = new TextDecoder();
  const parser = new SseParser();
  let progressed = false;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) return { progressed };
      for (const frame of parser.push(decoder.decode(value, { stream: true }))) {
        if (frame.id) seen(frame.id);
        progressed = true;
        let payload: unknown = {};
        try {
          payload = JSON.parse(frame.data);
        } catch {
          payload = frame.data;
        }
        printEvent(json, Number(frame.id ?? -1), frame.event, payload);
        if (isTerminalEvent(frame.event)) return { terminal: frame.event, progressed };
      }
    }
  } catch {
    return { progressed };
  } finally {
    reader.cancel().catch(() => {});
  }
}

async function refused(response: Response, url: string): Promise<CliError> {
  const body = await response.text().catch(() => "");
  const headers: Record<string, string> = {};
  response.headers.forEach((value, name) => {
    headers[name] = value;
  });
  return errorForResponse({
    status: response.status,
    statusText: response.statusText,
    headers,
    body,
    url,
    fromCache: false
  });
}

/**
 * The run row's status once it is terminal, or `undefined` when it cannot be
 * read in time — the row can trail the stream's last event by a moment.
 */
async function finalStatus(
  ctx: Context,
  ref: string,
  runId: string,
  attempts = 20,
  pollMs = 500
): Promise<string | undefined> {
  const suffix = `/runs?pipeline_ref=${encodeURIComponent(ref)}&limit=50`;
  for (let i = 0; i < attempts; i++) {
    try {
      const runs = await airwayCall<RunSummary[]>(ctx, "GET", suffix);
      const status = runs.find((r) => r.run_id === runId)?.status;
      if (status === undefined) return undefined;
      if (isTerminal(status)) return status;
    } catch {
      return undefined;
    }
    await sleep(pollMs);
  }
  return undefined;
}
