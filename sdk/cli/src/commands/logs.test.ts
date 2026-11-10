/**
 * `oxyc logs` — `GET /api/customer-apps/{org}/{app}/logs`, driven with
 * `globalThis.fetch` stubbed.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import { runLogs } from "./logs.js";

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
}

type RouteTable = Record<
  string,
  (init: RequestInit | undefined) => { status: number; body: unknown }
>;

function stubFetch(routes: RouteTable, calls: Call[]) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string | URL, init?: RequestInit) => {
      const url = String(input);
      const path = url.replace(TARGET, "");
      const method = (init?.method ?? "GET").toUpperCase();
      calls.push({ method, url });
      const key = `${method} ${path}`;
      const handler = routes[key];
      if (!handler)
        return new Response(JSON.stringify({ error: `no stub for ${key}` }), { status: 404 });
      const { status, body } = handler(init);
      return new Response(JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json" }
      });
    })
  );
}

/** The registry: only an app UUID needs it, to learn the app's slugs. */
function appDetailRoutes(): RouteTable {
  return {
    [`GET /api/customer-apps/${APP_ID}`]: () => ({
      status: 200,
      body: { id: APP_ID, slug: "store", org_slug: "acme" }
    })
  };
}

describe("runLogs", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  it("GETs /api/customer-apps/{org}/{app}/logs with no query by default", async () => {
    stubFetch(
      {
        "GET /api/customer-apps/acme/store/logs": () => ({ status: 200, body: { logs: [] } })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });

    await runLogs(fakeContext({ bearer: "tok" }), "acme/store", { json: true });
    write.mockRestore();

    expect(JSON.parse(printed.trim())).toEqual({ logs: [] });
    expect(calls.some((c) => c.url === `${TARGET}/api/customer-apps/acme/store/logs`)).toBe(true);
  });

  // The registry list sits on the staff console stack, which refuses a caller
  // inside an assume-role session; the logs route decides its own authority.
  // A slug pair must therefore reach the logs route and nothing else.
  it("reads <org>/<app> logs with one request and never walks the registry", async () => {
    stubFetch(
      { "GET /api/customer-apps/acme/store/logs": () => ({ status: 200, body: { logs: [] } }) },
      calls
    );

    await runLogs(fakeContext({ bearer: "tok" }), "acme/store", { json: true });

    expect(calls).toEqual([{ method: "GET", url: `${TARGET}/api/customer-apps/acme/store/logs` }]);
  });

  it("resolves an app UUID to its slugs through the registry detail", async () => {
    stubFetch(
      {
        ...appDetailRoutes(),
        "GET /api/customer-apps/acme/store/logs": () => ({ status: 200, body: { logs: [] } })
      },
      calls
    );

    await runLogs(fakeContext({ bearer: "tok" }), APP_ID, { json: true });

    expect(calls.map((c) => c.url)).toEqual([
      `${TARGET}/api/customer-apps/${APP_ID}`,
      `${TARGET}/api/customer-apps/acme/store/logs`
    ]);
  });

  it("maps a 404 from the logs route (unknown or unreachable app) to exit 5", async () => {
    stubFetch(
      {
        "GET /api/customer-apps/acme/nope/logs": () => ({
          status: 404,
          body: { error: "not permitted" }
        })
      },
      calls
    );
    const err = await runLogs(fakeContext({ bearer: "tok" }), "acme/nope", { json: true }).catch(
      (e: unknown) => e as CliError
    );
    expect((err as CliError).code).toBe(ExitCode.NOT_FOUND);
  });

  it("refuses an argument that is neither <org>/<app> nor a UUID before any request", async () => {
    stubFetch({}, calls);
    const err = await runLogs(fakeContext({ bearer: "tok" }), "acme", { json: true }).catch(
      (e: unknown) => e as CliError
    );
    expect((err as CliError).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });

  it("builds ?environment=&invocation_id=&request_id=&hours=&limit=", async () => {
    stubFetch(
      {
        "GET /api/customer-apps/acme/store/logs?environment=dev-a1&invocation_id=inv-1&request_id=req-1&hours=2&limit=50":
          () => ({ status: 200, body: { logs: [] } })
      },
      calls
    );

    await runLogs(fakeContext({ bearer: "tok" }), "acme/store", {
      appEnv: "dev-a1",
      invocation: "inv-1",
      request: "req-1",
      hours: 2,
      limit: 50,
      json: true
    });

    expect(
      calls.some(
        (c) =>
          c.url ===
          `${TARGET}/api/customer-apps/acme/store/logs?environment=dev-a1&invocation_id=inv-1&request_id=req-1&hours=2&limit=50`
      )
    ).toBe(true);
  });

  it("maps 501 (observability off) to exit 7", async () => {
    stubFetch(
      {
        "GET /api/customer-apps/acme/store/logs": () => ({
          status: 501,
          body: { error: "observability capture is not configured" }
        })
      },
      calls
    );
    const err = await runLogs(fakeContext({ bearer: "tok" }), "acme/store", { json: true }).catch(
      (e: unknown) => e as CliError
    );
    expect((err as CliError).code).toBe(ExitCode.UNAVAILABLE);
  });

  it("refuses a publish token (exit 2)", async () => {
    stubFetch({}, calls);
    const err = await runLogs(fakeContext({ bearer: "oxypublish_stored" }), "acme/store", {
      json: true
    }).catch((e: unknown) => e as CliError);
    expect((err as CliError).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });

  it("rejects a malformed --app-env before any request", async () => {
    stubFetch({}, calls);
    const err = await runLogs(fakeContext({ bearer: "tok" }), "acme/store", {
      appEnv: "not-valid",
      json: true
    }).catch((e: unknown) => e as CliError);
    expect((err as CliError).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });
});
