// One-call agent asks: start an ask and wait for its answer in the same
// request, for callers that want the result rather than a stream.
//
// `POST /api/projects/{project}/agents/{agent}/asks` with `wait_seconds`
// holds the response until the ask settles (up to 45 s): `200` with how it
// ended, or `202` with `status: "running"` when the wait runs out first. The
// run itself is queued on the fleet either way, so a `running` result is not
// a failure: follow it with `useAgentRun`'s stream, or ask again later on the
// same thread.
//
// `useAgentRun` stays the right tool for a chat UI (token-by-token events,
// artifacts). This is for a button that fills in a sentence, a batch over
// many rows, or anything else that wants `await ask(...)` and a string back.

import * as React from "react";
import { apiErrorFromResponse } from "./errors";
import { type AppFetcher, useOxyApp } from "./react";

/** The longest wait the server accepts. */
export const MAX_ASK_WAIT_SECONDS = 45;

/**
 * How far the ask got. The run's own status names: `running` means the wait
 * ran out first, and `awaiting_input` means the agent asked a clarifying
 * question — answer it with a new ask on the same `threadId`.
 */
export type AskStatus =
  | "done"
  | "failed"
  | "timed_out"
  | "cancelled"
  | "awaiting_input"
  | "running";

export interface AskResult {
  runId: string;
  threadId: string;
  /** Relative path to the conversation in Oxygen. */
  threadUrl: string;
  status: AskStatus;
  /** Set when `status` is `done`. */
  answer?: string;
  /** Set when `status` is `failed` or `timed_out`. */
  error?: string;
  /** The clarifying question, when `status` is `awaiting_input`. */
  question?: string;
  suggestions?: string[];
}

export interface AskOptions {
  /** Continue this conversation (a `threadId` from an earlier result). */
  threadId?: string;
  /** Seconds to wait for the answer, 1 to 45. Defaults to 45. */
  waitSeconds?: number;
  signal?: AbortSignal;
}

interface AskWire {
  run_id: string;
  thread_id: string;
  thread_url?: string;
  status?: AskStatus;
  answer?: string;
  error?: string;
  question?: string;
  suggestions?: string[];
}

function waitSecondsOf(opts: AskOptions): number {
  const wait = opts.waitSeconds ?? MAX_ASK_WAIT_SECONDS;
  if (!Number.isInteger(wait) || wait < 1 || wait > MAX_ASK_WAIT_SECONDS) {
    throw new RangeError(
      `waitSeconds must be a whole number from 1 to ${MAX_ASK_WAIT_SECONDS} (got ${wait}); ` +
        "read a longer ask from useAgentRun's stream"
    );
  }
  return wait;
}

/**
 * Ask `agentId` a question and wait for the answer. Throws `OxyApiError` for
 * a start the server refused (no such agent, not allowed), and `RangeError`
 * for a `waitSeconds` outside 1..45, before any request.
 */
export async function askAgent(
  fetcher: AppFetcher,
  projectId: string,
  agentId: string,
  question: string,
  opts: AskOptions = {}
): Promise<AskResult> {
  const waitSeconds = waitSecondsOf(opts);
  const resp = await fetcher(
    `/api/projects/${encodeURIComponent(projectId)}/agents/${encodeURIComponent(agentId)}/asks`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        v: 1,
        question,
        ...(opts.threadId ? { thread_id: opts.threadId } : {}),
        wait_seconds: waitSeconds
      }),
      signal: opts.signal
    }
  );
  if (!resp.ok) throw await apiErrorFromResponse(resp);
  const wire = (await resp.json()) as AskWire;
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

/**
 * `askAgent` bound to the app's project and credentialed fetcher.
 *
 * ```ts
 * const ask = useAskAgent();
 * const { status, answer } = await ask("analytics", "Revenue last week?");
 * ```
 */
export function useAskAgent(): (
  agentId: string,
  question: string,
  opts?: AskOptions
) => Promise<AskResult> {
  const { projectId, fetcher } = useOxyApp();
  return React.useCallback(
    (agentId: string, question: string, opts: AskOptions = {}) => {
      if (!projectId) return Promise.reject(new Error("project not configured"));
      return askAgent(fetcher, projectId, agentId, question, opts);
    },
    [projectId, fetcher]
  );
}
