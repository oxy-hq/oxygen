/**
 * `oxyc logs --follow` — the SSE parser, the reconnect loop and the flag
 * rules, driven with `globalThis.fetch` stubbed by streamed bodies.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { CliError, ExitCode } from "../util/errors.js";
import { follow, followTimeout, type Printer, printerFor, SseParser } from "./logs-follow.js";

const URL_BASE = "https://oxy.test/api/customer-apps/a1/environments/dev-a1/tail";

/** A response whose body arrives as `chunks`, then ends. */
function streamed(chunks: string[], status = 200): Response {
  const encoder = new TextEncoder();
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
      controller.close();
    }
  });
  return new Response(body, { status, headers: { "content-type": "text/event-stream" } });
}

function sse(event: string, id: number, data: unknown): string {
  return `event: ${event}\nid: ${id}\ndata: ${JSON.stringify(data)}\n\n`;
}

function recorder(): Printer & { seen: [string, Record<string, unknown>][] } {
  const seen: [string, Record<string, unknown>][] = [];
  return { seen, frame: (name, data) => seen.push([name, data]) };
}

const noSleep = async () => {};

/** A string field, or empty. */
const label = (value: unknown): string => (typeof value === "string" ? value : "");

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the human start line", () => {
  it("names the parent invocation only when another app's call started it", () => {
    let out = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      out += String(chunk);
      return true;
    });
    const frame = printerFor(false).frame;
    frame("start", {
      function: "f",
      mode: "call",
      invocation_id: "i2",
      parent_invocation_id: "i1"
    });
    frame("start", {
      function: "f",
      mode: "route",
      invocation_id: "i3",
      parent_invocation_id: null
    });
    write.mockRestore();
    expect(out).toBe(">> f started (call) i2 parent i1\n>> f started (route) i3\n");
  });
});

describe("SseParser", () => {
  it("joins events split across chunks and skips keep-alive comments", () => {
    const parser = new SseParser();
    expect(parser.push("event: log\nid: 7\nda")).toEqual([]);
    expect(parser.push('ta: {"seq":1}\n\n: keep-alive\n\nevent: done\r\ndata: {}\r\n\r\n')).toEqual(
      [
        { event: "log", id: "7", data: '{"seq":1}' },
        { event: "done", id: undefined, data: "{}" }
      ]
    );
  });
});

describe("follow", () => {
  it("prints events in order and ends on done", async () => {
    const fetch = vi.fn(async () =>
      streamed([
        sse("open", 1, { environment: "dev-a1", resumed: false }),
        sse("start", 2, { event: "start", invocation_id: "i1", function: "probe" }),
        sse("log", 3, { event: "log", invocation_id: "i1", seq: 1, message: "hi" }),
        sse("finish", 4, { event: "finish", invocation_id: "i1", status: "success" }),
        sse("done", 5, { reason: "timeout" })
      ])
    );
    vi.stubGlobal("fetch", fetch);
    const printer = recorder();
    await follow({ url: URL_BASE, headers: {} }, undefined, printer, noSleep);
    expect(printer.seen.map(([name]) => name)).toEqual(["open", "start", "log", "finish", "done"]);
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("reconnects with Last-Event-ID, says so, and prints a replayed finish once", async () => {
    const headers: Record<string, string>[] = [];
    const bodies = [
      [
        sse("open", 10, { environment: "dev-a1" }),
        sse("finish", 11, { event: "finish", invocation_id: "i1", status: "success" })
      ],
      [
        sse("open", 12, { environment: "dev-a1", resumed: true }),
        sse("finish", 13, { event: "finish", invocation_id: "i1", replayed: true }),
        sse("finish", 14, { event: "finish", invocation_id: "i2", replayed: true }),
        sse("done", 15, { reason: "shutdown" })
      ]
    ];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (_url: string, init?: RequestInit) => {
        headers.push({ ...(init?.headers as Record<string, string>) });
        return streamed(bodies.shift() ?? []);
      })
    );
    const printer = recorder();
    await follow({ url: URL_BASE, headers: {} }, undefined, printer, noSleep);
    expect(headers[0]?.["Last-Event-ID"]).toBeUndefined();
    expect(headers[1]?.["Last-Event-ID"]).toBe("11");
    expect(
      printer.seen.map(
        ([name, data]) => `${name}:${label(data.invocation_id) || label(data.reason)}`
      )
    ).toEqual([
      "open:",
      "finish:i1",
      "dropped:disconnected",
      "open:",
      "finish:i2",
      "done:shutdown"
    ]);
  });

  it("sends the time left as the server's timeout", async () => {
    const urls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string) => {
        urls.push(url);
        return streamed([sse("done", 1, { reason: "timeout" })]);
      })
    );
    await follow({ url: URL_BASE, headers: {} }, Date.now() + 30_000, recorder(), noSleep);
    expect(urls[0]).toMatch(/\?timeout=(29|30)$/);
  });

  it("retries a 5xx answer within the reconnect budget", async () => {
    const answers = [
      () =>
        new Response(JSON.stringify({ error: "tail_unavailable", message: "retry shortly" }), {
          status: 503
        }),
      () => streamed([sse("open", 1, {}), sse("done", 2, { reason: "timeout" })])
    ];
    const fetch = vi.fn(async () => answers.shift()?.() ?? streamed([]));
    vi.stubGlobal("fetch", fetch);
    const printer = recorder();
    await follow({ url: URL_BASE, headers: {} }, undefined, printer, noSleep);
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(printer.seen.map(([name]) => name)).toEqual(["dropped", "open", "done"]);
  });

  it("opens a new request on reauthenticate, carrying on from the last event", async () => {
    const headers: Record<string, string>[] = [];
    const bodies = [
      [sse("open", 1, {}), sse("log", 2, { seq: 1 }), sse("done", 3, { reason: "reauthenticate" })],
      [sse("open", 4, { resumed: true }), sse("done", 5, { reason: "timeout" })]
    ];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (_url: string, init?: RequestInit) => {
        headers.push({ ...(init?.headers as Record<string, string>) });
        return streamed(bodies.shift() ?? []);
      })
    );
    const printer = recorder();
    await follow({ url: URL_BASE, headers: {} }, undefined, printer, noSleep);
    expect(headers[1]?.["Last-Event-ID"]).toBe("3");
    expect(printer.seen.map(([name, data]) => `${name}:${label(data.reason)}`)).toEqual([
      "open:",
      "log:",
      "dropped:reauthenticate",
      "open:",
      "done:timeout"
    ]);
  });

  it("exits as an auth failure when the server ends the stream for access", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        streamed([sse("open", 1, {}), sse("done", 2, { reason: "access_revoked" })])
      )
    );
    const printer = recorder();
    const error = await follow({ url: URL_BASE, headers: {} }, undefined, printer, noSleep).catch(
      (e: unknown) => e
    );
    expect((error as CliError).code).toBe(ExitCode.AUTH);
    expect(printer.seen.map(([name]) => name)).toEqual(["open", "done"]);
  });

  it("throws the refusal's exit code without retrying", async () => {
    const fetch = vi.fn(
      async () =>
        new Response(JSON.stringify({ error: "non_production_refused", message: "staff only" }), {
          status: 403
        })
    );
    vi.stubGlobal("fetch", fetch);
    const error = await follow(
      { url: URL_BASE, headers: {} },
      undefined,
      recorder(),
      noSleep
    ).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CliError);
    expect((error as CliError).code).toBe(ExitCode.AUTH);
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("gives up as unavailable when the stream never opens", async () => {
    const fetch = vi.fn(async () => streamed([]));
    vi.stubGlobal("fetch", fetch);
    const error = await follow(
      { url: URL_BASE, headers: {} },
      undefined,
      recorder(),
      noSleep
    ).catch((e: unknown) => e);
    expect((error as CliError).code).toBe(ExitCode.UNAVAILABLE);
    expect(fetch).toHaveBeenCalledTimes(6);
  });
});

describe("followTimeout", () => {
  it("takes whole seconds in range and refuses the polled read's flags", () => {
    expect(followTimeout({})).toBeUndefined();
    expect(followTimeout({ timeout: "60" })).toBe(60);
    for (const timeout of ["0", "86401", "1.5", "soon"]) {
      expect(() => followTimeout({ timeout })).toThrow(/--timeout/);
    }
    for (const flag of ["invocation", "request", "hours", "limit"]) {
      const thrown = (() => {
        try {
          followTimeout({ [flag]: "1" });
        } catch (e) {
          return e as CliError;
        }
      })();
      expect(thrown?.code).toBe(ExitCode.USAGE);
    }
  });
});
