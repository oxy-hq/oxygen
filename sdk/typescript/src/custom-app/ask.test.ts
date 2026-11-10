import { describe, expect, it, vi } from "vitest";
import { askAgent } from "./ask";
import { OxyApiError } from "./errors";

const START = "/api/projects/p-1/agents/analytics/asks";

function fetcherAnswering(status: number, body: unknown) {
  return vi.fn(
    async (_input: RequestInfo | URL, _init?: RequestInit) =>
      new Response(JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json" }
      })
  );
}

function sentBody(fetcher: ReturnType<typeof fetcherAnswering>) {
  const [, init] = fetcher.mock.calls[0];
  return JSON.parse(init?.body as string);
}

describe("askAgent", () => {
  it("returns the answer of an ask that finished within the wait", async () => {
    const fetcher = fetcherAnswering(200, {
      run_id: "r-1",
      thread_id: "t-1",
      thread_url: "/acme/workspaces/w/threads/t-1",
      status: "done",
      answer: "42 orders"
    });
    const result = await askAgent(fetcher, "p-1", "analytics", "How many orders?", {
      threadId: "t-0",
      waitSeconds: 20
    });
    expect(result).toEqual({
      runId: "r-1",
      threadId: "t-1",
      threadUrl: "/acme/workspaces/w/threads/t-1",
      status: "done",
      answer: "42 orders"
    });
    expect(fetcher.mock.calls[0][0]).toBe(START);
    expect(sentBody(fetcher)).toEqual({
      v: 1,
      question: "How many orders?",
      thread_id: "t-0",
      wait_seconds: 20
    });
  });

  it("waits the longest the server allows when no wait is given", async () => {
    const fetcher = fetcherAnswering(200, {
      run_id: "r",
      thread_id: "t",
      status: "done",
      answer: ""
    });
    await askAgent(fetcher, "p-1", "analytics", "q");
    expect(sentBody(fetcher).wait_seconds).toBe(45);
  });

  it("returns the clarifying question of a parked ask", async () => {
    const fetcher = fetcherAnswering(200, {
      run_id: "r-1",
      thread_id: "t-1",
      status: "awaiting_input",
      question: "Which region?",
      suggestions: ["EU", "US"]
    });
    const result = await askAgent(fetcher, "p-1", "analytics", "Revenue?");
    expect(result.status).toBe("awaiting_input");
    expect(result.question).toBe("Which region?");
    expect(result.suggestions).toEqual(["EU", "US"]);
    expect(result.threadUrl).toBe("/threads/t-1");
  });

  it("reports an ask still going at the deadline as running", async () => {
    const fetcher = fetcherAnswering(202, { run_id: "r-1", thread_id: "t-1", status: "running" });
    const result = await askAgent(fetcher, "p-1", "analytics", "Slow one", { waitSeconds: 1 });
    expect(result).toMatchObject({ runId: "r-1", status: "running" });
    expect(result.answer).toBeUndefined();
  });

  it("reads a bare 202 from a server that predates the wait as running", async () => {
    const fetcher = fetcherAnswering(202, { run_id: "r-1", thread_id: "t-1" });
    expect((await askAgent(fetcher, "p-1", "analytics", "q")).status).toBe("running");
  });

  it("throws the server's refusal as an OxyApiError with its code", async () => {
    const fetcher = fetcherAnswering(400, {
      message: "`wait_seconds` is at most 45",
      code: "agent_wait_too_long"
    });
    const failure = askAgent(fetcher, "p-1", "analytics", "q", { waitSeconds: 45 });
    await expect(failure).rejects.toBeInstanceOf(OxyApiError);
    await expect(failure).rejects.toMatchObject({ status: 400, code: "agent_wait_too_long" });
  });

  it("refuses a wait outside 1..45 before any request", async () => {
    const fetcher = fetcherAnswering(200, {});
    for (const waitSeconds of [0, 46, 2.5, -1]) {
      await expect(askAgent(fetcher, "p-1", "analytics", "q", { waitSeconds })).rejects.toThrow(
        RangeError
      );
    }
    expect(fetcher).not.toHaveBeenCalled();
  });
});
