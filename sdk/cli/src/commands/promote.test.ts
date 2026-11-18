/**
 * `oxyc promote` / `oxyc rollback`, driven with `globalThis.fetch` stubbed, as
 * `env.test.ts` does: the plan first, then the act sending the plan's build
 * and `expect_current` back — or a refusal before any act is sent.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import { type Plan, planLines, runPromote, runRollback } from "./promote.js";

const TARGET = "https://oxy.test";
const APP_ID = "a1a1a1a1-2222-3333-4444-555555555555";
const BUILD = "b0b0b0b0-2222-3333-4444-555555555555";
const LIVE = "c0c0c0c0-2222-3333-4444-555555555555";

function fakeContext(bearer = "tok"): Context {
  return {
    cwd: "/tmp",
    flags: { env: "production", apiKeyEnv: "OXY_API_KEY" },
    target: () => TARGET,
    env: () => ({ target: TARGET, orgSlug: undefined }) as ReturnType<Context["env"]>,
    bearer: async () => bearer,
    maybeBearer: async () => bearer,
    storedBearer: () => bearer,
    async credential() {
      return { token: bearer, source: "env" };
    },
    serviceAccount: () => undefined,
    apiKey: () => undefined,
    customer: () => undefined,
    repoDir: () => undefined,
    placeholders: () => ({}),
    withEnv: () => fakeContext(bearer)
  };
}

interface Call {
  method: string;
  path: string;
  body: Record<string, unknown> | undefined;
}

type Handler = (body: Record<string, unknown> | undefined) => { status: number; body: unknown };

function stubFetch(routes: Record<string, Handler>, calls: Call[]) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string | URL, init?: RequestInit) => {
      const path = String(input).replace(TARGET, "");
      const method = (init?.method ?? "GET").toUpperCase();
      const body = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
      calls.push({ method, path, body });
      const handler = routes[`${method} ${path}`];
      if (!handler) return new Response(JSON.stringify({ error: "no stub" }), { status: 404 });
      const answer = handler(body);
      return new Response(JSON.stringify(answer.body), {
        status: answer.status,
        headers: { "content-type": "application/json" }
      });
    })
  );
}

const APPS = {
  "GET /api/customer-apps?limit=100&offset=0": () => ({
    status: 200,
    body: { items: [{ id: APP_ID, slug: "store", org_slug: "acme" }], next_offset: null }
  })
};

function plan(overrides: Partial<Plan> = {}): Plan {
  return {
    action: "promote",
    app_id: APP_ID,
    from: "staging",
    to: "production",
    build: {
      id: BUILD,
      build_id: "b-7",
      commit_sha: "0123456789abcdef",
      source_branch: "main",
      source_repo: "https://github.com/acme/store",
      published_by: "ana@oxy.tech",
      created_at: "2026-10-16T09:00:00Z",
      published_to: "staging"
    },
    current: { id: LIVE, build_id: "b-6" },
    expect_current: LIVE,
    served_since: "2026-10-16T09:00:00Z",
    served_for_seconds: 7_200,
    functions: { added: ["fresh"], removed: [], changed: ["whoami"] },
    schedules: {
      added: [],
      removed: [{ function: "nightly", cron: "0 0 * * *", timezone: "UTC" }],
      changed: []
    },
    migrations: { when: "before_move", oltp: ["0002_orders.sql"], airhouse: [] },
    required_secrets_unset: [],
    blockers: [],
    warnings: [],
    dry_run: true,
    moved: false,
    migrations_applied: null,
    ...overrides
  };
}

function capture(): { printed: () => string; restore: () => void } {
  let printed = "";
  const out = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
    printed += String(chunk);
    return true;
  });
  const err = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
  return {
    printed: () => printed,
    restore: () => {
      out.mockRestore();
      err.mockRestore();
    }
  };
}

const PROMOTE = `POST /api/customer-apps/${APP_ID}/environments/production/promote`;

describe("oxyc promote", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  it("--dry-run asks for the plan, prints it, and acts on nothing", async () => {
    stubFetch({ ...APPS, [PROMOTE]: () => ({ status: 200, body: plan() }) }, calls);
    const io = capture();
    await runPromote(fakeContext(), "acme/store", { dryRun: true, json: true });
    io.restore();
    const posts = calls.filter((c) => c.method === "POST");
    expect(posts).toHaveLength(1);
    expect(posts[0]?.body).toEqual({
      from: "staging",
      confirm_schedule_removals: [],
      dry_run: true
    });
    expect(JSON.parse(io.printed()).build.build_id).toBe("b-7");
  });

  it("--yes acts with the plan's build and expect_current", async () => {
    stubFetch(
      {
        ...APPS,
        [PROMOTE]: (body) =>
          body?.dry_run
            ? { status: 200, body: plan() }
            : { status: 200, body: plan({ dry_run: false, moved: true }) }
      },
      calls
    );
    const io = capture();
    await runPromote(fakeContext(), "acme/store", { yes: true, removeSchedules: ["nightly"] });
    io.restore();
    const posts = calls.filter((c) => c.method === "POST");
    expect(posts).toHaveLength(2);
    expect(posts[1]?.body).toEqual({
      from: "staging",
      confirm_schedule_removals: ["nightly"],
      build: BUILD,
      expect_current: LIVE
    });
    expect(io.printed()).toContain("production now serves build b-7");
  });

  it("off a terminal without --yes, exits 8 after the plan and sends no act", async () => {
    stubFetch({ ...APPS, [PROMOTE]: () => ({ status: 200, body: plan() }) }, calls);
    const io = capture();
    const err = await runPromote(fakeContext(), "acme/store", {}).catch((e: unknown) => e);
    io.restore();
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.REFUSED);
    expect(calls.filter((c) => c.method === "POST")).toHaveLength(1);
  });

  it("refuses a plan with blockers before any act (exit 6)", async () => {
    const blocked = plan({
      blockers: [{ code: "staging_never_served", message: "staging has never served build b-7" }]
    });
    stubFetch({ ...APPS, [PROMOTE]: () => ({ status: 200, body: blocked }) }, calls);
    const io = capture();
    const err = await runPromote(fakeContext(), "acme/store", { yes: true }).catch(
      (e: unknown) => e
    );
    io.restore();
    expect((err as CliError).code).toBe(ExitCode.REQUEST);
    expect((err as CliError).message).toContain("staging_never_served");
    expect(calls.filter((c) => c.method === "POST")).toHaveLength(1);
  });

  it("--break-listeners sends breakListeners on the plan and on the act", async () => {
    stubFetch(
      {
        ...APPS,
        [PROMOTE]: (body) =>
          body?.dry_run
            ? { status: 200, body: plan() }
            : { status: 200, body: plan({ dry_run: false, moved: true }) }
      },
      calls
    );
    const io = capture();
    await runPromote(fakeContext(), "acme/store", { yes: true, breakListeners: true });
    io.restore();
    const posts = calls.filter((c) => c.method === "POST");
    expect(posts).toHaveLength(2);
    for (const post of posts) expect(post.body).toMatchObject({ breakListeners: true });
  });

  it("hints --break-listeners on an event_listeners_broken blocker", async () => {
    const blocked = plan({
      blockers: [
        { code: "event_listeners_broken", message: "`a/b.c` is removed but x/fn listens to it" }
      ]
    });
    stubFetch({ ...APPS, [PROMOTE]: () => ({ status: 200, body: blocked }) }, calls);
    const io = capture();
    const err = await runPromote(fakeContext(), "acme/store", { yes: true }).catch(
      (e: unknown) => e
    );
    io.restore();
    expect((err as CliError).hint).toContain("--break-listeners");
  });

  it("hints --break-listeners when listeners are any blocker, not only the first", async () => {
    const blocked = plan({
      blockers: [
        { code: "staging_never_served", message: "staging has never served build b-7" },
        { code: "event_listeners_broken", message: "`a/b.c` is removed but x/fn listens to it" }
      ]
    });
    stubFetch({ ...APPS, [PROMOTE]: () => ({ status: 200, body: blocked }) }, calls);
    const io = capture();
    const err = await runPromote(fakeContext(), "acme/store", { yes: true }).catch(
      (e: unknown) => e
    );
    io.restore();
    expect((err as CliError).message).toContain("staging_never_served, event_listeners_broken");
    expect((err as CliError).hint).toContain("--break-listeners");
  });

  it("promotes a sandbox to staging, and refuses production as a source (exit 2)", async () => {
    const toStaging = `POST /api/customer-apps/${APP_ID}/environments/staging/promote`;
    stubFetch(
      { ...APPS, [toStaging]: () => ({ status: 200, body: plan({ to: "staging" }) }) },
      calls
    );
    const io = capture();
    await runPromote(fakeContext(), "acme/store", { from: "dev-a1", dryRun: true, json: true });
    const err = await runPromote(fakeContext(), "acme/store", { from: "production" }).catch(
      (e: unknown) => e
    );
    io.restore();
    expect(calls.find((c) => c.method === "POST")?.body?.from).toBe("dev-a1");
    expect((err as CliError).code).toBe(ExitCode.USAGE);
  });

  it("refuses a sandbox agent token before any request (exit 2)", async () => {
    stubFetch({}, calls);
    const err = await runPromote(fakeContext("oxy_sbx_abc"), "acme/store", {}).catch(
      (e: unknown) => e
    );
    expect((err as CliError).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });

  it("answers the server's target_moved with its code (exit 6)", async () => {
    stubFetch(
      {
        ...APPS,
        [PROMOTE]: (body) =>
          body?.dry_run
            ? { status: 200, body: plan() }
            : {
                status: 409,
                body: { code: "target_moved", error: "target_moved", message: "production moved" }
              }
      },
      calls
    );
    const io = capture();
    const err = await runPromote(fakeContext(), "acme/store", { yes: true }).catch(
      (e: unknown) => e
    );
    io.restore();
    expect((err as CliError).code).toBe(ExitCode.REQUEST);
    expect((err as CliError).serverCode).toBe("target_moved");
  });
});

describe("oxyc rollback", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  it("needs --app-env, and never a sandbox (exit 2)", async () => {
    stubFetch({}, calls);
    for (const appEnv of [undefined, "dev-a1", "prod"]) {
      const err = await runRollback(fakeContext(), "acme/store", { appEnv }).catch(
        (e: unknown) => e
      );
      expect((err as CliError).code).toBe(ExitCode.USAGE);
    }
    expect(calls).toHaveLength(0);
  });

  it("plans with --to, then pins the plan's build as `to`", async () => {
    const route = `POST /api/customer-apps/${APP_ID}/environments/production/rollback`;
    const rolled = plan({ action: "rollback", from: null });
    stubFetch({ ...APPS, [route]: () => ({ status: 200, body: rolled }) }, calls);
    const io = capture();
    await runRollback(fakeContext(), "acme/store", {
      appEnv: "production",
      build: "b-7",
      yes: true
    });
    io.restore();
    const posts = calls.filter((c) => c.method === "POST");
    expect(posts[0]?.body).toEqual({ confirm_schedule_removals: [], to: "b-7", dry_run: true });
    expect(posts[1]?.body).toEqual({
      confirm_schedule_removals: [],
      to: BUILD,
      expect_current: LIVE
    });
  });
});

describe("the plan, in words", () => {
  it("names the build, what changes and what production applies first", () => {
    const lines = planLines(plan()).join("\n");
    expect(lines).toContain("promote staging → production: build b-7");
    expect(lines).toContain("production serves b-6 now");
    expect(lines).toContain("staging has served it for 2h 0m");
    expect(lines).toContain("functions: +fresh ~whoami");
    expect(lines).toContain("schedules: -nightly");
    expect(lines).toContain("applied before the move): OLTP 0002_orders.sql");
  });
});
