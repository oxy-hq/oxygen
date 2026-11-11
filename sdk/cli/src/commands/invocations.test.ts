/**
 * `oxyc invocations list` / `held` — the read-back of what ran in
 * an app's environment, driven with `globalThis.fetch` stubbed.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import { runInvocationsHeld, runInvocationsList } from "./invocations.js";

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

function appsRoutes(): RouteTable {
  return {
    "GET /api/customer-apps?limit=100&offset=0": () => ({
      status: 200,
      body: { items: [{ id: APP_ID, slug: "store", org_slug: "acme" }], next_offset: null }
    })
  };
}

/**
 * One row as the server serializes it (`InvocationSummary` in
 * `admin/apps/invocations.rs`): the id is `id`. Only the held route and a run
 * detail call it `invocation_id`.
 */
const ROW = {
  id: "9b7a1c2e-0000-4000-8000-000000000001",
  function_name: "submit-order",
  mode: "route",
  environment: "dev-a1",
  build_id: "k3x9",
  build_uuid: "5d0e7f6a-0000-4000-8000-000000000002",
  status: "success",
  failed: false,
  result_status: null,
  duration_ms: 12,
  error: null,
  created_at: "2026-10-01T09:05:12+00:00",
  has_result: false
};

describe("runInvocationsList", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  it("GETs /api/customer-apps/{org}/{app}/invocations with no query by default", async () => {
    stubFetch(
      {
        "GET /api/customer-apps/acme/store/invocations": () => ({
          status: 200,
          body: { invocations: [ROW] }
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });

    await runInvocationsList(fakeContext({ bearer: "tok" }), "acme/store", { json: true });
    write.mockRestore();

    expect(JSON.parse(printed.trim())).toEqual({ invocations: [ROW] });
    const call = calls.find((c) => c.url.includes("/invocations"));
    expect(call?.url).toBe(`${TARGET}/api/customer-apps/acme/store/invocations`);
  });

  it("prints each row's id, function, environment and status without --json", async () => {
    stubFetch(
      {
        "GET /api/customer-apps/acme/store/invocations": () => ({
          status: 200,
          body: { invocations: [ROW] }
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });

    await runInvocationsList(fakeContext({ bearer: "tok" }), "acme/store", { json: false });
    write.mockRestore();

    // The id is what `invocations held` takes: a row that prints "?" for it
    // leaves the reader nothing to pass on.
    expect(printed.trim().split(/\s+/)).toEqual([ROW.id, "submit-order", "dev-a1", "success"]);
  });

  it("builds ?function=&environment=&build=&limit= in that order", async () => {
    stubFetch(
      {
        "GET /api/customer-apps/acme/store/invocations?function=submit-order&environment=dev-a1&build=k3x9&limit=10":
          () => ({ status: 200, body: { invocations: [] } })
      },
      calls
    );

    await runInvocationsList(fakeContext({ bearer: "tok" }), "acme/store", {
      appEnv: "dev-a1",
      build: "k3x9",
      fn: "submit-order",
      limit: 10,
      json: true
    });

    expect(
      calls.some(
        (c) =>
          c.url ===
          `${TARGET}/api/customer-apps/acme/store/invocations?function=submit-order&environment=dev-a1&build=k3x9&limit=10`
      )
    ).toBe(true);
  });

  it("refuses a publish token (exit 2)", async () => {
    stubFetch({}, calls);
    const err = await runInvocationsList(
      fakeContext({ bearer: "oxypublish_stored" }),
      "acme/store",
      { json: true }
    ).catch((e: unknown) => e as CliError);
    expect((err as CliError).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });

  // The console list and `{id}/invocations` refuse a caller inside an
  // assume-role session; the slug route is the `/logs` gate. A slug pair must
  // therefore reach it and nothing else.
  it("reads <org>/<app> with one request and never walks the registry", async () => {
    stubFetch(
      {
        "GET /api/customer-apps/acme/store/invocations": () => ({
          status: 200,
          body: { invocations: [] }
        })
      },
      calls
    );

    await runInvocationsList(fakeContext({ bearer: "tok" }), "acme/store", { json: true });

    expect(calls).toEqual([
      { method: "GET", url: `${TARGET}/api/customer-apps/acme/store/invocations` }
    ]);
  });

  // The slug route reads every environment for a caller with non-production
  // reach and production alone for one without, and says which.
  async function listWith(
    environments: string,
    appEnv?: string
  ): Promise<{ stdout: string; stderr: string }> {
    const query = appEnv === undefined ? "" : `?environment=${appEnv}`;
    stubFetch(
      {
        [`GET /api/customer-apps/acme/store/invocations${query}`]: () => ({
          status: 200,
          body: { invocations: [ROW], environments }
        })
      },
      calls
    );
    let stdout = "";
    let stderr = "";
    const out = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      stdout += String(chunk);
      return true;
    });
    const err = vi.spyOn(process.stderr, "write").mockImplementation((chunk) => {
      stderr += String(chunk);
      return true;
    });
    try {
      await runInvocationsList(fakeContext({ bearer: "tok" }), "acme/store", {
        appEnv,
        json: true
      });
    } finally {
      out.mockRestore();
      err.mockRestore();
    }
    return { stdout, stderr };
  }

  it("notes on stderr when a read naming no environment was limited to production", async () => {
    const { stdout, stderr } = await listWith("production");
    expect(stderr).toMatch(/production only.*non-production environments/);
    // stdout keeps its shape, with the added field.
    expect(JSON.parse(stdout.trim())).toEqual({ invocations: [ROW], environments: "production" });
  });

  it("says nothing when the read covered every environment", async () => {
    const { stdout, stderr } = await listWith("all");
    expect(stderr).toBe("");
    expect(JSON.parse(stdout.trim())).toEqual({ invocations: [ROW], environments: "all" });
  });

  it("says nothing when production was the environment asked for", async () => {
    const { stderr } = await listWith("production", "production");
    expect(stderr).toBe("");
  });

  it("keeps the console route by id for an app UUID", async () => {
    stubFetch(
      {
        [`GET /api/customer-apps/${APP_ID}`]: () => ({
          status: 200,
          body: { id: APP_ID, slug: "store", org_slug: "acme" }
        }),
        [`GET /api/customer-apps/${APP_ID}/invocations`]: () => ({
          status: 200,
          body: { invocations: [] }
        })
      },
      calls
    );

    await runInvocationsList(fakeContext({ bearer: "tok" }), APP_ID, { json: true });

    expect(calls.map((c) => c.url)).toEqual([
      `${TARGET}/api/customer-apps/${APP_ID}`,
      `${TARGET}/api/customer-apps/${APP_ID}/invocations`
    ]);
  });

  it("says what to do when the console refuses an assume session (a bare 403)", async () => {
    stubFetch(
      {
        // The console refuses the registry detail first, as it does every route.
        [`GET /api/customer-apps/${APP_ID}`]: () => ({ status: 403, body: undefined })
      },
      calls
    );
    const err = await runInvocationsList(fakeContext({ bearer: "tok" }), APP_ID, {
      json: true
    }).catch((e: unknown) => e as CliError);
    expect((err as CliError).code).toBe(ExitCode.AUTH);
    expect((err as CliError).hint).toMatch(/acting as a tenant.*or your staff grant/);
    expect((err as CliError).remedy).toMatch(
      /assume session.*<org-slug>\/<app-slug>.*oxyc assume end/
    );
  });

  it("maps 403 to exit 4", async () => {
    stubFetch(
      {
        "GET /api/customer-apps/acme/store/invocations": () => ({
          status: 403,
          body: { error: "non_production_refused", message: "nope" }
        })
      },
      calls
    );
    const err = await runInvocationsList(fakeContext({ bearer: "tok" }), "acme/store", {
      json: true
    }).catch((e: unknown) => e as CliError);
    expect((err as CliError).code).toBe(ExitCode.AUTH);
    // A refusal that says why is not the assume-session one.
    expect((err as CliError).remedy).toBeUndefined();
  });
});

describe("runInvocationsHeld", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  const HELD = {
    invocation_id: "inv-1",
    environment: "dev-a1",
    function: "submit-order",
    build_id: "k3x9",
    status: "success",
    held: [
      {
        op: "oltp.exec",
        plane: "oltp",
        namespace: "app_store",
        verb: "INSERT",
        table: "orders",
        statements: 1,
        rows: null,
        note: "no staging branch"
      }
    ]
  };

  it("GETs .../invocations/{id}/held and prints it verbatim with --json", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/customer-apps/${APP_ID}/invocations/inv-1/held`]: () => ({
          status: 200,
          body: HELD
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });

    await runInvocationsHeld(fakeContext({ bearer: "tok" }), "acme/store", "inv-1", { json: true });
    write.mockRestore();

    expect(JSON.parse(printed.trim())).toEqual(HELD);
  });

  async function printedHeld(held: unknown[]): Promise<string> {
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/customer-apps/${APP_ID}/invocations/inv-1/held`]: () => ({
          status: 200,
          body: { ...HELD, held }
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });
    await runInvocationsHeld(fakeContext({ bearer: "tok" }), "acme/store", "inv-1", {
      json: false
    });
    write.mockRestore();
    return printed;
  }

  it("prints a held call as before", async () => {
    expect(await printedHeld(HELD.held)).toBe("oltp.exec (oltp, orders) — no staging branch\n");
  });

  it("prints a sent provider-sandbox call with its path and status", async () => {
    const sent = {
      plane: "fetch",
      namespace: "api.stripe.com",
      verb: "POST",
      table: "",
      op: "fetch",
      statements: 1,
      rows: null,
      note: "sent: a registered provider sandbox",
      path: "/v1/charges",
      status: 200
    };
    expect(await printedHeld([sent])).toBe("fetch POST api.stripe.com/v1/charges → 200 (sent)\n");
    expect(await printedHeld([{ ...sent, status: undefined, statements: 3 }])).toBe(
      "fetch POST api.stripe.com/v1/charges → no response (sent) ×3\n"
    );
  });

  it("prints a held call whose path is null as held, not sent", async () => {
    const held = {
      plane: "fetch",
      namespace: "api.stripe.com",
      verb: "POST",
      table: "",
      op: "fetch",
      note: "the host is not in this environment's fetch allow-list",
      path: null,
      status: null
    };
    expect(await printedHeld([held])).toBe(
      "fetch (fetch) — the host is not in this environment's fetch allow-list\n"
    );
  });

  it("maps 404 invocation_not_found to exit 5", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/customer-apps/${APP_ID}/invocations/missing/held`]: () => ({
          status: 404,
          body: { error: "invocation_not_found", message: "no such invocation" }
        })
      },
      calls
    );
    const err = await runInvocationsHeld(fakeContext({ bearer: "tok" }), "acme/store", "missing", {
      json: true
    }).catch((e: unknown) => e as CliError);
    expect((err as CliError).code).toBe(ExitCode.NOT_FOUND);
  });
});
