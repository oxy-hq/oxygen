/**
 * `oxyc ask` — ask a workspace's agent a question and wait for the answer, in
 * one request.
 *
 * `POST /api/projects/{project}/agents/{agent}/asks` with `wait_seconds`: the
 * same route a custom app's `useAskAgent` calls, as the person logged in. The
 * server holds the response until the ask settles (up to 45 s) and answers
 * with how it ended; an ask still going at the deadline comes back `running`
 * and keeps running on the fleet, so it is reported, not treated as a failure.
 *
 * Exit codes: `done` and `awaiting_input` (the agent asked a clarifying
 * question, printed with its suggestions) are 0; `running` is 0 too, with the
 * run and thread named on stderr. No code in the contract fits either, and
 * `--json` carries `status` for a caller that branches. `failed`, `timed_out`
 * and `cancelled` are 1, as `oxyc fn run`'s failed run is.
 */

import { parseJson, request } from "../api/request.js";
import { ensureOk, UUID_RE } from "../apps/resolve.js";
import { type Context, notAuthenticated } from "../context/resolve.js";
import { err } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";

/** The longest wait the server accepts. */
export const MAX_WAIT_SECONDS = 45;

/** The `--json` report: one object on stdout. */
export interface AskResult {
  runId: string;
  threadId: string;
  threadUrl: string;
  /** `done` | `failed` | `timed_out` | `cancelled` | `awaiting_input` | `running`. */
  status: string;
  answer?: string;
  error?: string;
  question?: string;
  suggestions?: string[];
}

export interface AskOptions {
  thread?: string;
  waitSeconds: number;
}

interface AskWire {
  run_id?: string;
  thread_id?: string;
  thread_url?: string;
  status?: string;
  answer?: string;
  error?: string;
  question?: string;
  suggestions?: string[];
}

const FAILED = new Set(["failed", "timed_out", "cancelled"]);

function validate(project: string, question: string, opts: AskOptions): void {
  if (!UUID_RE.test(project)) {
    throw usageError(`project must be a workspace id (a UUID), got "${project}"`);
  }
  if (!question.trim()) throw usageError("the question is empty");
  // The server reads a malformed thread id as none and starts a new
  // conversation, so a mistyped reply would lose its context silently.
  if (opts.thread !== undefined && !UUID_RE.test(opts.thread)) {
    throw usageError(`--thread must be a thread id (a UUID), got "${opts.thread}"`);
  }
  const wait = opts.waitSeconds;
  if (!Number.isInteger(wait) || wait < 1 || wait > MAX_WAIT_SECONDS) {
    throw usageError(
      `--wait must be a whole number of seconds from 1 to ${MAX_WAIT_SECONDS}`,
      "a longer ask keeps running after the wait; its result is in the thread"
    );
  }
}

/** Start the ask and wait for it. No printing: `runAsk` and callers decide. */
export async function ask(
  ctx: Context,
  project: string,
  agent: string,
  question: string,
  opts: AskOptions
): Promise<AskResult> {
  validate(project, question, opts);
  const target = ctx.target();
  const apiKey = ctx.apiKey();
  const bearer = apiKey ? ctx.storedBearer() : await ctx.maybeBearer();
  if (!bearer && !apiKey) throw notAuthenticated(target, ctx.flags);
  const creds = { bearer };
  const response = await request({
    target,
    path: `/api/projects/${project}/agents/${encodeURIComponent(agent)}/asks`,
    method: "POST",
    body: JSON.stringify({
      v: 1,
      question,
      ...(opts.thread ? { thread_id: opts.thread } : {}),
      wait_seconds: opts.waitSeconds
    }),
    bearer,
    apiKey,
    // The server answers within the wait; the margin covers starting the ask.
    timeoutMs: (opts.waitSeconds + 30) * 1000
  });
  ensureOk(response, creds);
  const wire = parseJson(response.body) as AskWire | undefined;
  if (!wire?.run_id || !wire.thread_id) {
    throw new CliError("the ask was accepted but its response named no run", {
      code: ExitCode.UNAVAILABLE
    });
  }
  return {
    runId: wire.run_id,
    threadId: wire.thread_id,
    threadUrl: wire.thread_url ?? `/threads/${wire.thread_id}`,
    // A server that predates `wait_seconds` answers a bare 202.
    status: wire.status ?? "running",
    ...(wire.answer !== undefined ? { answer: wire.answer } : {}),
    ...(wire.error !== undefined ? { error: wire.error } : {}),
    ...(wire.question !== undefined ? { question: wire.question } : {}),
    ...(wire.suggestions !== undefined ? { suggestions: wire.suggestions } : {})
  };
}

export async function runAsk(
  ctx: Context,
  project: string,
  agent: string,
  question: string,
  opts: AskOptions & { json: boolean }
): Promise<void> {
  const result = await ask(ctx, project, agent, question, opts);
  if (opts.json) {
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } else {
    printResult(ctx.target(), result);
  }
  if (FAILED.has(result.status)) {
    throw new CliError(result.error ?? `the ask ended ${result.status}`, {
      code: ExitCode.FAILURE
    });
  }
}

/** The answer (or question) on stdout, so it pipes; everything else on stderr. */
function printResult(target: string, result: AskResult): void {
  const quiet = Boolean(process.env.OXYC_QUIET);
  const thread = `${target.replace(/\/$/, "")}${result.threadUrl}`;
  switch (result.status) {
    case "done":
      process.stdout.write(`${result.answer ?? ""}\n`);
      return;
    case "awaiting_input":
      process.stdout.write(`${result.question ?? ""}\n`);
      for (const s of result.suggestions ?? []) process.stdout.write(`  - ${s}\n`);
      if (!quiet) {
        process.stderr.write(
          `${err.yellow("?")} the agent asked a question; answer with: oxyc ask … --thread ${result.threadId} "<answer>"\n`
        );
      }
      return;
    case "running":
      if (!quiet) {
        process.stderr.write(
          `${err.yellow("…")} still running after the wait (run ${result.runId}); it keeps going, see ${thread}\n`
        );
      }
      return;
    default:
      // failed / timed_out / cancelled: `runAsk` throws with the error.
      return;
  }
}
