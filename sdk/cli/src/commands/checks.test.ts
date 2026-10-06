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

import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { pendingRevokes, runExitRevokes } from "../auth/exit-revoke.js";
import { resetOidcExchanges } from "../auth/oidc.js";
import { type Context, createContext } from "../context/resolve.js";
import { SERVICE_ACCOUNT_ID } from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import { type ChecksReport, runChecks } from "./checks.js";

const TARGET = "https://oxy.test";
const APP_ID = "a1a1a1a1-2222-3333-4444-555555555555";

/**
 * The REAL `Context`, with the credential sources pinned through the
 * environment: `OXY_TOKEN`, `OXY_API_KEY`, and a credentials file that does not
 * exist. Real rather than a stand-in because the order those sources are
 * consulted in — and where the GitHub OIDC exchange falls in it — is part of
 * what these cases pin.
 */
function contextWith(
  opts: { bearer?: string; apiKey?: string; serviceAccount?: string } = {}
): Context {
  vi.stubEnv("OXY_TOKEN", opts.bearer ?? "");
  vi.stubEnv("OXY_API_KEY", opts.apiKey ?? "");
  vi.stubEnv("OXY_SERVICE_ACCOUNT", opts.serviceAccount ?? "");
  vi.stubEnv("OXY_CREDENTIALS_PATH", join(tmpdir(), "oxyc-no-such-credentials.json"));
  return createContext({ env: "production", target: TARGET }, tmpdir());
}

/** A job with `id-token: write`. `calls` shows which audience was asked for. */
function inGithubActions(): void {
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_URL", `${TARGET}/__gh/token`);
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "gh-request-token");
}

/** GitHub minting an id token for either audience. */
const GITHUB_ROUTES = {
  "GET /__gh/token?audience=oxy%3Aoxy.test": () => ({
    status: 200,
    body: { value: "gh-jwt-oxy" }
  }),
  "GET /__gh/token?audience=oxy-publish": () => ({ status: 200, body: { value: "gh-jwt" } })
};

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
      // A 204 may not carry a body — `Response` throws if it is given one.
      if (status === 204) return new Response(null, { status });
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
    // Each case stands in for a separate process, and the exchange is
    // memoised per process.
    resetOidcExchanges();
  });

  afterEach(async () => {
    // Drain what a case minted while `fetch` is still the stub — never the
    // network.
    await runExitRevokes();
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

    await runChecks(contextWith({ bearer: "tok" }), "oxy-canary/platform-canary", {
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
    //
    // The job names no service account, so the general exchange is never
    // asked and the app's publisher is where the credential comes from — the
    // path every workflow written before trust policies existed depends on.
    let runs = 0;
    stubFetch(
      {
        ...GITHUB_ROUTES,
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
    inGithubActions();

    await runChecks(contextWith(), "oxy-canary/platform-canary", {
      json: false,
      timeoutSeconds: 5,
      pollMs: 0
    });

    expect(runs).toBe(1);
    expect(calls.some((c) => c.url.includes("/api/admin/"))).toBe(false);
    const run = calls.find((c) => c.method === "POST" && c.url.includes("/runs"));
    expect(run?.headers.authorization).toBe("Bearer oxypublish_minted");
    // Straight to the publisher: with no account named, nothing is put to
    // the general exchange at all.
    const exchanges = calls.filter((c) => c.url.includes("exchange")).map((c) => c.url);
    expect(exchanges).toEqual([`${TARGET}/api/customer-apps/publish/oidc-exchange`]);
  });

  it("with an account named, asks the general exchange first and the publisher on its 404", async () => {
    stubFetch(
      {
        ...GITHUB_ROUTES,
        "POST /api/customer-apps/publish/oidc-exchange": () => ({
          status: 200,
          body: { token: "oxypublish_minted", expires_at: "later", app_id: APP_ID }
        }),
        [`GET /api/customer-apps/${APP_ID}/functions`]: () => ({ status: 200, body: [] })
      },
      calls
    );
    inGithubActions();

    await runChecks(
      contextWith({ serviceAccount: SERVICE_ACCOUNT_ID }),
      "oxy-canary/platform-canary",
      { json: false, timeoutSeconds: 5, pollMs: 0 }
    ).catch(() => undefined);

    // Each with an id token of its own audience.
    const exchanges = calls.filter((c) => c.url.includes("exchange")).map((c) => c.url);
    expect(exchanges).toEqual([
      `${TARGET}/api/auth/oidc/exchange`,
      `${TARGET}/api/customer-apps/publish/oidc-exchange`
    ]);
  });

  it("acts as a service account when a trust policy matches, finding the app in its grants", async () => {
    let runs = 0;
    stubFetch(
      {
        ...GITHUB_ROUTES,
        "POST /api/auth/oidc/exchange": () => ({
          status: 200,
          body: {
            token: "oxy_ci_minted",
            token_id: "tok-1",
            expires_at: "2099-01-01T00:00:00Z",
            service_account: "oxy-canary/deployer",
            grants: []
          }
        }),
        "GET /api/auth/token": () => ({
          status: 200,
          body: {
            id: "tok-1",
            name: "ci",
            kind: "ci",
            grants: [
              {
                id: "g1",
                kind: "app_publish",
                org_id: "o1",
                org_name: "Oxy Canary",
                app_id: APP_ID,
                app_name: "platform-canary",
                revoked_at: null
              }
            ]
          }
        }),
        "DELETE /api/auth/token": () => ({ status: 204, body: null }),
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
    inGithubActions();

    await runChecks(
      contextWith({ serviceAccount: SERVICE_ACCOUNT_ID }),
      "oxy-canary/platform-canary",
      { json: false, timeoutSeconds: 5, pollMs: 0 }
    );

    expect(runs).toBe(1);
    const run = calls.find((c) => c.method === "POST" && c.url.includes("/runs"));
    expect(run?.headers.authorization).toBe("Bearer oxy_ci_minted");
    // A service account has no platform standing: never the admin surface, and
    // never the publisher exchange once the general one has answered.
    expect(calls.some((c) => c.url.includes("/api/admin/"))).toBe(false);
    expect(calls.some((c) => c.url.includes("publish/oidc-exchange"))).toBe(false);
    // Minted here, so queued for revocation when the process ends.
    expect(pendingRevokes()).toEqual(["oxy_ci_minted"]);
  });

  it("does not run a different app's checks because the token happens to be scoped to one", async () => {
    // The token names ONE app, and it is not the one asked for. Using it would
    // report a pass under this app's name for checks nobody ran here.
    stubFetch(
      {
        "GET /api/auth/token": () => ({
          status: 200,
          body: {
            id: "tok-1",
            name: "ci",
            kind: "ci",
            grants: [
              {
                id: "g1",
                kind: "app_publish",
                org_id: "o1",
                org_name: "Oxy Canary",
                app_id: APP_ID,
                app_name: "some-other-app",
                revoked_at: null
              }
            ]
          }
        })
      },
      calls
    );

    await expect(
      runChecks(contextWith({ bearer: "oxy_ci_handed_in" }), "oxy-canary/platform-canary", {
        json: false,
        timeoutSeconds: 5,
        pollMs: 0
      })
    ).rejects.toThrow(/cannot resolve <org>\/<app>/);
    expect(calls.some((c) => c.url.includes("/functions"))).toBe(false);
  });

  it("prefers OXY_API_KEY to minting, in a job that could do either", async () => {
    // The release canary's shape: a staff key in a job that also holds
    // `id-token: write`. Minting over it would swap the key for a service
    // account that cannot reach the admin surface.
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/admin/apps/${APP_ID}/functions`]: () => ({ status: 200, body: [] })
      },
      calls
    );
    inGithubActions();

    await expect(
      runChecks(contextWith({ apiKey: "oxy_canary_key" }), "oxy-canary/platform-canary", {
        json: false,
        timeoutSeconds: 5,
        pollMs: 0
      })
    ).rejects.toThrow(/declares no checks/);
    expect(calls.some((c) => c.url.includes("/__gh/token"))).toBe(false);
    expect(calls.every((c) => c.headers["x-api-key"] === "oxy_canary_key")).toBe(true);
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
      runChecks(contextWith({ bearer: "oxypublish_stored" }), APP_ID, {
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
        ...GITHUB_ROUTES,
        "POST /api/customer-apps/publish/oidc-exchange": () => ({
          status: 200,
          body: { token: "oxypublish_minted", expires_at: "later" }
        })
      },
      calls
    );
    inGithubActions();

    await expect(
      runChecks(contextWith(), "oxy-canary/platform-canary", {
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
      runChecks(contextWith({ bearer: "oxypublish_stored" }), "oxy-canary/platform-canary", {
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

    await runChecks(contextWith({ bearer: "tok" }), "oxy-canary/platform-canary", {
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
      runChecks(contextWith({ bearer: "tok" }), "oxy-canary/platform-canary", {
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

    const err = await runChecks(contextWith({ bearer: "tok" }), "oxy-canary/platform-canary", {
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

    const err = await runChecks(contextWith({ bearer: "tok" }), "oxy-canary/platform-canary", {
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
    const err = await runChecks(contextWith({ bearer: "tok" }), "oxy-canary/platform-canary", {
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

    const err = await runChecks(contextWith({ bearer: "tok" }), "oxy-canary/platform-canary", {
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

    const err = await runChecks(contextWith({ bearer: "tok" }), "oxy-canary/no-such-app", {
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

      const err = await runChecks(contextWith({ bearer: "tok" }), APP_ID, {
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

    await runChecks(contextWith({ apiKey: "sekrit" }), "oxy-canary/platform-canary", {
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

    await runChecks(contextWith({ bearer: "tok" }), "oxy-canary/platform-canary", {
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

describe("runChecks --app-env", () => {
  let calls: { method: string; url: string; headers: Record<string, string> }[];

  beforeEach(() => {
    calls = [];
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
  });

  it("appends ?environment= to all three routes and the report carries environment and invocationId", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/admin/apps/${APP_ID}/functions?environment=dev-a1`]: () => ({
          status: 200,
          body: [{ name: "canary", check: true }]
        }),
        [`POST /api/admin/apps/${APP_ID}/functions/canary/runs?environment=dev-a1`]: () => ({
          status: 200,
          body: { run_id: "run-1" }
        }),
        [`GET /api/admin/apps/${APP_ID}/function-runs/run-1?environment=dev-a1`]: () => ({
          status: 200,
          body: {
            run_id: "run-1",
            status: "done",
            answer: null,
            error: null,
            invocation_id: "inv-1"
          }
        })
      },
      calls
    );

    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });

    await runChecks(contextWith({ bearer: "tok" }), "oxy-canary/platform-canary", {
      json: true,
      timeoutSeconds: 5,
      pollMs: 0,
      appEnv: "dev-a1"
    });
    write.mockRestore();

    const parsed = JSON.parse(printed.trim()) as ChecksReport;
    expect(parsed.environment).toBe("dev-a1");
    expect(parsed.checks[0]?.invocationId).toBe("inv-1");
  });

  it("without --app-env, the requests and the report stay byte-identical to today's", async () => {
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

    await runChecks(contextWith({ bearer: "tok" }), "oxy-canary/platform-canary", {
      json: true,
      timeoutSeconds: 5,
      pollMs: 0
    });
    write.mockRestore();

    const parsed = JSON.parse(printed.trim()) as Record<string, unknown>;
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
    expect("environment" in parsed).toBe(false);
    const checkCalls = calls.filter((c) => c.url.includes("/functions") || c.url.includes("/runs"));
    expect(checkCalls.length).toBeGreaterThan(0);
    expect(checkCalls.every((c) => !c.url.includes("environment="))).toBe(true);
  });

  it("refuses a publish-token credential with a non-production --app-env (exit 2), before any request", async () => {
    stubFetch({}, calls);

    const error = await runChecks(contextWith({ bearer: "oxypublish_stored" }), APP_ID, {
      json: false,
      timeoutSeconds: 5,
      pollMs: 0,
      appEnv: "dev-a1"
    }).catch((e: unknown) => e as CliError);

    expect(error).toBeInstanceOf(CliError);
    expect((error as CliError).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });

  it("rejects a malformed --app-env with a usage error before any request", async () => {
    stubFetch({}, calls);

    const error = await runChecks(contextWith({ bearer: "tok" }), APP_ID, {
      json: false,
      timeoutSeconds: 5,
      pollMs: 0,
      appEnv: "not-a-real-env"
    }).catch((e: unknown) => e as CliError);

    expect(error).toBeInstanceOf(CliError);
    expect((error as CliError).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });

  it("refuses --app-env dev-a1 before the OIDC exchange, with no stored credential (exit 2, zero calls)", async () => {
    // A CI job with `id-token: write` and nothing stored (no OXY_TOKEN, no
    // login cache) is exactly the shape that minted a token and threw it
    // away: either exchange yields a machine token (a service account's, or
    // an `oxypublish_…` one), which --app-env dev-a1 was always going to
    // refuse. The fix is to refuse
    // BEFORE minting, so this asserts zero network calls — not just the
    // right exit code, which a late refusal would also produce.
    stubFetch(
      {
        "GET /__gh/token?audience=oxy-publish": () => ({ status: 200, body: { value: "gh-jwt" } }),
        "POST /api/customer-apps/publish/oidc-exchange": () => ({
          status: 200,
          body: { token: "oxypublish_minted", expires_at: "later", app_id: APP_ID }
        })
      },
      calls
    );
    vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_URL", `${TARGET}/__gh/token`);
    vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "gh-request-token");

    const error = await runChecks(contextWith(), "oxy-canary/platform-canary", {
      json: false,
      timeoutSeconds: 5,
      pollMs: 0,
      appEnv: "dev-a1"
    }).catch((e: unknown) => e as CliError);

    expect(error).toBeInstanceOf(CliError);
    expect((error as CliError).code).toBe(ExitCode.USAGE);
    // The real assertion: no GitHub token request, no exchange, nothing.
    expect(calls).toHaveLength(0);
  });

  it("without --app-env, the OIDC exchange still runs — unchanged from before this fix", async () => {
    // Regression guard for the ordering fix above: production (no --app-env)
    // must still exchange and succeed exactly as `runChecks`'s existing
    // "mints a credential in CI" test (outside this describe block) already
    // pins in full. This only re-confirms the exchange is still REACHED. (No
    // general exchange here — its 404 sends the job to the app's publisher.)
    let runs = 0;
    stubFetch(
      {
        ...GITHUB_ROUTES,
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
          body: { run_id: "run-1", status: "done", answer: null, error: null }
        })
      },
      calls
    );
    inGithubActions();

    await runChecks(contextWith(), "oxy-canary/platform-canary", {
      json: false,
      timeoutSeconds: 5,
      pollMs: 0
    });

    expect(runs).toBe(1);
    expect(calls.some((c) => c.url.includes("/oidc-exchange"))).toBe(true);
  });
});
