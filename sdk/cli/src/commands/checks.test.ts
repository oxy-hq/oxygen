/**
 * `oxyc checks run`, driven with `globalThis.fetch` stubbed.
 *
 * A ROUTE TABLE, not a local HTTP server: this command talks to the admin
 * surface (four fixed routes) and the thing worth pinning is the wire
 * shape at each of them plus the poll/timeout/pass-rule state machine — none
 * of which needs a real socket. No existing test in this package stubs
 * `fetch` this way (checked `commands/assume.test.ts`, `util/util.test.ts`,
 * `commands/proxy.test.ts` — all spawn the binary or a loopback server), so
 * this stubs it with `vi.stubGlobal("fetch", …)`.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import { runChecks } from "./checks.js";

const TARGET = "https://oxy.test";
const APP_ID = "a1a1a1a1-2222-3333-4444-555555555555";

/** A minimal `Context`, matching how `commands/auth.ts:runWhoami` receives one. */
function fakeContext(opts: { bearer?: string; apiKey?: string } = {}): Context {
  return {
    cwd: "/tmp",
    flags: { env: "production", tokenEnv: "OXY_TOKEN", apiKeyEnv: "OXY_API_KEY" },
    target: () => TARGET,
    env: () => ({ target: TARGET, orgSlug: undefined }) as ReturnType<Context["env"]>,
    bearer: () => {
      if (opts.bearer) return opts.bearer;
      throw new CliError(`not authenticated for ${TARGET}`, {
        code: ExitCode.AUTH,
        hint: "oxyc login --env production"
      });
    },
    maybeBearer: () => opts.bearer,
    apiKey: () => opts.apiKey,
    customer: () => undefined,
    repoDir: () => undefined,
    placeholders: () => ({}),
    withEnv: () => fakeContext(opts)
  };
}

/** One fetch handler, keyed by `METHOD path` (path includes the query string). */
type RouteTable = Record<
  string,
  (init: RequestInit | undefined) => { status: number; body: unknown }
>;

function stubFetch(
  routes: RouteTable,
  calls: { method: string; url: string; headers: Record<string, string> }[]
) {
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
      if (!handler) {
        return new Response(JSON.stringify({ error: `no stub for ${key}` }), { status: 404 });
      }
      const { status, body } = handler(init);
      return new Response(JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json" }
      });
    })
  );
}

const APPS_PAGE_1 = { items: [], next_offset: 100 };
const APPS_PAGE_2 = {
  items: [{ id: APP_ID, slug: "platform-canary", org_slug: "oxy-canary" }],
  next_offset: null
};

function appsRoutes(): RouteTable {
  return {
    "GET /api/admin/apps?limit=100&offset=0": () => ({ status: 200, body: APPS_PAGE_1 }),
    "GET /api/admin/apps?limit=100&offset=100": () => ({ status: 200, body: APPS_PAGE_2 })
  };
}

describe("runChecks", () => {
  let calls: { method: string; url: string; headers: Record<string, string> }[];

  beforeEach(() => {
    calls = [];
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
  });

  it("resolves an org/slug across pages of GET /api/admin/apps", async () => {
    let runs = 0;
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/admin/apps/${APP_ID}/functions`]: () => ({
          status: 200,
          body: [{ name: "canary", check: true }]
        }),
        [`POST /api/admin/apps/${APP_ID}/functions/canary/runs`]: () => {
          runs += 1;
          return { status: 200, body: { run_id: "run-1" } };
        },
        [`GET /api/admin/apps/${APP_ID}/function-runs/run-1`]: () => ({
          status: 200,
          body: { run_id: "run-1", status: "done", trigger: "manual", answer: null, error: null }
        })
      },
      calls
    );

    await runChecks(fakeContext({ bearer: "tok" }), "oxy-canary/platform-canary", {
      json: false,
      timeoutSeconds: 5,
      pollMs: 0
    });

    expect(runs).toBe(1);
    const appsCalls = calls.filter((c) => c.url.includes("/api/admin/apps?"));
    expect(appsCalls.map((c) => c.url)).toEqual([
      `${TARGET}/api/admin/apps?limit=100&offset=0`,
      `${TARGET}/api/admin/apps?limit=100&offset=100`
    ]);
  });

  it("mints a credential in CI and drives the machine surface", async () => {
    // No stored token and no API key: a job holding `id-token: write` exchanges
    // for the same short-lived app-scoped token `oxyc publish` uses, and then
    // talks to `/api/customer-apps/…` — the surface that token may reach. The
    // app-listing route is NOT reachable with it, so the app id has to come
    // from the exchange; if it did not, every call below would 404 on the stub.
    let runs = 0;
    stubFetch(
      {
        "GET /__gh/token?audience=oxy-publish": () => ({
          status: 200,
          body: { value: "gh-jwt" }
        }),
        "POST /api/customer-apps/publish/oidc-exchange": () => ({
          status: 200,
          body: { token: "oxypublish_minted", expires_at: "later", app_id: APP_ID }
        }),
        [`GET /api/customer-apps/${APP_ID}/functions`]: () => ({
          status: 200,
          body: [{ name: "canary", check: true }]
        }),
        [`POST /api/customer-apps/${APP_ID}/functions/canary/runs`]: () => {
          runs += 1;
          return { status: 200, body: { run_id: "run-1" } };
        },
        [`GET /api/customer-apps/${APP_ID}/function-runs/run-1`]: () => ({
          status: 200,
          body: { run_id: "run-1", status: "done", trigger: "manual", answer: null, error: null }
        })
      },
      calls
    );
    vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_URL", `${TARGET}/__gh/token`);
    vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "gh-request-token");

    await runChecks(fakeContext(), "oxy-canary/platform-canary", {
      json: false,
      timeoutSeconds: 5,
      pollMs: 0
    });

    expect(runs).toBe(1);
    expect(calls.some((c) => c.url.includes("/api/admin/"))).toBe(false);
    const run = calls.find((c) => c.method === "POST" && c.url.includes("/runs"));
    expect(run?.headers.authorization).toBe("Bearer oxypublish_minted");
  });

  it("uses the machine surface for a publish token handed in directly", async () => {
    // Same token, minted elsewhere (a long-lived CI publish token). It is the
    // credential that picks the surface, not the environment: the admin routes
    // refuse it, so sending it there would 403 on a path the user cannot fix.
    stubFetch(
      {
        [`GET /api/customer-apps/${APP_ID}/functions`]: () => ({ status: 200, body: [] })
      },
      calls
    );

    await expect(
      runChecks(fakeContext({ bearer: "oxypublish_stored" }), APP_ID, {
        json: false,
        timeoutSeconds: 5,
        pollMs: 0
      })
    ).rejects.toThrow(/declares no checks/);
    expect(calls.every((c) => c.url.includes("/api/customer-apps/"))).toBe(true);
  });

  it("names the version skew when the exchange returns no app_id", async () => {
    // The other half of the same decision: `publish` must not fail over a field
    // it never reads, so the check lives here, where the id is actually needed.
    stubFetch(
      {
        "GET /__gh/token?audience=oxy-publish": () => ({ status: 200, body: { value: "gh-jwt" } }),
        "POST /api/customer-apps/publish/oidc-exchange": () => ({
          status: 200,
          body: { token: "oxypublish_minted", expires_at: "later" }
        })
      },
      calls
    );
    vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_URL", `${TARGET}/__gh/token`);
    vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "gh-request-token");

    await expect(
      runChecks(fakeContext(), "oxy-canary/platform-canary", {
        json: false,
        timeoutSeconds: 5,
        pollMs: 0
      })
    ).rejects.toThrow(/returned no app_id/);
    // It stopped at the exchange — no doomed request to a route it cannot reach.
    expect(calls.filter((c) => c.url.includes("/functions"))).toHaveLength(0);
  });

  it("refuses a slug when the credential cannot resolve one", async () => {
    // `resolveApp` pages `/api/admin/apps`, which a publish token may not
    // reach — the request would 403 on a route the caller cannot be given, so
    // it is never sent. No route stub: any request at all fails this test.
    stubFetch({}, calls);

    await expect(
      runChecks(fakeContext({ bearer: "oxypublish_stored" }), "oxy-canary/platform-canary", {
        json: false,
        timeoutSeconds: 5,
        pollMs: 0
      })
    ).rejects.toThrow(/cannot resolve <org>\/<app>/);
    expect(calls).toHaveLength(0);
  });

  it("runs only check:true functions, in name order", async () => {
    const runPaths: string[] = [];
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/admin/apps/${APP_ID}/functions`]: () => ({
          status: 200,
          body: [
            { name: "echo", check: false },
            { name: "canary", check: true }
          ]
        }),
        [`POST /api/admin/apps/${APP_ID}/functions/canary/runs`]: () => {
          runPaths.push("canary");
          return { status: 200, body: { run_id: "run-1" } };
        },
        [`GET /api/admin/apps/${APP_ID}/function-runs/run-1`]: () => ({
          status: 200,
          body: { run_id: "run-1", status: "done", answer: null, error: null }
        })
      },
      calls
    );

    await runChecks(fakeContext({ bearer: "tok" }), "oxy-canary/platform-canary", {
      json: false,
      timeoutSeconds: 5,
      pollMs: 0
    });

    expect(runPaths).toEqual(["canary"]);
  });

  it("polls until terminal: queued → running → done passes with no error", async () => {
    let poll = 0;
    const statuses = ["queued", "running", "done"];
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/admin/apps/${APP_ID}/functions`]: () => ({
          status: 200,
          body: [{ name: "canary", check: true }]
        }),
        [`POST /api/admin/apps/${APP_ID}/functions/canary/runs`]: () => ({
          status: 200,
          body: { run_id: "run-1" }
        }),
        [`GET /api/admin/apps/${APP_ID}/function-runs/run-1`]: () => {
          const status = statuses[Math.min(poll, statuses.length - 1)];
          poll += 1;
          return { status: 200, body: { run_id: "run-1", status, answer: null, error: null } };
        }
      },
      calls
    );

    await expect(
      runChecks(fakeContext({ bearer: "tok" }), "oxy-canary/platform-canary", {
        json: false,
        timeoutSeconds: 5,
        pollMs: 0
      })
    ).resolves.toBeUndefined();
    expect(poll).toBeGreaterThanOrEqual(3);
  });

  it("a `failed` run status fails the check with exit code 9", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/admin/apps/${APP_ID}/functions`]: () => ({
          status: 200,
          body: [{ name: "canary", check: true }]
        }),
        [`POST /api/admin/apps/${APP_ID}/functions/canary/runs`]: () => ({
          status: 200,
          body: { run_id: "run-1" }
        }),
        [`GET /api/admin/apps/${APP_ID}/function-runs/run-1`]: () => ({
          status: 200,
          body: { run_id: "run-1", status: "failed", answer: null, error: "boom" }
        })
      },
      calls
    );

    const err = await runChecks(fakeContext({ bearer: "tok" }), "oxy-canary/platform-canary", {
      json: false,
      timeoutSeconds: 5,
      pollMs: 0
    }).catch((e: unknown) => e as CliError);

    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.CHECK_FAILED);
  });

  it("`done` with an `ok:false` answer fails the check with exit code 9", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/admin/apps/${APP_ID}/functions`]: () => ({
          status: 200,
          body: [{ name: "canary", check: true }]
        }),
        [`POST /api/admin/apps/${APP_ID}/functions/canary/runs`]: () => ({
          status: 200,
          body: { run_id: "run-1" }
        }),
        [`GET /api/admin/apps/${APP_ID}/function-runs/run-1`]: () => ({
          status: 200,
          body: {
            run_id: "run-1",
            status: "done",
            answer: JSON.stringify({ ok: false }),
            error: null
          }
        })
      },
      calls
    );

    const err = await runChecks(fakeContext({ bearer: "tok" }), "oxy-canary/platform-canary", {
      json: false,
      timeoutSeconds: 5,
      pollMs: 0
    }).catch((e: unknown) => e as CliError);

    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.CHECK_FAILED);
  });

  it("a run that stays `running` past timeoutSeconds times out with exit code 9", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/admin/apps/${APP_ID}/functions`]: () => ({
          status: 200,
          body: [{ name: "canary", check: true }]
        }),
        [`POST /api/admin/apps/${APP_ID}/functions/canary/runs`]: () => ({
          status: 200,
          body: { run_id: "run-1" }
        }),
        [`GET /api/admin/apps/${APP_ID}/function-runs/run-1`]: () => ({
          status: 200,
          body: { run_id: "run-1", status: "running", answer: null, error: null }
        })
      },
      calls
    );

    let report: unknown;
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      report = chunk;
      return true;
    });

    // 1 ms, not 0: a timeout of zero or less is now a usage error (see below).
    const err = await runChecks(fakeContext({ bearer: "tok" }), "oxy-canary/platform-canary", {
      json: true,
      timeoutSeconds: 0.001,
      pollMs: 0
    }).catch((e: unknown) => e as CliError);
    write.mockRestore();

    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.CHECK_FAILED);
    const parsed = JSON.parse(String(report)) as { checks: { status: string; passed: boolean }[] };
    expect(parsed.checks[0]?.status).toBe("timed_out");
    expect(parsed.checks[0]?.passed).toBe(false);
  });

  it('no check:true functions throws exit code 1, naming "check": true', async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/admin/apps/${APP_ID}/functions`]: () => ({
          status: 200,
          body: [{ name: "echo", check: false }]
        })
      },
      calls
    );

    const err = await runChecks(fakeContext({ bearer: "tok" }), "oxy-canary/platform-canary", {
      json: false,
      timeoutSeconds: 5,
      pollMs: 0
    }).catch((e: unknown) => e as CliError);

    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.FAILURE);
    expect((err as CliError).message).toContain('"check": true');
  });

  it("an app that never matches throws exit code 5", async () => {
    stubFetch(appsRoutes(), calls);

    const err = await runChecks(fakeContext({ bearer: "tok" }), "oxy-canary/no-such-app", {
      json: false,
      timeoutSeconds: 5,
      pollMs: 0
    }).catch((e: unknown) => e as CliError);

    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.NOT_FOUND);
  });

  // `main.ts` passes `Number(opts.timeout)`: `--timeout abc` is NaN, and a NaN
  // deadline never passes, so the poll would never end.
  it.each([
    ["non-numeric (NaN)", Number("abc")],
    ["zero", 0],
    ["negative", -5],
    ["infinite", Number.POSITIVE_INFINITY]
  ])(
    "--timeout %s is a usage error (exit 2) before any request",
    async (_label, timeoutSeconds) => {
      stubFetch(appsRoutes(), calls);

      const err = await runChecks(fakeContext({ bearer: "tok" }), APP_ID, {
        json: false,
        timeoutSeconds,
        pollMs: 0
      }).catch((e: unknown) => e as CliError);

      expect(err).toBeInstanceOf(CliError);
      expect((err as CliError).code).toBe(ExitCode.USAGE);
      expect((err as CliError).message).toContain("--timeout");
      expect(calls).toHaveLength(0);
    }
  );

  it("with no bearer and an API key, requests carry X-API-Key and no Authorization", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/admin/apps/${APP_ID}/functions`]: () => ({
          status: 200,
          body: [{ name: "canary", check: true }]
        }),
        [`POST /api/admin/apps/${APP_ID}/functions/canary/runs`]: () => ({
          status: 200,
          body: { run_id: "run-1" }
        }),
        [`GET /api/admin/apps/${APP_ID}/function-runs/run-1`]: () => ({
          status: 200,
          body: { run_id: "run-1", status: "done", answer: null, error: null }
        })
      },
      calls
    );

    await runChecks(fakeContext({ apiKey: "sekrit" }), "oxy-canary/platform-canary", {
      json: false,
      timeoutSeconds: 5,
      pollMs: 0
    });

    expect(calls.length).toBeGreaterThan(0);
    for (const call of calls) {
      expect(call.headers["x-api-key"]).toBe("sekrit");
      expect(call.headers.authorization).toBeUndefined();
    }
  });

  it("--json prints one JSON object with the documented shape on stdout", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/admin/apps/${APP_ID}/functions`]: () => ({
          status: 200,
          body: [{ name: "canary", check: true }]
        }),
        [`POST /api/admin/apps/${APP_ID}/functions/canary/runs`]: () => ({
          status: 200,
          body: { run_id: "run-1" }
        }),
        [`GET /api/admin/apps/${APP_ID}/function-runs/run-1`]: () => ({
          status: 200,
          body: { run_id: "run-1", status: "done", answer: null, error: null }
        })
      },
      calls
    );

    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });

    await runChecks(fakeContext({ bearer: "tok" }), "oxy-canary/platform-canary", {
      json: true,
      timeoutSeconds: 5,
      pollMs: 0
    });
    write.mockRestore();

    const parsed = JSON.parse(printed.trim()) as {
      app: string;
      appId: string;
      checks: {
        name: string;
        runId: string;
        status: string;
        passed: boolean;
        durationMs: number;
      }[];
    };
    expect(parsed).toEqual({
      app: "oxy-canary/platform-canary",
      appId: APP_ID,
      checks: [
        {
          name: "canary",
          runId: "run-1",
          status: "done",
          passed: true,
          durationMs: expect.any(Number)
        }
      ]
    });
  });
});
