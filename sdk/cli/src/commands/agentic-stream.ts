/**
 * Following one agentic run's events: `GET /api/{workspace}/analytics/runs/{id}/events`
 * (SSE), for `oxyc agentic run` and `oxyc agentic events`.
 *
 * THE `--json` CONTRACT is the Rust `oxy agentic run --json` / `events --json`
 * one, line for line: one compact object per UI event,
 * `{"seq":<n>,"event_type":"<type>","payload":<object>}`, keys in that order,
 * several lines sharing a `seq` when one stored event became several UI
 * events. The server's SSE frame carries exactly those three things — `id` is
 * the seq, `event` the type, `data` the payload — from the same
 * `EventRegistry` stream processor the Rust command ran in-process, so the
 * line is assembled from the frame's raw `data` rather than re-serialized:
 * the payload's bytes are serde's. What the route adds, and the Rust command
 * never printed:
 *
 * - `"attempt":<n>` appended to every object payload (the run's attempt);
 * - a `recovery_resumed` event when the run was re-driven after a restart;
 * - a terminal event synthesized from the run row (`done` / `error` /
 *   `cancelled`, payload with `trace_id`) when the run ended without one.
 *
 * All three are additive. `agentic.test.ts` pins the shape against a
 * transcript derived from the Rust command's code.
 *
 * WHERE A STREAM ENDS. The route closes on `done`, `error` or `cancelled`
 * (`agentic_http::sse::is_terminal` for analytics). It does NOT close on
 * `awaiting_input`: a suspended run is still a live run, and the browser keeps
 * its stream open for the answer. The Rust command stopped there (its driver
 * returned on suspension), so this does too — once the stream has been quiet
 * for {@link SUSPEND_QUIET_MS} after the question, whatever else the run wrote
 * after it. Only `input_resolved` (the answer taken), or the coordinator
 * starting a delegation (see {@link RESUME_EVENTS}), lifts the suspension,
 * which is what lets `events` replay a run that was suspended, answered and
 * resumed: its `awaiting_input` is followed by `input_resolved` and the rest.
 */

import { authHeaders, buildUrl, errorForResponse } from "../api/request.js";
import { withUserAgent } from "../api/user-agent.js";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import { analyticsPath, runSuffix, sleep } from "./agentic-client.js";
import { renderEvent } from "./agentic-render.js";
import { type SseFrame, SseParser } from "./logs-follow.js";

/** The events that end an analytics stream — `agentic_http::sse::is_terminal`. */
export const TERMINAL_EVENTS = ["done", "error", "cancelled"] as const;

/** The terminal events that mean the run did not answer: `oxyc agentic run` exits 1. */
export const FAILURE_EVENTS = ["error", "cancelled"] as const;

/** The UI event a run emits when it stops to ask the person a question. */
export const SUSPEND_EVENT = "awaiting_input";

/**
 * The events that say a suspension is not a person's question, or no longer
 * one. The orchestrator emits `awaiting_input` for EVERY suspension,
 * delegating to an automation or another agent included (its request in
 * `questions`, so the frame alone cannot tell them apart); the coordinator
 * then writes `delegation_started` / `waiting_on_children` on the same stream
 * and the run goes on, to `input_resolved` once the child is done.
 */
export const RESUME_EVENTS = ["input_resolved", "delegation_started", "waiting_on_children"];

/** How long the stream must stay quiet after a question before the run counts as suspended. */
// Long enough for the coordinator to write `delegation_started` after a
// delegating suspension, which it does at once.
export const SUSPEND_QUIET_MS = 3_000;

/** Reconnects in a row with no event between them before giving up (exit 7). */
const MAX_RECONNECTS = 5;

export function isTerminalEvent(event: string): boolean {
  return (TERMINAL_EVENTS as readonly string[]).includes(event);
}

export function isFailureEvent(event: string): boolean {
  return (FAILURE_EVENTS as readonly string[]).includes(event);
}

/** How following a run ended. */
export type StreamEnd =
  | { kind: "terminal"; event: string; events: number }
  | { kind: "suspended"; events: number };

export interface StreamOptions {
  json: boolean;
  /** Replay only events after this seq (`Last-Event-ID`). */
  afterSeq?: number;
  /** Shown in the pretty `awaiting_input` hint. */
  runId: string;
  quietMs?: number;
}

/**
 * One `--json` line, assembled from the frame. `data` is the server's serde
 * output and is kept byte for byte; it is quoted only if it is not JSON.
 */
export function jsonLine(frame: SseFrame): string {
  const seq = Number(frame.id ?? -1);
  const payload = isJson(frame.data) ? frame.data : JSON.stringify(frame.data);
  return `{"seq":${seq},"event_type":${JSON.stringify(frame.event)},"payload":${payload}}`;
}

function isJson(text: string): boolean {
  try {
    JSON.parse(text);
    return true;
  } catch {
    return false;
  }
}

function print(frame: SseFrame, opts: StreamOptions): void {
  if (opts.json) {
    process.stdout.write(`${jsonLine(frame)}\n`);
    return;
  }
  let payload: unknown = {};
  try {
    payload = JSON.parse(frame.data);
  } catch {
    payload = {};
  }
  const data = payload && typeof payload === "object" ? (payload as Record<string, unknown>) : {};
  process.stdout.write(renderEvent(frame.event, data, opts.runId));
}

/** Follow the run's events until a terminal event, or a question nobody has answered yet. */
export async function followRun(ctx: Context, opts: StreamOptions): Promise<StreamEnd> {
  const path = analyticsPath(ctx, runSuffix(opts.runId, "/events"));
  const base = withUserAgent({
    Accept: "text/event-stream",
    ...authHeaders({ target: ctx.target(), path, method: "GET", bearer: await ctx.bearer() })
  });
  const url = buildUrl(ctx.target(), path);
  const state: FollowState = {
    lastId: opts.afterSeq !== undefined && opts.afterSeq >= 0 ? String(opts.afterSeq) : undefined,
    events: 0,
    suspended: false
  };
  let failures = 0;
  for (;;) {
    const headers = state.lastId ? { ...base, "Last-Event-ID": state.lastId } : base;
    const before = state.events;
    const end = await followOnce(url, headers, opts, state);
    if (end) return end;
    failures = state.events > before ? 1 : failures + 1;
    if (failures > MAX_RECONNECTS) throw gaveUp(opts.runId);
    await sleep(Math.min(500 * 2 ** (failures - 1), 8_000));
  }
}

interface FollowState {
  lastId?: string;
  events: number;
  /** The run asked a question and has not taken an answer since. */
  suspended: boolean;
}

function gaveUp(runId: string): CliError {
  return new CliError(`the event stream for run ${runId} kept closing; gave up`, {
    code: ExitCode.UNAVAILABLE,
    detail: "the run carries on on the deployment without this command",
    hint: `pick it up again with \`oxyc agentic events ${runId}\``
  });
}

/** One connection. `undefined` when it dropped and should be reopened. */
async function followOnce(
  url: string,
  headers: Record<string, string>,
  opts: StreamOptions,
  state: FollowState
): Promise<StreamEnd | undefined> {
  let response: Response;
  try {
    response = await fetch(url, { method: "GET", headers });
  } catch {
    return undefined;
  }
  if (response.status >= 500) return undefined;
  if (!response.ok) throw await refused(response, url);
  const reader = response.body?.getReader();
  if (!reader) return undefined;
  try {
    return await readFrames(reader, opts, state);
  } catch {
    return undefined;
  } finally {
    reader.cancel().catch(() => {});
  }
}

const QUIET = Symbol("quiet");

async function readFrames(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  opts: StreamOptions,
  state: FollowState
): Promise<StreamEnd | undefined> {
  const decoder = new TextDecoder();
  const parser = new SseParser();
  let pending = reader.read();
  for (;;) {
    const next = state.suspended ? await quietOr(pending, opts.quietMs) : await pending;
    if (next === QUIET) return { kind: "suspended", events: state.events };
    if (next.done) return state.suspended ? { kind: "suspended", events: state.events } : undefined;
    pending = reader.read();
    for (const frame of parser.push(decoder.decode(next.value, { stream: true }))) {
      const end = handleFrame(frame, opts, state);
      if (end) return end;
    }
  }
}

function handleFrame(
  frame: SseFrame,
  opts: StreamOptions,
  state: FollowState
): StreamEnd | undefined {
  if (frame.id) state.lastId = frame.id;
  state.events += 1;
  print(frame, opts);
  // Sticky: whatever the run writes after its question (a step closing, its
  // usage) keeps it suspended; only the answer being taken resumes it.
  if (frame.event === SUSPEND_EVENT) state.suspended = true;
  else if (RESUME_EVENTS.includes(frame.event)) state.suspended = false;
  if (isTerminalEvent(frame.event)) {
    return { kind: "terminal", event: frame.event, events: state.events };
  }
  return undefined;
}

function quietOr<T>(pending: Promise<T>, quietMs = SUSPEND_QUIET_MS): Promise<T | typeof QUIET> {
  let timer: NodeJS.Timeout | undefined;
  const quiet = new Promise<typeof QUIET>((resolve) => {
    timer = setTimeout(() => resolve(QUIET), quietMs);
  });
  return Promise.race([pending, quiet]).finally(() => clearTimeout(timer));
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
