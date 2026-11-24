/**
 * `oxyc agentic run | events | answer | cancel` — start, follow and steer an
 * agentic agent's run on a deployment.
 *
 *   POST /api/{workspace}/analytics/runs               { agent_id, question, thinking_mode }
 *   GET  /api/{workspace}/analytics/runs/{id}/events   SSE (see agentic-stream.ts)
 *   POST /api/{workspace}/analytics/runs/{id}/answer   { answer }
 *   POST /api/{workspace}/analytics/runs/{id}/cancel
 *
 * THE BEHAVIOUR CHANGE FROM `oxy agentic run`: the server runs the agent it has
 * COMPILED for the workspace's served revision, with the workspace's own
 * secrets and LLM keys — not a `.agentic.yml` in this checkout, and nothing
 * from this shell. To try an unpushed edit, push a branch and preview it.
 * Each run is a chat: the server opens a thread for it, in the caller's
 * history, as the web app's does.
 *
 * Exit codes: `run` exits 0 on `done` and on a question (the run is suspended,
 * waiting for `oxyc agentic answer`), 1 on `error` or `cancelled` — the Rust
 * command exited 0 for both. `events` reports a run and does not judge it:
 * 0 whatever the run did. A refused start is the status's code: 6 the agent
 * could not start (400), 7 the workspace is not compiled yet (503).
 */

import type { Context } from "../context/resolve.js";
import { err } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { agenticCall, runSuffix } from "./agentic-client.js";
import { followRun, isFailureEvent, type StreamEnd } from "./agentic-stream.js";

export const THINKING_MODES = ["auto", "extended_thinking"] as const;

export interface RunFlags {
  question: string;
  thinkingMode: string;
  json: boolean;
}

function validateRun(config: string, flags: RunFlags): void {
  if (!config.trim()) {
    throw usageError("name the agent: oxyc agentic run <path/to/agent.agentic.yml> -q <question>");
  }
  if (config.startsWith("/") || /^[A-Za-z]:[\\/]/.test(config)) {
    throw usageError(
      `the agent is named by its path inside the workspace, not on this machine: ${config}`,
      "e.g. `agents/sales.agentic.yml` (or `agents/sales`), as the workspace's repository has it"
    );
  }
  if (!flags.question.trim()) throw usageError("the question is empty");
  if (!(THINKING_MODES as readonly string[]).includes(flags.thinkingMode)) {
    throw usageError(
      `--thinking-mode must be one of ${THINKING_MODES.join(", ")}, got "${flags.thinkingMode}"`
    );
  }
}

export async function runAgenticRun(ctx: Context, config: string, flags: RunFlags): Promise<void> {
  validateRun(config, flags);
  const started = await agenticCall<{ run_id?: string; thread_id?: string | null }>(
    ctx,
    "POST",
    "/runs",
    { agent_id: config, question: flags.question, thinking_mode: flags.thinkingMode }
  );
  if (!started?.run_id) {
    throw new CliError("POST …/analytics/runs returned no run_id", { code: ExitCode.FAILURE });
  }
  const runId = started.run_id;
  if (!flags.json) process.stderr.write(`${err.cyan(`Run started: ${runId}`)}\n`);
  const end = await followRun(ctx, { json: flags.json, runId });
  settle(end, runId, flags.json);
}

/** The exit a finished `run` deserves. */
function settle(end: StreamEnd, runId: string, json: boolean): void {
  if (end.kind === "suspended") {
    if (json) {
      process.stderr.write(
        `the run is waiting for an answer: oxyc agentic answer ${runId} --answer "..."\n`
      );
    }
    return;
  }
  if (isFailureEvent(end.event)) {
    throw new CliError(`agentic run ${runId} ended with ${end.event} — see the events above`, {
      code: ExitCode.FAILURE
    });
  }
}

export interface EventsFlags {
  afterSeq: number;
  json: boolean;
}

export async function runAgenticEvents(
  ctx: Context,
  runId: string,
  flags: EventsFlags
): Promise<void> {
  if (!runId.trim()) throw usageError("name the run: oxyc agentic events <run_id>");
  if (!Number.isInteger(flags.afterSeq) || flags.afterSeq < -1) {
    throw usageError("--after-seq must be a whole number, -1 (the default) for every event");
  }
  const end = await followRun(ctx, { json: flags.json, runId, afterSeq: flags.afterSeq });
  if (end.events === 0 && !flags.json) {
    process.stdout.write(`No events found for run ${runId}.\n`);
  }
}

export async function runAgenticAnswer(ctx: Context, runId: string, answer: string): Promise<void> {
  if (!runId.trim()) throw usageError("name the run: oxyc agentic answer <run_id> --answer <text>");
  if (!answer.trim()) throw usageError("the answer is empty");
  await agenticCall(ctx, "POST", runSuffix(runId, "/answer"), { answer });
  process.stdout.write(`Answer submitted for run ${runId}.\n`);
  process.stderr.write(`follow it with: oxyc agentic events ${runId}\n`);
}

export async function runAgenticCancel(ctx: Context, runId: string): Promise<void> {
  if (!runId.trim()) throw usageError("name the run: oxyc agentic cancel <run_id>");
  await agenticCall(ctx, "POST", runSuffix(runId, "/cancel"));
  process.stdout.write(`Run ${runId} cancelled.\n`);
}
