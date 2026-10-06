/**
 * `oxyc apps list | show | builds | health | usage`, driven with
 * `globalThis.fetch` stubbed by a route table, as `checks.test.ts` does.
 *
 * What is pinned is the part a reader of the output cannot check for
 * themselves: that a paged listing is walked to its end, that a request which
 * failed never prints as an empty result, and that the exit code separates the
 * two.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import {
  BUILDS_CONCURRENCY,
  runAppsBuilds,
  runAppsHealth,
  runAppsList,
  runAppsShow,
  runAppsUsage
} from "./apps.js";

const TARGET = "https://oxy.test";

function fakeContext(): Context {
  return {
    cwd: "/tmp",
    flags: { env: "production", tokenEnv: "OXY_TOKEN", apiKeyEnv: "OXY_API_KEY" },
    target: () => TARGET,
    env: () => ({ target: TARGET, orgSlug: undefined }) as ReturnType<Context["env"]>,
    bearer: async () => "tok",
    maybeBearer: async () => "tok",
    storedBearer: () => "tok",
    async credential() {
      return { token: await this.bearer(), source: "env" };
    },
    serviceAccount: () => undefined,
    apiKey: () => undefined,
    customer: () => undefined,
    repoDir: () => undefined,
    placeholders: () => ({}),
    withEnv: () => fakeContext()
  };
}

/** One GET handler per path (query string included). A missing path answers 404. */
interface Reply {
  status: number;
  body: unknown;
}
type Routes = Record<string, () => Reply | Promise<Reply>>;

function stubFetch(routes: Routes, calls: string[]): void {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string | URL, init?: RequestInit) => {
      const path = String(input).replace(TARGET, "");
      calls.push(`${(init?.method ?? "GET").toUpperCase()} ${path}`);
      const handler = routes[path];
      if (!handler) return new Response(JSON.stringify({ error: "no stub" }), { status: 404 });
      const { status, body } = await handler();
      return new Response(typeof body === "string" ? body : JSON.stringify(body), { status });
    })
  );
}

/** Collect what a command writes, per stream. */
function capture(): { stdout: () => string; stderr: () => string } {
  const chunks = { out: [] as string[], err: [] as string[] };
  vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
    chunks.out.push(String(chunk));
    return true;
  });
  vi.spyOn(process.stderr, "write").mockImplementation((chunk) => {
    chunks.err.push(String(chunk));
    return true;
  });
  return { stdout: () => chunks.out.join(""), stderr: () => chunks.err.join("") };
}

/** The `CliError` a command rejects with. */
async function failure(run: Promise<void>): Promise<CliError> {
  const cause = await run.then(
    () => undefined,
    (e: unknown) => e
  );
  expect(cause).toBeInstanceOf(CliError);
  return cause as CliError;
}

const LIST = "/api/customer-apps";
const ID_A = "aaaaaaaa-1111-4111-8111-111111111111";
const ID_B = "bbbbbbbb-2222-4222-8222-222222222222";
const ID_C = "cccccccc-3333-4333-8333-333333333333";

function app(id: string, org: string, slug: string, publishedAt: string | null) {
  return {
    id,
    slug,
    name: slug,
    org_id: "org",
    org_slug: org,
    project_id: "project",
    branch: "main",
    source_repo: "oxy-hq/customer-apps",
    status: "created",
    url: `${TARGET}/customer-apps/${org}/${slug}/`,
    published_at: publishedAt,
    created_at: "2026-09-01T00:00:00+00:00",
    updated_at: "2026-09-01T00:00:00+00:00"
  };
}

const LIVE = app(ID_A, "acme", "store", "2026-09-10T08:00:00+00:00");
const DRAFT = app(ID_B, "acme", "draft-only", null);
const OTHER = app(ID_C, "globex", "portal", "2026-09-12T08:00:00+00:00");

/** The listing split over two pages, with `LIVE` repeated on the second. */
function twoPages(): Routes {
  return {
    [`${LIST}?limit=100&offset=0`]: () => ({
      status: 200,
      body: { items: [LIVE, DRAFT], next_offset: 100 }
    }),
    // `LIVE` again: the listing is ordered by `updated_at`, so a row can cross
    // the page boundary between two requests.
    [`${LIST}?limit=100&offset=100`]: () => ({
      status: 200,
      body: { items: [LIVE, OTHER], next_offset: null }
    })
  };
}

function build(over: Record<string, unknown>) {
  return {
    id: "build-row",
    build_id: "build-1",
    created_at: "2026-09-10T08:00:00+00:00",
    is_draft: false,
    is_published: false,
    published_by_email: "ada@oxy.test",
    published_via: null,
    source_repo: "https://github.com/oxy-hq/customer-apps.git",
    commit_sha: "0123456789abcdef0123456789abcdef01234567",
    source_branch: "main",
    ...over
  };
}

const HEALTH_PASS = {
  oxy_app_health: "pass",
  app: { id: ID_A, org_slug: "acme", slug: "store" },
  build: { build_id: "build-1", published_at: "2026-09-10T08:00:00+00:00" },
  checks: [
    { name: "registered", result: "pass" },
    { name: "published", result: "pass" }
  ],
  checked_at: "2026-10-02T00:00:00+00:00"
};

const AVAILABILITY = {
  verdict: "healthy",
  objective: 0.99,
  windows: [
    { window_minutes: 5, total: 0, failed: 0, failure_ratio: null },
    { window_minutes: 1440, total: 40, failed: 1, failure_ratio: 0.025 }
  ]
};

const USAGE = {
  total_views_7d: 9,
  unique_users_7d: 4,
  total_events_7d: 28,
  last_viewed_at: "2026-10-01T22:21:05Z"
};

let calls: string[];
beforeEach(() => {
  calls = [];
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("apps list", () => {
  it("walks next_offset to the last page and lists each app once", async () => {
    stubFetch(twoPages(), calls);
    const io = capture();

    await runAppsList(fakeContext(), { json: true });

    expect(calls).toEqual([`GET ${LIST}?limit=100&offset=0`, `GET ${LIST}?limit=100&offset=100`]);
    const rows = JSON.parse(io.stdout()) as Array<{ id: string }>;
    // Sorted by <org>/<app>, and `LIVE` — served on both pages — appears once.
    expect(rows.map((row) => row.id)).toEqual([ID_B, ID_A, ID_C]);
  });

  it("filters by organization and by published state", async () => {
    stubFetch(twoPages(), calls);
    const io = capture();
    const ids = async (flags: Parameters<typeof runAppsList>[1]) => {
      const before = io.stdout().length;
      await runAppsList(fakeContext(), { ...flags, json: true });
      return (JSON.parse(io.stdout().slice(before)) as Array<{ id: string }>).map((r) => r.id);
    };

    expect(await ids({ org: "acme" })).toEqual([ID_B, ID_A]);
    expect(await ids({ published: true })).toEqual([ID_A, ID_C]);
    expect(await ids({ draft: true })).toEqual([ID_B]);
  });

  it("refuses --published with --draft before any request", async () => {
    stubFetch(twoPages(), calls);
    const error = await failure(runAppsList(fakeContext(), { published: true, draft: true }));
    expect(error.code).toBe(ExitCode.USAGE);
    expect(calls).toEqual([]);
  });

  it("refuses a publish token before any request, with a hint that fits this command", async () => {
    // A publish token may not list apps. The refusal comes from `staffCreds`,
    // which the sandbox commands share, so its hint must not name only them.
    stubFetch(twoPages(), calls);
    const withPublishToken: Context = {
      ...fakeContext(),
      bearer: async () => "oxypublish_abc",
      maybeBearer: async () => "oxypublish_abc",
      storedBearer: () => "oxypublish_abc"
    };
    const error = await failure(runAppsList(withPublishToken, {}));
    expect(error.code).toBe(ExitCode.USAGE);
    expect(error.message).toBe("a publish token cannot use this command");
    expect(error.hint).toContain("staff credential");
    expect(error.hint).not.toContain("sandbox");
    expect(calls).toEqual([]);
  });

  it("renders the table with the state and the registered source", async () => {
    stubFetch(twoPages(), calls);
    const io = capture();

    await runAppsList(fakeContext(), {});

    expect(io.stdout()).toContain("| APP | NAME | STATE | PUBLISHED | LAST ACTIVE | SOURCE |");
    expect(io.stdout()).toContain(
      "| acme/store | store | live | 2026-09-10 08:00 | — | oxy-hq/customer-apps |"
    );
    expect(io.stdout()).toContain("| acme/draft-only | draft-only | draft | — |");
    expect(io.stderr()).toContain("3 app(s): 2 live, 1 draft");
  });

  /**
   * AN EMPTY RESULT AND A FAILED REQUEST ARE DIFFERENT ANSWERS. Empty resolves
   * (exit 0) with nothing on stdout and a sentence on stderr; a failure rejects
   * with the exit code its status maps to.
   */
  it("says so, and exits 0, when the listing is empty", async () => {
    stubFetch(
      {
        [`${LIST}?limit=100&offset=0`]: () => ({
          status: 200,
          body: { items: [], next_offset: null }
        })
      },
      calls
    );
    const io = capture();

    await runAppsList(fakeContext(), {});

    expect(io.stdout()).toBe("");
    expect(io.stderr()).toContain("no custom apps are visible to you");
  });

  it("rejects with the status's exit code when the listing request fails", async () => {
    stubFetch({ [`${LIST}?limit=100&offset=0`]: () => ({ status: 503, body: "down" }) }, calls);
    const io = capture();

    const error = await failure(runAppsList(fakeContext(), { json: true }));

    expect(error.code).toBe(ExitCode.UNAVAILABLE);
    // Nothing that could be read as "there are no apps".
    expect(io.stdout()).toBe("");
  });

  it("rejects when a later page fails, instead of listing only the first", async () => {
    const routes = twoPages();
    routes[`${LIST}?limit=100&offset=100`] = () => ({ status: 500, body: "boom" });
    stubFetch(routes, calls);
    const io = capture();

    const error = await failure(runAppsList(fakeContext(), {}));

    expect(error.code).toBe(ExitCode.UNAVAILABLE);
    expect(io.stdout()).toBe("");
  });

  /**
   * A deployment that answers an unknown path with its HTML page and a 200
   * would otherwise list as "no apps".
   */
  it("rejects a 200 that is not an app listing", async () => {
    stubFetch(
      { [`${LIST}?limit=100&offset=0`]: () => ({ status: 200, body: "<!doctype html><html>" }) },
      calls
    );
    capture();

    const error = await failure(runAppsList(fakeContext(), {}));

    expect(error.code).toBe(ExitCode.FAILURE);
    expect(error.message).toContain("did not return JSON");
  });

  it("--builds adds the live build of each app, one request per app", async () => {
    stubFetch(
      {
        ...twoPages(),
        [`${LIST}/${ID_A}/builds`]: () => ({
          status: 200,
          body: {
            builds: [
              build({ is_draft: true }),
              build({ id: "live", build_id: "build-live", is_published: true })
            ],
            promoted_at: null,
            promoted_by_email: null
          }
        }),
        [`${LIST}/${ID_C}/builds`]: () => ({
          status: 200,
          body: { builds: [], promoted_at: null, promoted_by_email: null }
        })
      },
      calls
    );
    const io = capture();

    await runAppsList(fakeContext(), { published: true, builds: true, json: true });

    const rows = JSON.parse(io.stdout()) as Array<{
      id: string;
      live_build: { build_id: string } | null;
    }>;
    expect(rows.find((row) => row.id === ID_A)?.live_build?.build_id).toBe("build-live");
    // Published, but its history holds no live build: `null`, not an error.
    expect(rows.find((row) => row.id === ID_C)?.live_build).toBeNull();
    // The draft app was filtered out before the fan-out, so it cost no request.
    expect(calls).not.toContain(`GET ${LIST}/${ID_B}/builds`);
  });

  it("--builds marks a failed builds request NOT READ and exits non-zero", async () => {
    stubFetch(
      {
        ...twoPages(),
        [`${LIST}/${ID_A}/builds`]: () => ({
          status: 200,
          body: {
            builds: [build({ is_published: true })],
            promoted_at: null,
            promoted_by_email: null
          }
        }),
        [`${LIST}/${ID_C}/builds`]: () => ({ status: 502, body: { error: "upstream" } })
      },
      calls
    );
    const io = capture();

    const error = await failure(runAppsList(fakeContext(), { published: true, builds: true }));

    expect(error.code).toBe(ExitCode.UNAVAILABLE);
    expect(error.message).toBe("1 of 2 builds request(s) failed");
    // The table still printed: the row that was read, and the one that was not.
    expect(io.stdout()).toMatch(/\| acme\/store \|.*\| build-1 \|/);
    expect(io.stdout()).toMatch(/\| globex\/portal \|.*\| NOT READ \| NOT READ \|/);
  });

  it("--builds keeps at most BUILDS_CONCURRENCY requests in flight", async () => {
    const many = Array.from({ length: 20 }, (_unused, i) =>
      app(
        `${String(i).padStart(8, "0")}-0000-4000-8000-000000000000`,
        "acme",
        `app-${i}`,
        "2026-09-01T00:00:00+00:00"
      )
    );
    let inFlight = 0;
    let peak = 0;
    const routes: Routes = {
      [`${LIST}?limit=100&offset=0`]: () => ({
        status: 200,
        body: { items: many, next_offset: null }
      })
    };
    for (const row of many) {
      routes[`${LIST}/${row.id}/builds`] = async () => {
        inFlight += 1;
        peak = Math.max(peak, inFlight);
        await new Promise((resolve) => setTimeout(resolve, 2));
        inFlight -= 1;
        return { status: 200, body: { builds: [], promoted_at: null, promoted_by_email: null } };
      };
    }
    stubFetch(routes, calls);
    capture();

    await runAppsList(fakeContext(), { builds: true, json: true });

    expect(calls.filter((call) => call.endsWith("/builds"))).toHaveLength(20);
    expect(peak).toBeLessThanOrEqual(BUILDS_CONCURRENCY);
    expect(peak).toBeGreaterThan(1);
  });
});

describe("resolving <org>/<app>", () => {
  const builds = {
    [`${LIST}/${ID_C}/builds`]: () => ({
      status: 200,
      body: {
        builds: [
          build({ id: "old", build_id: "build-old", created_at: "2026-09-01T00:00:00+00:00" }),
          build({
            id: "new",
            build_id: "build-new",
            created_at: "2026-09-12T00:00:00+00:00",
            is_published: true,
            is_draft: true
          })
        ],
        promoted_at: "2026-09-12T00:01:00+00:00",
        promoted_by_email: "ada@oxy.test"
      }
    })
  };

  it("finds a slug on a later page of the listing", async () => {
    stubFetch({ ...twoPages(), ...builds }, calls);
    const io = capture();

    await runAppsBuilds(fakeContext(), "globex/portal", false);

    expect(calls).toEqual([
      `GET ${LIST}?limit=100&offset=0`,
      `GET ${LIST}?limit=100&offset=100`,
      `GET ${LIST}/${ID_C}/builds`
    ]);
    // Newest first whatever order the server sent, with the channel marked.
    const lines = io.stdout().split("\n");
    expect(lines[2]).toContain("| build-new | 2026-09-12 00:00 | live, draft |");
    expect(lines[3]).toContain("| build-old | 2026-09-01 00:00 |  |");
  });

  it("stops paging at the page that holds the app", async () => {
    stubFetch(
      {
        ...twoPages(),
        [`${LIST}/${ID_A}/builds`]: () => ({
          status: 200,
          body: { builds: [], promoted_at: null, promoted_by_email: null }
        })
      },
      calls
    );
    const io = capture();

    await runAppsBuilds(fakeContext(), "acme/store", false);

    expect(calls).toEqual([`GET ${LIST}?limit=100&offset=0`, `GET ${LIST}/${ID_A}/builds`]);
    // No builds is an empty result: exit 0, said in words, nothing on stdout.
    expect(io.stdout()).toBe("");
    expect(io.stderr()).toContain("acme/store has no builds");
  });

  it("accepts an app UUID and resolves it to the same row", async () => {
    stubFetch({ ...twoPages(), ...builds }, calls);
    const io = capture();

    await runAppsBuilds(fakeContext(), ID_C.toUpperCase(), true);

    expect(JSON.parse(io.stdout())).toMatchObject({ app: "globex/portal", app_id: ID_C });
  });

  it("exits NOT_FOUND for a slug no page holds", async () => {
    stubFetch(twoPages(), calls);
    capture();

    const error = await failure(runAppsBuilds(fakeContext(), "acme/nope", false));

    expect(error.code).toBe(ExitCode.NOT_FOUND);
    expect(error.message).toBe('no app "acme/nope"');
    // It looked at every page before saying so.
    expect(calls).toHaveLength(2);
  });

  it("refuses an argument that is neither <org>/<app> nor a UUID, before any request", async () => {
    stubFetch(twoPages(), calls);
    for (const bad of ["store", "acme/", "/store"]) {
      const error = await failure(runAppsBuilds(fakeContext(), bad, false));
      expect(error.code).toBe(ExitCode.USAGE);
    }
    expect(calls).toEqual([]);
  });
});

describe("apps show", () => {
  function showRoutes(over: Routes = {}): Routes {
    return {
      ...twoPages(),
      [`${LIST}/${ID_A}/builds`]: () => ({
        status: 200,
        body: {
          builds: [
            build({
              id: "d",
              build_id: "build-draft",
              created_at: "2026-09-20T00:00:00+00:00",
              is_draft: true
            }),
            build({ id: "l", build_id: "build-live", is_published: true })
          ],
          promoted_at: "2026-09-10T08:01:00+00:00",
          promoted_by_email: "ada@oxy.test"
        }
      }),
      [`${LIST}/acme/store/health`]: () => ({ status: 200, body: HEALTH_PASS }),
      [`${LIST}/acme/store/availability`]: () => ({ status: 200, body: AVAILABILITY }),
      [`${LIST}/${ID_A}/activity/summary`]: () => ({ status: 200, body: USAGE }),
      ...over
    };
  }

  it("puts the row, both builds, health, availability and usage on one screen", async () => {
    stubFetch(showRoutes(), calls);
    const io = capture();

    await runAppsShow(fakeContext(), "acme/store", false);

    const text = io.stdout();
    expect(text).toMatch(/state\s+live since 2026-09-10 08:00/);
    expect(text).toMatch(/live build\s+build-live {2}built 2026-09-10 08:00 by ada@oxy.test/);
    expect(text).toContain("oxy-hq/customer-apps@0123456789 (main)");
    expect(text).toMatch(/draft build\s+build-draft {2}built 2026-09-20 00:00/);
    expect(text).toMatch(/health\s+pass \(2 checks\)/);
    expect(text).toMatch(
      /availability\s+healthy — objective 99%; last 24h: 40 request\(s\), 1 failed/
    );
    expect(text).toMatch(/usage, 7 days\s+9 view\(s\), 4 user\(s\), 28 event\(s\)/);
  });

  it("does not show a draft that is the live build itself", async () => {
    stubFetch(
      showRoutes({
        [`${LIST}/${ID_A}/builds`]: () => ({
          status: 200,
          body: {
            builds: [build({ is_published: true, is_draft: true })],
            promoted_at: null,
            promoted_by_email: null
          }
        })
      }),
      calls
    );
    const io = capture();

    await runAppsShow(fakeContext(), "acme/store", true);

    const doc = JSON.parse(io.stdout());
    expect(doc.live_build.build_id).toBe("build-1");
    expect(doc.draft_build).toBeNull();
    expect(doc.failed).toEqual([]);
  });

  /**
   * One backend failing must not discard the others — and must not print as a
   * healthy-looking blank either. The section says NOT READ, the rest prints,
   * and the exit code is the failed request's.
   */
  it("prints what it read, marks a failed section NOT READ, and exits non-zero", async () => {
    stubFetch(
      showRoutes({
        [`${LIST}/acme/store/availability`]: () => ({
          status: 502,
          body: { error: "availability query failed" }
        })
      }),
      calls
    );
    const io = capture();

    const error = await failure(runAppsShow(fakeContext(), "acme/store", false));

    expect(error.code).toBe(ExitCode.UNAVAILABLE);
    expect(error.message).toBe("1 of 3 request(s) failed: availability");
    expect(io.stdout()).toMatch(/availability\s+NOT READ — 502/);
    expect(io.stdout()).toMatch(/health\s+pass/);
    expect(io.stdout()).toMatch(/usage, 7 days\s+9 view/);
  });

  it("--json carries a failed section as null plus an entry in `failed`", async () => {
    stubFetch(
      showRoutes({ [`${LIST}/${ID_A}/activity/summary`]: () => ({ status: 500, body: "boom" }) }),
      calls
    );
    const io = capture();

    const error = await failure(runAppsShow(fakeContext(), "acme/store", true));

    expect(error.code).toBe(ExitCode.UNAVAILABLE);
    const doc = JSON.parse(io.stdout());
    expect(doc.usage).toBeNull();
    expect(doc.health.oxy_app_health).toBe("pass");
    expect(doc.failed.map((f: { section: string }) => f.section)).toEqual(["usage"]);
  });
});

describe("apps health <app>", () => {
  const HEALTH_FAIL = {
    ...HEALTH_PASS,
    oxy_app_health: "fail",
    checks: [
      { name: "registered", result: "pass" },
      { name: "bundle_entrypoint", result: "fail", detail: "index.html is missing" }
    ]
  };

  function healthRoutes(health: () => Reply): Routes {
    return {
      ...twoPages(),
      [`${LIST}/acme/store/health`]: health,
      [`${LIST}/acme/store/availability`]: () => ({ status: 200, body: AVAILABILITY }),
      [`${LIST}/acme/store/errors?hours=24`]: () => ({ status: 200, body: { errors: [] } })
    };
  }

  /**
   * The health route answers 503 WITH ITS REPORT when a check fails. Mapping
   * that to "unavailable, retry" would hide the failing check — the one thing
   * the caller asked to see.
   */
  it("reads a 503 that carries a check list as a failing report, not an outage", async () => {
    stubFetch(
      healthRoutes(() => ({ status: 503, body: HEALTH_FAIL })),
      calls
    );
    const io = capture();

    await runAppsHealth(fakeContext(), "acme/store", {});

    expect(io.stdout()).toContain("FAIL — bundle_entrypoint");
    expect(io.stdout()).toContain("| bundle_entrypoint | fail | index.html is missing |");
    expect(io.stdout()).toContain("none recorded");
  });

  it("treats a 503 without a check list as a failed request", async () => {
    stubFetch(
      healthRoutes(() => ({ status: 503, body: "<html>Service Unavailable</html>" })),
      calls
    );
    const io = capture();

    const error = await failure(runAppsHealth(fakeContext(), "acme/store", { json: true }));

    expect(error.code).toBe(ExitCode.UNAVAILABLE);
    const doc = JSON.parse(io.stdout());
    expect(doc.health).toBeNull();
    expect(doc.availability.verdict).toBe("healthy");
    expect(doc.errors).toEqual([]);
    expect(doc.failed.map((f: { section: string }) => f.section)).toEqual(["health"]);
  });

  it("refuses --needs-attention with an app", async () => {
    stubFetch(twoPages(), calls);
    const error = await failure(
      runAppsHealth(fakeContext(), "acme/store", { needsAttention: true })
    );
    expect(error.code).toBe(ExitCode.USAGE);
    expect(calls).toEqual([]);
  });
});

describe("apps health (the fleet table)", () => {
  const FLEET = "/api/customer-apps/fleet-health";
  const row = (slug: string, health: string) => ({
    app_id: slug,
    app_slug: slug,
    app_name: slug,
    org_id: "org",
    org_slug: "acme",
    health,
    reason: health === "down" ? "every request failed" : null,
    requests: 12,
    failed: health === "down" ? 12 : 0,
    baseline: null,
    window_minutes: 360
  });
  const page = (apps: unknown[], summary: Record<string, number>, hasMore: boolean) => ({
    status: 200,
    body: {
      apps,
      summary: { down: 0, degraded: 0, not_measured: 0, quiet: 0, operational: 0, ...summary },
      total: 201,
      has_more: hasMore,
      evaluated_at: "2026-10-02T11:34:37+00:00",
      observability_configured: true
    }
  });

  it("walks has_more by the page size and adds the pages' summaries up", async () => {
    stubFetch(
      {
        [`${FLEET}?limit=200&offset=0`]: () =>
          page([row("a", "operational")], { operational: 150, quiet: 50 }, true),
        [`${FLEET}?limit=200&offset=200`]: () => page([row("b", "down")], { down: 1 }, false)
      },
      calls
    );
    const io = capture();

    await runAppsHealth(fakeContext(), undefined, { json: true });

    expect(calls).toEqual([`GET ${FLEET}?limit=200&offset=0`, `GET ${FLEET}?limit=200&offset=200`]);
    const doc = JSON.parse(io.stdout());
    expect(doc.apps.map((r: { app_slug: string }) => r.app_slug)).toEqual(["a", "b"]);
    expect(doc.summary).toEqual({
      down: 1,
      degraded: 0,
      not_measured: 0,
      quiet: 50,
      operational: 150
    });
    expect(doc.has_more).toBe(false);
  });

  /**
   * `needs_attention` drops rows AFTER the server cut the page, so a page can
   * hold no rows and still have more behind it. Advancing by the rows returned
   * would ask for offset 0 forever.
   */
  it("--needs-attention still advances past a page that returned no rows", async () => {
    stubFetch(
      {
        [`${FLEET}?limit=200&offset=0&needs_attention=true`]: () =>
          page([], { operational: 200 }, true),
        [`${FLEET}?limit=200&offset=200&needs_attention=true`]: () =>
          page([row("b", "down")], { down: 1 }, false)
      },
      calls
    );
    const io = capture();

    await runAppsHealth(fakeContext(), undefined, { needsAttention: true });

    expect(calls).toHaveLength(2);
    expect(io.stdout()).toContain(
      "| acme/b | b | down | every request failed | 12 | 12 | — | 6h |"
    );
    expect(io.stderr()).toContain(
      "201 published app(s): 1 down, 0 degraded, 0 not measured, 0 quiet, 200 operational"
    );
  });

  it("says nothing needs attention, rather than printing an empty table", async () => {
    stubFetch(
      {
        [`${FLEET}?limit=200&offset=0&needs_attention=true`]: () => page([], { quiet: 201 }, false)
      },
      calls
    );
    const io = capture();

    await runAppsHealth(fakeContext(), undefined, { needsAttention: true });

    expect(io.stdout()).toBe("");
    expect(io.stderr()).toContain("none of the 201 published app(s) needs attention");
  });

  it("rejects when the fleet request fails", async () => {
    stubFetch({ [`${FLEET}?limit=200&offset=0`]: () => ({ status: 500, body: "boom" }) }, calls);
    const io = capture();

    const error = await failure(runAppsHealth(fakeContext(), undefined, {}));

    expect(error.code).toBe(ExitCode.UNAVAILABLE);
    expect(io.stdout()).toBe("");
  });
});

describe("apps usage", () => {
  const base = `${LIST}/${ID_A}/activity`;
  const routes = (): Routes => ({
    ...twoPages(),
    [`${base}/summary`]: () => ({ status: 200, body: USAGE }),
    [`${base}/visitors?days=7&limit=50`]: () => ({
      status: 200,
      body: {
        rows: [
          {
            user_id: "u1",
            user_email: "ada@oxy.test",
            sessions: 3,
            views: 5,
            first_seen_at: "2026-09-28T19:40:00Z",
            last_seen_at: "2026-10-01T22:21:00Z",
            app_role: null,
            org_role: "member"
          }
        ]
      }
    }),
    [`${base}/events?days=7&limit=50`]: () => ({ status: 200, body: { groups: [] } })
  });

  it("prints the summary, the visitors, and says when there are no events", async () => {
    stubFetch(routes(), calls);
    const io = capture();

    await runAppsUsage(fakeContext(), "acme/store", false);

    const text = io.stdout();
    expect(text).toContain("9 view(s), 4 user(s), 28 event(s); last viewed 2026-10-01 22:21");
    expect(text).toContain(
      "| ada@oxy.test | 3 | 5 | 2026-09-28 19:40 | 2026-10-01 22:21 | — | member |"
    );
    expect(text).toMatch(/## Events\n\nnone/);
  });

  it("rejects, printing nothing, when one of its requests fails", async () => {
    const failing = routes();
    failing[`${base}/visitors?days=7&limit=50`] = () => ({ status: 500, body: "boom" });
    stubFetch(failing, calls);
    const io = capture();

    const error = await failure(runAppsUsage(fakeContext(), "acme/store", true));

    expect(error.code).toBe(ExitCode.UNAVAILABLE);
    expect(io.stdout()).toBe("");
  });
});
