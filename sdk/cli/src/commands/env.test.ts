/**
 * `oxyc env` — sandbox environments of a custom app, driven with
 * `globalThis.fetch` stubbed, as `checks.test.ts` does.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import {
  describeOltpBranch,
  describeOltpSchema,
  describePlanes,
  PLANE_ORDER,
  runEnvCreate,
  runEnvDelete,
  runEnvList,
  runEnvShow
} from "./env.js";

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

type RouteTable = Record<
  string,
  (init: RequestInit | undefined) => { status: number; body: unknown }
>;

interface Call {
  method: string;
  url: string;
  headers: Record<string, string>;
}

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
      const { status, body } = handler(init);
      return new Response(JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json" }
      });
    })
  );
}

/** A single-page admin-apps listing that resolves `acme/store` to APP_ID. */
function appsRoutes(): RouteTable {
  return {
    "GET /api/customer-apps?limit=100&offset=0": () => ({
      status: 200,
      body: { items: [{ id: APP_ID, slug: "store", org_slug: "acme" }], next_offset: null }
    })
  };
}

const ENVIRONMENT = {
  name: "dev-a1",
  kind: "dev",
  status: "active",
  build_id: null,
  build_uuid: null,
  semantic_revision_id: null,
  owner: { user_id: "u-1", email: "ana@oxy.tech" },
  created_at: "2026-10-01T09:00:00Z",
  updated_at: "2026-10-01T09:00:00Z",
  last_activity_at: "2026-10-01T09:00:00Z",
  expires_at: "2026-10-08T09:00:00Z",
  url: "https://dev-a1--acme--store.customer-apps-dev.oxygen-hq.com/"
};

describe("runEnvCreate", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  it("POSTs {name} to .../environments and prints the Environment with --json", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`POST /api/customer-apps/${APP_ID}/environments`]: () => ({
          status: 201,
          body: ENVIRONMENT
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });

    await runEnvCreate(fakeContext({ bearer: "tok" }), "acme/store", "dev-a1", { json: true });
    write.mockRestore();

    expect(JSON.parse(printed.trim())).toEqual(ENVIRONMENT);
    const post = calls.find((c) => c.method === "POST");
    expect(post?.url).toBe(`${TARGET}/api/customer-apps/${APP_ID}/environments`);
  });

  it.each([
    [null, "dev-a1 (dev) — active, no build yet"],
    ["b-7", "dev-a1 (dev) — active, build b-7"]
  ])("describes a sandbox whose build is %s in words that read", async (buildId, line) => {
    stubFetch(
      {
        ...appsRoutes(),
        [`POST /api/customer-apps/${APP_ID}/environments`]: () => ({
          status: 201,
          body: { ...ENVIRONMENT, build_id: buildId }
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });

    await runEnvCreate(fakeContext({ bearer: "tok" }), "acme/store", "dev-a1", { json: false });
    write.mockRestore();

    expect(printed.split("\n")[0]).toBe(line);
  });

  it("rejects a malformed name before any request (exit 2)", async () => {
    stubFetch({}, calls);
    const err = await runEnvCreate(fakeContext({ bearer: "tok" }), "acme/store", "not-a-sandbox", {
      json: true
    }).catch((e: unknown) => e as CliError);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });

  it("refuses production/staging as a name to create (exit 2)", async () => {
    stubFetch({}, calls);
    const err = await runEnvCreate(fakeContext({ bearer: "tok" }), "acme/store", "staging", {
      json: true
    }).catch((e: unknown) => e as CliError);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });

  it("refuses a publish token before any request (exit 2)", async () => {
    stubFetch({}, calls);
    const err = await runEnvCreate(
      fakeContext({ bearer: "oxypublish_stored" }),
      "acme/store",
      "dev-a1",
      { json: true }
    ).catch((e: unknown) => e as CliError);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });

  it("maps 409 environment_exists to exit 6", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`POST /api/customer-apps/${APP_ID}/environments`]: () => ({
          status: 409,
          body: { error: "environment_exists", message: "dev-a1 already exists" }
        })
      },
      calls
    );
    const err = await runEnvCreate(fakeContext({ bearer: "tok" }), "acme/store", "dev-a1", {
      json: true
    }).catch((e: unknown) => e as CliError);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.REQUEST);
    expect((err as CliError).detail).toContain("environment_exists");
  });

  it("maps 404 app_not_found to exit 5", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`POST /api/customer-apps/${APP_ID}/environments`]: () => ({
          status: 404,
          body: { error: "app_not_found", message: "no such app" }
        })
      },
      calls
    );
    const err = await runEnvCreate(fakeContext({ bearer: "tok" }), "acme/store", "dev-a1", {
      json: true
    }).catch((e: unknown) => e as CliError);
    expect((err as CliError).code).toBe(ExitCode.NOT_FOUND);
  });

  it("with no bearer and an API key, sends X-API-Key and no Authorization", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`POST /api/customer-apps/${APP_ID}/environments`]: () => ({
          status: 201,
          body: ENVIRONMENT
        })
      },
      calls
    );

    await runEnvCreate(fakeContext({ apiKey: "sekrit" }), "acme/store", "dev-a1", { json: true });

    expect(calls.length).toBeGreaterThan(0);
    for (const call of calls) {
      expect(call.headers["x-api-key"]).toBe("sekrit");
      expect(call.headers.authorization).toBeUndefined();
    }
  });
});

describe("runEnvList", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  it("GETs .../environments and prints {environments} with --json", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/customer-apps/${APP_ID}/environments`]: () => ({
          status: 200,
          body: { environments: [ENVIRONMENT] }
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });

    await runEnvList(fakeContext({ bearer: "tok" }), "acme/store", { json: true });
    write.mockRestore();

    expect(JSON.parse(printed.trim())).toEqual({ environments: [ENVIRONMENT] });
  });
});

describe("runEnvShow", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  it("GETs .../environments/{name} and prints the Environment with --json", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/customer-apps/${APP_ID}/environments/dev-a1`]: () => ({
          status: 200,
          body: ENVIRONMENT
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });

    await runEnvShow(fakeContext({ bearer: "tok" }), "acme/store", "dev-a1", { json: true });
    write.mockRestore();

    expect(JSON.parse(printed.trim())).toEqual(ENVIRONMENT);
  });

  it("prints a sandbox's oltp_schema verbatim with --json, and as an OLTP line without", async () => {
    const oltp_schema = {
      schema: "app_store__dev_a1",
      status: "ready",
      seeded_at: "2026-10-02T09:00:05Z",
      tables: 3,
      structure_only: ["audit_log"],
      error: null
    };
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/customer-apps/${APP_ID}/environments/dev-a1`]: () => ({
          status: 200,
          body: { ...ENVIRONMENT, oltp_schema }
        })
      },
      calls
    );
    for (const json of [true, false]) {
      let printed = "";
      const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
        printed += String(chunk);
        return true;
      });
      await runEnvShow(fakeContext({ bearer: "tok" }), "acme/store", "dev-a1", { json });
      write.mockRestore();
      if (json) expect(JSON.parse(printed.trim()).oltp_schema).toEqual(oltp_schema);
      else
        expect(printed).toContain(
          "OLTP app_store__dev_a1 — ready (3 tables from staging, copied empty (over the size cap): audit_log)"
        );
    }
  });

  it("prints a sandbox's oltp_branch verbatim with --json, and as an OLTP branch line without", async () => {
    const oltp_branch = {
      mode: "branch",
      status: "ready",
      fork_at: "2026-11-02T09:00:00Z",
      expires_at: "2026-12-02T09:00:00Z",
      error: null
    };
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/customer-apps/${APP_ID}/environments/dev-a1`]: () => ({
          status: 200,
          body: { ...ENVIRONMENT, oltp_branch }
        })
      },
      calls
    );
    for (const json of [true, false]) {
      let printed = "";
      const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
        printed += String(chunk);
        return true;
      });
      await runEnvShow(fakeContext({ bearer: "tok" }), "acme/store", "dev-a1", { json });
      write.mockRestore();
      if (json) expect(JSON.parse(printed.trim()).oltp_branch).toEqual(oltp_branch);
      else
        expect(printed).toContain(
          "OLTP branch (branch) — ready (forked 2026-11-02T09:00:00Z, expires 2026-12-02T09:00:00Z (the sandbox goes with it))"
        );
    }
  });

  it("passes parent, fork_at, planes and planes_error through --json unchanged", async () => {
    const branch = {
      parent: "staging",
      fork_at: "2026-10-02T09:00:00Z",
      planes: { build: "own", oltp: "schema" },
      planes_error: null
    };
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/customer-apps/${APP_ID}/environments/dev-a1`]: () => ({
          status: 200,
          body: { ...ENVIRONMENT, ...branch }
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });
    await runEnvShow(fakeContext({ bearer: "tok" }), "acme/store", "dev-a1", { json: true });
    write.mockRestore();
    const parsed = JSON.parse(printed.trim());
    expect(parsed.parent).toBe(branch.parent);
    expect(parsed.fork_at).toBe(branch.fork_at);
    expect(parsed.planes).toEqual(branch.planes);
    expect(parsed.planes_error).toBeNull();
  });

  it("prints the planes line after the URL without --json", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/customer-apps/${APP_ID}/environments/dev-a1`]: () => ({
          status: 200,
          body: { ...ENVIRONMENT, parent: "staging", planes: { build: "own", oltp: "schema" } }
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });
    await runEnvShow(fakeContext({ bearer: "tok" }), "acme/store", "dev-a1", { json: false });
    write.mockRestore();
    expect(printed).toContain("\n  from staging · build=own oltp=schema\n");
  });

  it("says what an OLTP branch is doing, and what to do when it failed", () => {
    const base = {
      mode: "branch",
      fork_at: "2026-11-02T09:00:00Z",
      expires_at: "2026-12-02T09:00:00Z",
      error: null
    };
    expect(describeOltpBranch({ ...base, status: "ready" })).toBe(
      "OLTP branch (branch) — ready (forked 2026-11-02T09:00:00Z, expires 2026-12-02T09:00:00Z (the sandbox goes with it))"
    );
    expect(describeOltpBranch({ ...base, mode: "empty", expires_at: null, status: "ready" })).toBe(
      "OLTP branch (empty) — ready (forked 2026-11-02T09:00:00Z, no expiry)"
    );
    for (const status of ["cutting", "migrating", "resetting"]) {
      expect(describeOltpBranch({ ...base, status })).toBe(
        `OLTP branch (branch) — ${status} (forked 2026-11-02T09:00:00Z); ctx.oltp is refused until it is ready`
      );
    }
    expect(describeOltpBranch({ ...base, status: "failed", error: "migration 0002 failed" })).toBe(
      "OLTP branch (branch) — failed: migration 0002 failed; oxyc env reset <app> <env> --plane oltp cuts it again"
    );
    expect(describeOltpBranch({ ...base, status: "failed" })).toContain("no reason recorded");
  });

  it("says what to do for a schema that failed or went stale", () => {
    const base = {
      schema: "app_store__dev_a1",
      seeded_at: null,
      tables: 0,
      structure_only: [],
      error: null
    };
    expect(describeOltpSchema({ ...base, status: "seeding" })).toBe(
      "OLTP app_store__dev_a1 — seeding"
    );
    expect(describeOltpSchema({ ...base, status: "stale" })).toContain(
      "was reset; publish to the sandbox again"
    );
    expect(describeOltpSchema({ ...base, status: "failed", error: "no such role" })).toBe(
      "OLTP app_store__dev_a1 — failed: no such role; publish to the sandbox again"
    );
    expect(describeOltpSchema({ ...base, status: "ready", tables: 1 })).toBe(
      "OLTP app_store__dev_a1 — ready (1 table from staging)"
    );
    expect(
      describeOltpSchema({
        ...base,
        status: "ready",
        tables: 1,
        staging_dependencies: ["column labels.state uses type app_store.status"]
      })
    ).toBe(
      "OLTP app_store__dev_a1 — ready (1 table from staging); still uses staging's column labels.state uses type app_store.status"
    );
    // The server's own reasons (`oltp_state::SEED_STALLED`, `BRANCH_RESET`):
    // it says why, the CLI says what to do — once.
    for (const why of [
      "the seed did not finish (its worker may have stopped)",
      "the org's OLTP staging branch was reset (or removed) since this schema was seeded"
    ]) {
      const line = describeOltpSchema({ ...base, status: "stale", error: why });
      expect(line).toBe(`OLTP app_store__dev_a1 — stale: ${why}; publish to the sandbox again`);
      expect(line.split("publish to the sandbox again")).toHaveLength(2);
    }
  });

  it("accepts production/staging too — any valid --app-env name", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/customer-apps/${APP_ID}/environments/staging`]: () => ({
          status: 200,
          body: { ...ENVIRONMENT, name: "staging", kind: "staging", owner: null }
        })
      },
      calls
    );
    await expect(
      runEnvShow(fakeContext({ bearer: "tok" }), "acme/store", "staging", { json: true })
    ).resolves.toBeUndefined();
  });

  it("maps 404 environment_not_found to exit 5", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`GET /api/customer-apps/${APP_ID}/environments/dev-gone`]: () => ({
          status: 404,
          body: { error: "environment_not_found", message: "no such environment" }
        })
      },
      calls
    );
    const err = await runEnvShow(fakeContext({ bearer: "tok" }), "acme/store", "dev-gone", {
      json: true
    }).catch((e: unknown) => e as CliError);
    expect((err as CliError).code).toBe(ExitCode.NOT_FOUND);
  });
});

describe("runEnvDelete", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  it("refuses without --yes off a terminal (exit 8), making no DELETE request", async () => {
    stubFetch({ ...appsRoutes() }, calls);
    const err = await runEnvDelete(fakeContext({ bearer: "tok" }), "acme/store", "dev-a1", {
      json: true
    }).catch((e: unknown) => e as CliError);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.REFUSED);
    expect(calls.some((c) => c.method === "DELETE")).toBe(false);
  });

  it("with --yes, DELETEs and prints {name,status:deleting,teardown_run_id} without --wait", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`DELETE /api/customer-apps/${APP_ID}/environments/dev-a1`]: () => ({
          status: 202,
          body: { name: "dev-a1", status: "deleting", teardown_run_id: "run-9" }
        })
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });

    await runEnvDelete(fakeContext({ bearer: "tok" }), "acme/store", "dev-a1", {
      yes: true,
      json: true
    });
    write.mockRestore();

    expect(JSON.parse(printed.trim())).toEqual({
      name: "dev-a1",
      status: "deleting",
      teardown_run_id: "run-9"
    });
  });

  it("--wait polls GET until 404, then prints {name,status:deleted}", async () => {
    let polls = 0;
    stubFetch(
      {
        ...appsRoutes(),
        [`DELETE /api/customer-apps/${APP_ID}/environments/dev-a1`]: () => ({
          status: 202,
          body: { name: "dev-a1", status: "deleting", teardown_run_id: "run-9" }
        }),
        [`GET /api/customer-apps/${APP_ID}/environments/dev-a1`]: () => {
          polls += 1;
          return polls < 3
            ? { status: 200, body: { ...ENVIRONMENT, name: "dev-a1", status: "deleting" } }
            : { status: 404, body: { error: "environment_not_found", message: "gone" } };
        }
      },
      calls
    );
    let printed = "";
    const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      printed += String(chunk);
      return true;
    });

    await runEnvDelete(fakeContext({ bearer: "tok" }), "acme/store", "dev-a1", {
      yes: true,
      json: true,
      waitSeconds: 10,
      pollMs: 0
    });
    write.mockRestore();

    expect(JSON.parse(printed.trim())).toEqual({ name: "dev-a1", status: "deleted" });
    expect(polls).toBeGreaterThanOrEqual(3);
  });

  it("past the --wait deadline, throws exit 7 (UNAVAILABLE)", async () => {
    stubFetch(
      {
        ...appsRoutes(),
        [`DELETE /api/customer-apps/${APP_ID}/environments/dev-a1`]: () => ({
          status: 202,
          body: { name: "dev-a1", status: "deleting", teardown_run_id: "run-9" }
        }),
        [`GET /api/customer-apps/${APP_ID}/environments/dev-a1`]: () => ({
          status: 200,
          body: { ...ENVIRONMENT, name: "dev-a1", status: "deleting" }
        })
      },
      calls
    );

    const err = await runEnvDelete(fakeContext({ bearer: "tok" }), "acme/store", "dev-a1", {
      yes: true,
      json: true,
      waitSeconds: 0.001,
      pollMs: 0
    }).catch((e: unknown) => e as CliError);

    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.UNAVAILABLE);
  });

  it("refuses production/staging as a name to delete (exit 2)", async () => {
    stubFetch({}, calls);
    const err = await runEnvDelete(fakeContext({ bearer: "tok" }), "acme/store", "production", {
      yes: true,
      json: true
    }).catch((e: unknown) => e as CliError);
    expect((err as CliError).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });
});

describe("describePlanes", () => {
  it("is null for an older server that sends none of the fields", () => {
    expect(describePlanes({})).toBeNull();
  });

  it("is null for production, where all four are null", () => {
    expect(
      describePlanes({ parent: null, fork_at: null, planes: null, planes_error: null })
    ).toBeNull();
  });

  it("names the parent, then each plane's mode in the fixed order, whatever order the server sent", () => {
    const line = describePlanes({
      parent: "staging",
      fork_at: "2026-10-02T09:00:00Z",
      // Alphabetical, as a BTreeMap serialises.
      planes: {
        airhouse: "write-only",
        build: "own",
        fetch: "held-writes",
        messages: "invoker",
        oltp: "schema",
        reads: "live",
        schedules: "switch",
        secrets: "inherit",
        semantic: "build-pin",
        storage: "branch",
        warehouse: "mapped",
        "workspace-actions": "refused"
      },
      planes_error: null
    });
    expect(line).toBe(
      "from staging · build=own semantic=build-pin reads=live oltp=schema airhouse=write-only " +
        "warehouse=mapped storage=branch secrets=inherit messages=invoker fetch=held-writes " +
        "schedules=switch workspace-actions=refused"
    );
  });

  it("renders the exact payload the server sends for a dev-a1 sandbox", () => {
    const sandbox = {
      parent: "staging",
      fork_at: "2026-11-02T08:15:00Z",
      planes: {
        airhouse: "write-only",
        build: "own",
        fetch: "held-writes",
        messages: "invoker",
        oltp: "schema",
        reads: "live",
        schedules: "switch",
        secrets: "inherit",
        semantic: "build-pin",
        storage: "branch",
        warehouse: "mapped",
        "workspace-actions": "refused"
      },
      planes_error: null
    };
    expect(Object.keys(sandbox.planes)).toEqual([...Object.keys(sandbox.planes)].sort());
    expect(describePlanes(sandbox)).toBe(
      "from staging · build=own semantic=build-pin reads=live oltp=schema airhouse=write-only " +
        "warehouse=mapped storage=branch secrets=inherit messages=invoker fetch=held-writes " +
        "schedules=switch workspace-actions=refused"
    );
  });

  it("puts a plane the CLI does not know after the known ones, alphabetically", () => {
    const line = describePlanes({
      parent: "staging",
      planes: { zeta: "x", build: "own", alpha: "y", "workspace-actions": "refused" }
    });
    expect(line).toBe("from staging · build=own workspace-actions=refused alpha=y zeta=x");
    expect(PLANE_ORDER[0]).toBe("build");
    expect(PLANE_ORDER[PLANE_ORDER.length - 1]).toBe("workspace-actions");
  });

  it("omits the from-prefix rather than print null when there is no parent", () => {
    expect(describePlanes({ parent: null, planes: { build: "own" } })).toBe("build=own");
    expect(describePlanes({ planes: { build: "own" } })).toBe("build=own");
  });

  it("prints only the parent for an empty planes object, and nothing without one", () => {
    expect(describePlanes({ parent: "staging", planes: {}, planes_error: null })).toBe(
      "from staging"
    );
    expect(describePlanes({ parent: null, planes: {}, planes_error: null })).toBeNull();
    expect(describePlanes({ planes: {} })).toBeNull();
  });

  it("says the planes are unreadable, and why, when the server could not read them", () => {
    expect(
      describePlanes({
        parent: "staging",
        fork_at: null,
        planes: null,
        planes_error: "unknown plane olpt"
      })
    ).toBe("from staging · planes: unreadable — unknown plane olpt");
  });

  it("says the planes are unreadable without a parent prefix when there is no parent", () => {
    expect(
      describePlanes({
        parent: null,
        fork_at: null,
        planes: null,
        planes_error: "unknown plane olpt"
      })
    ).toBe("planes: unreadable — unknown plane olpt");
  });
});
