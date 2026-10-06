/**
 * `oxyc fn call` — direct `fetch` against the function-call route (the
 * response is SSE text, not JSON), driven with `globalThis.fetch` stubbed.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import { type FnCallResult, parseFunctionStream, runFnCall } from "./fn.js";

const TARGET = "https://oxy.test";
const APP_ID = "a1a1a1a1-2222-3333-4444-555555555555";

function fakeContext(opts: { bearer?: string; apiKey?: string } = {}): Context {
  return {
    cwd: "/tmp",
    flags: { env: "production", apiKeyEnv: "OXY_API_KEY" },
    target: () => TARGET,
    env: () => ({ target: TARGET, orgSlug: undefined }) as ReturnType<Context["env"]>,
    bearer: async () => {
      if (opts.bearer) return opts.bearer;
      throw new CliError(`not authenticated for ${TARGET}`, {
        code: ExitCode.AUTH,
        hint: "oxyc login --env production"
      });
    },
    maybeBearer: async () => opts.bearer,
    storedBearer: () => opts.bearer,
    async credential() {
      return { token: await this.bearer(), source: "env" };
    },
    serviceAccount: () => undefined,
    apiKey: () => opts.apiKey,
    customer: () => undefined,
    repoDir: () => undefined,
    placeholders: () => ({}),
    withEnv: () => fakeContext(opts)
  };
}

interface Call {
  method: string;
  url: string;
  headers: Record<string, string>;
}

type RouteTable = Record<
  string,
  (init: RequestInit | undefined) => {
    status: number;
    body: string;
    headers?: Record<string, string>;
  }
>;

function stubFetch(routes: RouteTable, calls: Call[]) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string | URL, init?: RequestInit) => {
      const url = String(input);
      const path = url.replace(TARGET, "");
      const method = (init?.method ?? "GET").toUpperCase();
      const headers: Record<string, string> = {};
      new Headers(init?.headers).forEach((v, k) => {
        headers[k] = v;
      });
      calls.push({ method, url, headers });
      const key = `${method} ${path}`;
      const handler = routes[key];
      if (!handler)
        return new Response(JSON.stringify({ error: `no stub for ${key}` }), { status: 404 });
      const { status, body, headers: responseHeaders } = handler(init);
      return new Response(body, { status, headers: responseHeaders });
    })
  );
}

function sse(...frames: Array<[string, unknown]>): string {
  return frames
    .map(([event, data]) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`)
    .join("");
}

/** `<app>` already as "org/app" needs no admin lookup at all — see the UUID test below for the one that does. */
function appAdminLookup(): RouteTable {
  return {
    [`GET /api/admin/apps/${APP_ID}`]: () => ({
      status: 200,
      body: JSON.stringify({ id: APP_ID, slug: "store", org_slug: "acme" })
    })
  };
}

describe("parseFunctionStream", () => {
  it("parses log frames plus a successful done", () => {
    const text = sse(
      ["log", { level: "log", message: "saved" }],
      ["data", { id: 7 }],
      ["done", { status: 200 }]
    );
    const result = parseFunctionStream(text);
    expect(result).toEqual({
      ok: true,
      status: 200,
      body: { id: 7 },
      logs: [{ level: "log", message: "saved" }],
      error: undefined
    });
  });

  it("a done with a non-2xx status is ok:false", () => {
    const text = sse(["data", { error: "forbidden" }], ["done", { status: 403 }]);
    const result = parseFunctionStream(text);
    expect(result.ok).toBe(false);
    expect(result.status).toBe(403);
  });

  it("an error frame is ok:false with the message", () => {
    const text = sse(["error", { error: "Exception", message: "boom" }]);
    const result = parseFunctionStream(text);
    expect(result.ok).toBe(false);
    expect(result.error).toBe("boom");
  });
});

describe("runFnCall", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  it("POSTs to /customer-apps/{org}/{app}/fn/{name} (no /api) and reports a 2xx done as exit 0", async () => {
    stubFetch(
      {
        "POST /customer-apps/acme/store/fn/submit-order": () => ({
          status: 200,
          body: sse(["data", { id: 7 }], ["done", { status: 200 }]),
          headers: { "x-oxy-invocation-id": "inv-1" }
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });

    await runFnCall(fakeContext({ bearer: "tok" }), "acme/store", "submit-order", {
      json: true,
      timeoutSeconds: 5
    });
    write.mockRestore();

    const result = JSON.parse(printed.trim()) as FnCallResult;
    expect(result).toMatchObject({
      function: "submit-order",
      environment: "production",
      invocationId: "inv-1",
      ok: true,
      status: 200,
      body: { id: 7 }
    });
    const call = calls.find((c) => c.url.includes("/fn/submit-order"));
    expect(call?.url).toBe(`${TARGET}/customer-apps/acme/store/fn/submit-order`);
  });

  it("a done with 403 is exit 1 (FAILURE), ok:false", async () => {
    stubFetch(
      {
        "POST /customer-apps/acme/store/fn/submit-order": () => ({
          status: 200,
          body: sse(["data", { error: "forbidden" }], ["done", { status: 403 }])
        })
      },
      calls
    );

    const error = await runFnCall(fakeContext({ bearer: "tok" }), "acme/store", "submit-order", {
      json: false,
      timeoutSeconds: 5
    }).catch((e: unknown) => e as CliError);

    expect(error).toBeInstanceOf(CliError);
    expect((error as CliError).code).toBe(ExitCode.FAILURE);
  });

  it("an error frame is exit 1 (FAILURE)", async () => {
    stubFetch(
      {
        "POST /customer-apps/acme/store/fn/submit-order": () => ({
          status: 200,
          body: sse(["error", { error: "Exception", message: "boom" }])
        })
      },
      calls
    );

    const error = await runFnCall(fakeContext({ bearer: "tok" }), "acme/store", "submit-order", {
      json: false,
      timeoutSeconds: 5
    }).catch((e: unknown) => e as CliError);

    expect(error).toBeInstanceOf(CliError);
    expect((error as CliError).code).toBe(ExitCode.FAILURE);
  });

  it("a JSON 403 before the stream starts is exit 4 (AUTH)", async () => {
    stubFetch(
      {
        "POST /customer-apps/acme/store/fn/submit-order": () => ({
          status: 403,
          body: JSON.stringify({ error: "EnvironmentRefused", message: "not admitted" })
        })
      },
      calls
    );

    const error = await runFnCall(fakeContext({ bearer: "tok" }), "acme/store", "submit-order", {
      json: false,
      timeoutSeconds: 5
    }).catch((e: unknown) => e as CliError);

    expect(error).toBeInstanceOf(CliError);
    expect((error as CliError).code).toBe(ExitCode.AUTH);
  });

  it("sends X-Oxy-App-Env for a sandbox and omits it for production", async () => {
    stubFetch(
      {
        "POST /customer-apps/acme/store/fn/submit-order": () => ({
          status: 200,
          body: sse(["done", { status: 200 }])
        })
      },
      calls
    );

    await runFnCall(fakeContext({ bearer: "tok" }), "acme/store", "submit-order", {
      json: true,
      timeoutSeconds: 5,
      appEnv: "dev-a1"
    });
    const sandboxCall = calls.find((c) => c.url.includes("/fn/"));
    expect(sandboxCall?.headers["x-oxy-app-env"]).toBe("dev-a1");

    calls = [];
    await runFnCall(fakeContext({ bearer: "tok" }), "acme/store", "submit-order", {
      json: true,
      timeoutSeconds: 5
    });
    const prodCall = calls.find((c) => c.url.includes("/fn/"));
    expect(prodCall?.headers["x-oxy-app-env"]).toBeUndefined();
  });

  it("a UUID <app> resolves org/app through GET /api/admin/apps/{id} first", async () => {
    stubFetch(
      {
        ...appAdminLookup(),
        "POST /customer-apps/acme/store/fn/submit-order": () => ({
          status: 200,
          body: sse(["done", { status: 200 }])
        })
      },
      calls
    );

    await runFnCall(fakeContext({ bearer: "tok" }), APP_ID, "submit-order", {
      json: true,
      timeoutSeconds: 5
    });

    expect(calls.some((c) => c.url === `${TARGET}/api/admin/apps/${APP_ID}`)).toBe(true);
    expect(calls.some((c) => c.url === `${TARGET}/customer-apps/acme/store/fn/submit-order`)).toBe(
      true
    );
  });

  it("refuses a publish-token credential before any request for --app-env other than production (exit 2)", async () => {
    stubFetch({}, calls);

    const error = await runFnCall(
      fakeContext({ bearer: "oxypublish_stored" }),
      "acme/store",
      "submit-order",
      { json: true, timeoutSeconds: 5, appEnv: "dev-a1" }
    ).catch((e: unknown) => e as CliError);

    expect(error).toBeInstanceOf(CliError);
    expect((error as CliError).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });

  it("a publish-token credential still reaches production", async () => {
    stubFetch(
      {
        "POST /customer-apps/acme/store/fn/submit-order": () => ({
          status: 200,
          body: sse(["done", { status: 200 }])
        })
      },
      calls
    );

    await expect(
      runFnCall(fakeContext({ bearer: "oxypublish_stored" }), "acme/store", "submit-order", {
        json: true,
        timeoutSeconds: 5
      })
    ).resolves.toBeUndefined();
  });

  it("refuses a publish-token credential given a UUID <app> (exit 2), making no request", async () => {
    // The admin-apps-by-id lookup a UUID needs is the one route a publish
    // token must never reach (same invariant checks.ts enforces, in the
    // opposite direction). Without this guard the request is sent and the
    // server 403s, surfacing as a confusing exit 4.
    stubFetch({}, calls);

    const error = await runFnCall(
      fakeContext({ bearer: "oxypublish_stored" }),
      APP_ID,
      "submit-order",
      { json: true, timeoutSeconds: 5 }
    ).catch((e: unknown) => e as CliError);

    expect(error).toBeInstanceOf(CliError);
    expect((error as CliError).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });
});
