/**
 * `oxyc logs <app> --follow` — the live tail of one environment's function
 * invocations: `GET /api/customer-apps/{id}/environments/{env}/tail`, an SSE
 * stream (`crates/app/src/server/api/custom_apps_tail/`).
 *
 * Unlike `oxyc logs`, it needs no observability capture: lines go from the
 * process running the function to the stream directly, as they are printed.
 *
 * The stream is lossy by design, and says so. A `dropped` event marks lines
 * the server could not deliver; a dropped CONNECTION is reopened with
 * `Last-Event-ID`, and the server replays the outcomes (`finish`, marked
 * `replayed`) of invocations that ended meanwhile — never their lines. A
 * `done` event ends the command with exit 0.
 */

import { authHeaders, buildUrl, errorForResponse } from "../api/request.js";
import { withUserAgent } from "../api/user-agent.js";
import { parseAppEnv } from "../apps/environment.js";
import { resolveApp, staffCreds } from "../apps/resolve.js";
import { requireOwnSandbox, sandboxTokenError } from "../apps/sandbox-token.js";
import { isSandboxAgentToken } from "../auth/token-kind.js";
import type { Context } from "../context/resolve.js";
import { err } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { printable } from "../util/printable.js";

/** The server's longest `?timeout=`. */
export const MAX_FOLLOW_SECONDS = 86_400;
/** Reconnects in a row, with no `open` between them, before giving up (exit 7). */
const MAX_RECONNECTS = 5;

export interface FollowOptions {
  appEnv?: string;
  /** End after this many seconds, across reconnects. Absent: until `done` or Ctrl-C. */
  timeoutSeconds?: number;
  json: boolean;
}

/** One SSE event as the server frames it. */
export interface SseFrame {
  event: string;
  id?: string;
  data: string;
}

/**
 * Incremental SSE parsing: feed chunks as they arrive, take complete events.
 * Comment lines (the keep-alives) and unknown fields are skipped.
 */
export class SseParser {
  private buffer = "";

  push(chunk: string): SseFrame[] {
    this.buffer += chunk.replace(/\r\n/g, "\n");
    const frames: SseFrame[] = [];
    let end = this.buffer.indexOf("\n\n");
    while (end !== -1) {
      const block = this.buffer.slice(0, end);
      this.buffer = this.buffer.slice(end + 2);
      const frame = parseBlock(block);
      if (frame) frames.push(frame);
      end = this.buffer.indexOf("\n\n");
    }
    return frames;
  }
}

function parseBlock(block: string): SseFrame | undefined {
  let event = "message";
  let id: string | undefined;
  const data: string[] = [];
  for (const line of block.split("\n")) {
    if (line === "" || line.startsWith(":")) continue;
    const colon = line.indexOf(":");
    const field = colon === -1 ? line : line.slice(0, colon);
    const value = colon === -1 ? "" : line.slice(colon + 1).replace(/^ /, "");
    if (field === "event") event = value;
    else if (field === "id") id = value;
    else if (field === "data") data.push(value);
  }
  if (data.length === 0) return undefined;
  return { event, id, data: data.join("\n") };
}

/**
 * What one connection ended with: the server's `done`; a `reopen`, when the
 * server ended this request for the client to authenticate again; or a
 * drop — the connection broke, or the server answered 5xx — with the error
 * to report if the reconnects run out.
 */
type Ending =
  | { kind: "done"; reason: string }
  | { kind: "reopen" }
  | { kind: "dropped"; opened: boolean; error?: CliError };

/** The `done` reason that asks for a new request, not an end. */
const REAUTHENTICATE = "reauthenticate";

/** Where events go: one JSON line each, or a human line. */
export interface Printer {
  frame(name: string, data: Record<string, unknown>): void;
}

export function printerFor(json: boolean): Printer {
  return json ? { frame: printJson } : { frame: printHuman };
}

function printJson(name: string, data: Record<string, unknown>): void {
  // The server's own events carry `event`; `open`, `dropped` and `done` do not.
  process.stdout.write(`${JSON.stringify({ event: name, ...data })}\n`);
}

/** A field the server sent, as one printable line: a string or a number, else empty. */
function text(value: unknown): string {
  if (typeof value === "string") return printable(value);
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return "";
}

function printHuman(name: string, data: Record<string, unknown>): void {
  const fn = text(data.function);
  const inv = text(data.invocation_id);
  switch (name) {
    case "log":
      process.stdout.write(
        `${new Date().toISOString()} ${text(data.level).padEnd(5)} ${fn} ${text(data.message)}\n`
      );
      return;
    case "start":
      // `mode` is `call` when another app's call started it, and then names its parent.
      process.stdout.write(
        `>> ${fn} started (${text(data.mode)}) ${inv}${data.parent_invocation_id ? ` parent ${text(data.parent_invocation_id)}` : ""}\n`
      );
      return;
    case "finish": {
      const how = `${text(data.status)} ${text(data.http_status)} in ${text(data.duration_ms)}ms`;
      const error = data.error ? `: ${text(data.error)}` : "";
      const replayed = data.replayed ? " (replayed)" : "";
      const line = `<< ${fn} ${how}${error}${replayed} ${inv}`;
      process.stdout.write(`${data.failed ? err.red(line) : line}\n`);
      return;
    }
    case "open":
      process.stderr.write(
        `${err.dim(`-- following ${text(data.environment)}${data.resumed ? " (resumed)" : ""}`)}\n`
      );
      return;
    case "dropped": {
      const count = data.events === undefined ? "" : ` (${text(data.events)} events)`;
      process.stderr.write(`${err.yellow(`-- dropped: ${text(data.reason)}${count}`)}\n`);
      return;
    }
    case "done":
      process.stderr.write(`${err.dim(`-- done: ${text(data.reason)}`)}\n`);
      return;
  }
}

/** One connection's request. */
interface Connection {
  url: string;
  headers: Record<string, string>;
  bearer?: string;
}

/**
 * Open the stream once and print what it sends. Throws for a refusal (4xx);
 * returns how the stream ended otherwise, a 5xx included — `tail_unavailable`
 * says "retry shortly". `seen` holds the invocations whose `finish` was
 * printed, so a replayed one is not printed twice.
 */
async function streamOnce(
  conn: Connection,
  lastEventId: { value?: string },
  seen: Set<string>,
  printer: Printer
): Promise<Ending> {
  const headers = { ...conn.headers };
  if (lastEventId.value) headers["Last-Event-ID"] = lastEventId.value;
  let response: Response;
  try {
    response = await fetch(conn.url, { method: "GET", headers });
  } catch {
    return { kind: "dropped", opened: false };
  }
  if (response.status >= 500) {
    return { kind: "dropped", opened: false, error: await refused(response, conn) };
  }
  if (!response.ok) throw await refused(response, conn);
  const reader = response.body?.getReader();
  if (!reader) return { kind: "dropped", opened: false };
  const decoder = new TextDecoder();
  const parser = new SseParser();
  let opened = false;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) return { kind: "dropped", opened };
      for (const frame of parser.push(decoder.decode(value, { stream: true }))) {
        if (frame.id) lastEventId.value = frame.id;
        const data = parseData(frame.data);
        if (frame.event === "open") opened = true;
        if (frame.event === "finish" && !firstFinish(data, seen)) continue;
        // Not the end of the follow: the next request carries on from here.
        if (frame.event === "done" && data.reason === REAUTHENTICATE) return { kind: "reopen" };
        printer.frame(frame.event, data);
        if (frame.event === "done") return { kind: "done", reason: text(data.reason) };
      }
    }
  } catch {
    return { kind: "dropped", opened };
  } finally {
    reader.cancel().catch(() => {});
  }
}

function parseData(raw: string): Record<string, unknown> {
  try {
    const parsed = JSON.parse(raw) as unknown;
    return parsed && typeof parsed === "object" ? (parsed as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

/** Whether this is the first `finish` seen for its invocation. */
function firstFinish(data: Record<string, unknown>, seen: Set<string>): boolean {
  const id = typeof data.invocation_id === "string" ? data.invocation_id : undefined;
  if (id === undefined) return true;
  if (seen.has(id)) return false;
  seen.add(id);
  return true;
}

async function refused(response: Response, conn: Connection): Promise<CliError> {
  const body = await response.text().catch(() => "");
  const headers: Record<string, string> = {};
  response.headers.forEach((value, name) => {
    headers[name] = value;
  });
  const api = {
    status: response.status,
    statusText: response.statusText,
    headers,
    body,
    url: conn.url,
    fromCache: false
  };
  return isSandboxAgentToken(conn.bearer) ? sandboxTokenError(api) : errorForResponse(api);
}

/**
 * A `done` is a clean end (exit 0) — except `access_revoked`: the server asked
 * the caller again mid-stream and it may no longer read this environment,
 * which is the auth failure its next request would have been (exit 4).
 */
function endedBy(reason: string, url: string): void {
  if (reason !== "access_revoked") return;
  throw new CliError(`the tail at ${url} ended: this credential may no longer read it`, {
    code: ExitCode.AUTH,
    hint: "the token was revoked or expired, a grant was removed, or the sandbox was deleted"
  });
}

/**
 * The error once reconnects run out: the server's last 5xx when there was
 * one (its exit code is 7 too), else the connection that kept breaking.
 */
function gaveUp(url: string, last: CliError | undefined): CliError {
  return (
    last ??
    new CliError(`the tail at ${url} kept dropping; gave up after ${MAX_RECONNECTS} reconnects`, {
      code: ExitCode.UNAVAILABLE,
      hint: "the deployment may be restarting — run the command again"
    })
  );
}

/** The URL for a connection, with the time left as the server's `?timeout=`. */
function tailUrl(base: string, deadline: number | undefined): string | undefined {
  if (deadline === undefined) return base;
  const left = Math.ceil((deadline - Date.now()) / 1000);
  return left > 0 ? `${base}?timeout=${Math.min(left, MAX_FOLLOW_SECONDS)}` : undefined;
}

/** Follow until `done`, the deadline, or reconnects run out. */
export async function follow(
  conn: Connection,
  deadline: number | undefined,
  printer: Printer,
  sleep: (ms: number) => Promise<void> = (ms) => new Promise((r) => setTimeout(r, ms))
): Promise<void> {
  const lastEventId: { value?: string } = {};
  const seen = new Set<string>();
  let failures = 0;
  for (;;) {
    const url = tailUrl(conn.url, deadline);
    if (url === undefined) {
      printer.frame("done", { reason: "timeout" });
      return;
    }
    const ending = await streamOnce({ ...conn, url }, lastEventId, seen, printer);
    if (ending.kind === "done") return endedBy(ending.reason, conn.url);
    if (ending.kind === "reopen") {
      // A new request at once: lines printed while it opens may be missed.
      failures = 0;
      printer.frame("dropped", { reason: REAUTHENTICATE });
      continue;
    }
    failures = ending.opened ? 1 : failures + 1;
    if (failures > MAX_RECONNECTS) throw gaveUp(conn.url, ending.error);
    printer.frame("dropped", { reason: "disconnected" });
    await sleep(Math.min(500 * 2 ** (failures - 1), 8_000));
  }
}

/** The polled read's narrowing flags, which a live tail does not take. */
const POLLED_ONLY = ["invocation", "request", "hours", "limit"] as const;

/**
 * `--timeout` for `--follow`, in seconds. Refuses (exit 2) the polled read's
 * flags beside `--follow`, and a timeout the server would refuse.
 */
export function followTimeout(opts: Record<string, unknown>): number | undefined {
  for (const flag of POLLED_ONLY) {
    if (opts[flag] !== undefined) {
      throw usageError(
        `--${flag} does not apply to --follow`,
        "--follow streams every invocation of the environment as it runs; --invocation, --request, --hours and --limit narrow the polled read"
      );
    }
  }
  if (opts.timeout === undefined) return undefined;
  const seconds = Number(opts.timeout);
  if (!Number.isInteger(seconds) || seconds < 1 || seconds > MAX_FOLLOW_SECONDS) {
    throw usageError(
      `--timeout must be a whole number of seconds between 1 and ${MAX_FOLLOW_SECONDS}`
    );
  }
  return seconds;
}

export async function runLogsFollow(ctx: Context, app: string, opts: FollowOptions): Promise<void> {
  const appEnv = opts.appEnv !== undefined ? parseAppEnv(opts.appEnv) : undefined;
  const creds = staffCreds(ctx);
  // Before any request: with no environment this tails production.
  if (isSandboxAgentToken(creds.bearer)) requireOwnSandbox("oxyc logs --follow", appEnv);
  const resolved = await resolveApp(creds, app);
  const environment = appEnv ?? "production";
  const path = `/api/customer-apps/${resolved.appId}/environments/${environment}/tail`;
  const headers = withUserAgent({
    Accept: "text/event-stream",
    ...authHeaders({ target: creds.target, path, method: "GET", bearer: creds.bearer }),
    ...(creds.headers ?? {})
  });
  const deadline =
    opts.timeoutSeconds === undefined ? undefined : Date.now() + opts.timeoutSeconds * 1000;
  await follow(
    { url: buildUrl(creds.target, path), headers, bearer: creds.bearer },
    deadline,
    printerFor(opts.json)
  );
}
