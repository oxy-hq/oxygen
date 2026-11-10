/**
 * `oxyc ask` — one request that waits for the answer, driven with
 * `globalThis.fetch` stubbed (`testing/stub-fetch.ts`).
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { stubFetch } from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import { ask, runAsk } from "./ask.js";

const TARGET = "https://oxy.test";
const PROJECT = "a1a1a1a1-2222-3333-4444-555555555555";
const START = `POST /api/projects/${PROJECT}/agents/analytics/asks`;

/** `null` is a context with no credential at all. */
function fakeContext(token: string | null = "tok"): Context {
  const bearer = token ?? undefined;
  return {
    cwd: "/tmp",
    flags: { env: "production", apiKeyEnv: "OXY_API_KEY" },
    target: () => TARGET,
    env: () => ({ target: TARGET, orgSlug: undefined }) as ReturnType<Context["env"]>,
    bearer: async () => bearer ?? "",
    maybeBearer: async () => bearer,
    storedBearer: () => bearer,
    async credential() {
      return { token: bearer ?? "", source: "env" };
    },
    serviceAccount: () => undefined,
    apiKey: () => undefined,
    customer: () => undefined,
    repoDir: () => undefined,
    placeholders: () => ({}),
    withEnv: () => fakeContext(token)
  };
}

async function failureOf(p: Promise<unknown>): Promise<CliError> {
  try {
    await p;
  } catch (e) {
    if (e instanceof CliError) return e;
    throw e;
  }
  throw new Error("expected the command to fail");
}

afterEach(() => vi.unstubAllGlobals());

describe("ask", () => {
  it("sends the question, thread and wait, and returns the answer", async () => {
    const calls = stubFetch(TARGET, {
      [START]: () => ({
        status: 200,
        body: { run_id: "r-1", thread_id: "t-1", status: "done", answer: "42 orders" }
      })
    });
    const result = await ask(fakeContext(), PROJECT, "analytics", "How many orders?", {
      thread: "0f0f0f0f-1111-2222-3333-444444444444",
      waitSeconds: 30
    });
    expect(result).toEqual({
      runId: "r-1",
      threadId: "t-1",
      threadUrl: "/threads/t-1",
      status: "done",
      answer: "42 orders"
    });
    expect(calls[0]?.headers.authorization).toBe("Bearer tok");
    expect(JSON.parse(calls[0]?.body ?? "")).toEqual({
      v: 1,
      question: "How many orders?",
      thread_id: "0f0f0f0f-1111-2222-3333-444444444444",
      wait_seconds: 30
    });
  });

  it("returns a clarifying question with its suggestions", async () => {
    stubFetch(TARGET, {
      [START]: () => ({
        status: 200,
        body: {
          run_id: "r-1",
          thread_id: "t-1",
          status: "awaiting_input",
          question: "Which region?",
          suggestions: ["EU"]
        }
      })
    });
    const result = await ask(fakeContext(), PROJECT, "analytics", "Revenue?", { waitSeconds: 45 });
    expect(result).toMatchObject({
      status: "awaiting_input",
      question: "Which region?",
      suggestions: ["EU"]
    });
  });

  it("reports an ask still going at the deadline as running, not a failure", async () => {
    stubFetch(TARGET, {
      [START]: () => ({ status: 202, body: { run_id: "r-1", thread_id: "t-1", status: "running" } })
    });
    const write = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    vi.spyOn(process.stderr, "write").mockImplementation(() => true);
    await runAsk(fakeContext(), PROJECT, "analytics", "slow", { waitSeconds: 5, json: true });
    expect(JSON.parse(String(write.mock.calls[0]?.[0]))).toMatchObject({
      runId: "r-1",
      status: "running"
    });
    vi.restoreAllMocks();
  });

  it("exits 1 with the run's error when the ask failed", async () => {
    stubFetch(TARGET, {
      [START]: () => ({
        status: 200,
        body: { run_id: "r-1", thread_id: "t-1", status: "failed", error: "no warehouse" }
      })
    });
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    const e = await failureOf(
      runAsk(fakeContext(), PROJECT, "analytics", "q", { waitSeconds: 5, json: true })
    );
    vi.restoreAllMocks();
    expect(e.code).toBe(ExitCode.FAILURE);
    expect(e.message).toBe("no warehouse");
  });

  it("maps the server's 400 to the request exit code", async () => {
    stubFetch(TARGET, {
      [START]: () => ({
        status: 400,
        body: { message: "`wait_seconds` is at most 45", code: "agent_wait_too_long" }
      })
    });
    const e = await failureOf(ask(fakeContext(), PROJECT, "analytics", "q", { waitSeconds: 45 }));
    expect(e.code).toBe(ExitCode.REQUEST);
  });

  it("refuses a bad wait, project or question before any request", async () => {
    const calls = stubFetch(TARGET, {});
    for (const [project, question, waitSeconds] of [
      [PROJECT, "q", 0],
      [PROJECT, "q", 46],
      [PROJECT, "q", 2.5],
      ["not-a-uuid", "q", 10],
      [PROJECT, "   ", 10]
    ] as const) {
      const e = await failureOf(
        ask(fakeContext(), project, "analytics", question, { waitSeconds })
      );
      expect(e.code, `${project} ${question} ${waitSeconds}`).toBe(ExitCode.USAGE);
    }
    expect(calls).toHaveLength(0);
  });

  /** A malformed thread would start a new conversation server-side, losing the reply's context. */
  it("refuses a --thread that is not a thread id", async () => {
    const calls = stubFetch(TARGET, {});
    const e = await failureOf(
      ask(fakeContext(), PROJECT, "analytics", "EU", { thread: "t-1abc", waitSeconds: 5 })
    );
    expect(e.code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });

  it("needs a credential", async () => {
    stubFetch(TARGET, {});
    const e = await failureOf(
      ask(fakeContext(null), PROJECT, "analytics", "q", { waitSeconds: 5 })
    );
    expect(e.code).toBe(ExitCode.AUTH);
  });
});
