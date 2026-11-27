/**
 * `oxyc checks run <app> --build … --report`, against a stand-in platform on a
 * loopback port (`testing/fake-checks-platform.ts`) and a scripted browser
 * (`testing/fake-playwright.ts`): the order of the calls, the exact bodies of
 * the two reports, and what the run exits with. The theme guard is the SDK's
 * real one.
 *
 * What goes wrong, and what the run does about it: `checks-build-failures.test.ts`.
 */

import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, type MockInstance, vi } from "vitest";
import { confinementArgs } from "../checks/browser-confine.js";
import { type Context, createContext } from "../context/resolve.js";
import {
  APP_ID,
  BUILD_ID,
  BUILD_ROW,
  claimOf,
  type FakePlatform,
  startFakePlatform,
  WORKSPACE_ID
} from "../testing/fake-checks-platform.js";
import { FakeChromium, type PageScript } from "../testing/fake-playwright.js";
import { CliError, ExitCode } from "../util/errors.js";
import { type BuildChecksDeps, type BuildChecksSummary, runBuildChecks } from "./checks-build.js";

/** The variables GitHub sets in the workflow's job. */
const GITHUB_ENV = {
  GITHUB_ACTIONS: "true",
  GITHUB_RUN_NUMBER: "1482",
  GITHUB_RUN_ID: "123",
  GITHUB_SERVER_URL: "https://github.com",
  GITHUB_REPOSITORY: "oxy-hq/oxygen-internal"
};

describe("oxyc checks run --build", () => {
  let platform: FakePlatform;
  let stdout: MockInstance;
  let stderr: MockInstance;
  let waits: number[];

  /** A page that mounts on staging's build, at any viewport: it reports the build's row id. */
  function readyPage(): PageScript {
    return {
      requests: [
        { url: `${platform.pageUrl}assets/index.js` },
        {
          method: "POST",
          url: `${platform.pageUrl}__oxy/beacon`,
          postData: '{"events":[{"n":"oxy-app-ready"}]}'
        }
      ],
      app: { environment: "staging", buildId: BUILD_ROW }
    };
  }

  /** The job as the workflow runs it: the checker's token in `OXY_API_KEY`, no bearer anywhere. */
  function asChecker(): Context {
    vi.stubEnv("OXY_API_KEY", "oxysvc_checker");
    vi.stubEnv("OXY_CHECKS_NO_BEARER", "");
    vi.stubEnv("OXY_CREDENTIALS_PATH", join(tmpdir(), "oxyc-no-such-credentials.json"));
    return createContext({ target: platform.url, tokenEnv: "OXY_CHECKS_NO_BEARER" }, tmpdir());
  }

  function deps(chromium: FakeChromium, overrides: Partial<BuildChecksDeps> = {}) {
    return {
      loadChromium: async () => chromium,
      pageWaitMs: 40,
      pageSettleMs: 0,
      sleep: async (ms: number) => {
        waits.push(ms);
      },
      env: GITHUB_ENV,
      ...overrides
    };
  }

  function printed(stream: MockInstance): string {
    // oxlint-disable-next-line typescript/no-base-to-string -- every write here is a string
    const text = stream.mock.calls.map(([chunk]) => String(chunk)).join("");
    // biome-ignore lint/suspicious/noControlCharactersInRegex: stripping ANSI colour
    return text.replace(/\u001b\[[0-9;]*m/g, "");
  }

  beforeEach(async () => {
    platform = await startFakePlatform();
    waits = [];
    stdout = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    stderr = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
  });

  afterEach(async () => {
    stdout.mockRestore();
    stderr.mockRestore();
    vi.unstubAllEnvs();
    await platform.close();
  });

  it("claims, waits for the functions, loads, reads the CSS, and reports each check as it finishes", async () => {
    // The functions take three asks to reach a verdict.
    platform.on("POST /functions/settle", (_, nth) => ({
      status: 200,
      body:
        nth < 3
          ? { done: false, status: "running", summary: `${nth} of 3 done` }
          : { done: true, status: "passed", summary: "3 of 3 passed" }
    }));
    platform.on("POST /report", (_, nth) => ({
      status: 200,
      body:
        nth === 1 ? { state: "running", open: ["design_system"] } : { state: "passed", open: [] }
    }));
    const chromium = new FakeChromium(readyPage);

    await runBuildChecks(
      asChecker(),
      APP_ID,
      { build: BUILD_ROW, attempt: "2", report: true, json: true },
      deps(chromium)
    );

    // The claim first, naming the runner; then the two reports in order, the
    // stylesheets read between them.
    const routes = platform.routes();
    expect(routes[0]).toBe("POST /claim");
    expect(platform.bodies("POST /claim")).toEqual([
      {
        attempt: 2,
        runner: {
          label: "GitHub Actions run #1482",
          url: "https://github.com/oxy-hq/oxygen-internal/actions/runs/123"
        },
        // Named on every claim and every report: staging unless the run says production.
        environment: "staging"
      }
    ]);
    expect(routes.filter((r) => !r.endsWith("/settle"))).toEqual([
      "POST /claim",
      "POST /report",
      "GET /stylesheets",
      "POST /report"
    ]);
    // Asked until done, every 2 s, with the attempt.
    expect(platform.bodies("POST /functions/settle")).toEqual([
      { attempt: 2 },
      { attempt: 2 },
      { attempt: 2 }
    ]);
    expect(waits).toEqual([2000, 2000]);

    const viewport = (name: string, width: number, height: number) => ({
      name,
      width,
      height,
      ready: true,
      ready_ms: expect.any(Number),
      environment: "staging",
      build_id: BUILD_ROW,
      page_errors: [],
      failed_requests: [],
      refused_writes: [],
      refused_off_origin: []
    });
    expect(platform.bodies("POST /report")).toEqual([
      {
        attempt: 2,
        environment: "staging",
        checks: [
          {
            kind: "loads",
            status: "passed",
            summary: expect.stringMatching(/^ready in \d\.\ds desk, \d\.\ds phone$/),
            detail: {
              findings: [],
              url: platform.pageUrl,
              identity: "app_checker",
              viewports: [viewport("desk", 1440, 900), viewport("phone", 390, 844)]
            }
          }
        ]
      },
      {
        attempt: 2,
        environment: "staging",
        checks: [
          {
            kind: "design_system",
            status: "passed",
            summary: "SDK 2.31.0 · guard clean",
            detail: {
              findings: [],
              sdk_version: "2.31.0",
              sdk_floor: "2.30.0",
              files: ["index.html", "assets/index-abc.css"]
            }
          }
        ]
      }
    ]);

    const summary = JSON.parse(printed(stdout)) as BuildChecksSummary;
    expect(summary).toEqual({
      app: "acme/store",
      appId: APP_ID,
      build: { id: BUILD_ROW, build_id: BUILD_ID },
      attempt: 2,
      environment: "staging",
      runner: {
        label: "GitHub Actions run #1482",
        url: "https://github.com/oxy-hq/oxygen-internal/actions/runs/123"
      },
      closed: false,
      checks: [
        { kind: "functions", by: "platform", status: "passed", summary: "3 of 3 passed" },
        {
          kind: "loads",
          by: "runner",
          status: "passed",
          summary: expect.stringMatching(/^ready in /),
          reported: true
        },
        {
          kind: "design_system",
          by: "runner",
          status: "passed",
          summary: "SDK 2.31.0 · guard clean",
          reported: true
        }
      ],
      state: "passed",
      open: []
    });
  });

  it("sends the checker's token as X-API-Key on every call, and looks no app up", async () => {
    await runBuildChecks(
      asChecker(),
      APP_ID,
      { build: BUILD_ROW, report: true, json: true },
      deps(new FakeChromium(readyPage))
    );
    expect(platform.calls.length).toBeGreaterThanOrEqual(5);
    const base = `/api/customer-apps/${APP_ID}/builds/${BUILD_ROW}/checks/`;
    for (const call of platform.calls) {
      // Nothing outside the build's checks: not the app listing, not the app itself.
      expect(call.path.startsWith(base), call.path).toBe(true);
      expect(call.headers["x-api-key"]).toBe("oxysvc_checker");
      expect(call.headers.authorization).toBeUndefined();
    }
    // No attempt named: the newest.
    expect(platform.bodies("POST /claim")).toMatchObject([{ attempt: null }]);
  });

  it("loads the page as the checker: signed under the app's base and on its data reads, a write never sent", async () => {
    const page = (): PageScript => ({
      ...readyPage(),
      requests: [
        { method: "POST", url: `${platform.url}/api/projects/${WORKSPACE_ID}/query` },
        { method: "POST", url: `${platform.pageUrl}fn/save` },
        ...(readyPage().requests ?? [])
      ]
    });
    const chromium = new FakeChromium(page);
    await runBuildChecks(
      asChecker(),
      APP_ID,
      { build: BUILD_ROW, report: true, json: true },
      deps(chromium)
    );
    const signed = { "x-api-key": "oxysvc_checker", "x-oxy-app-env": "staging" };
    const desk = chromium.sent.filter((s) => s.context === 0);
    expect(desk.map((s) => [s.method, s.url.replace(platform.url, ""), s.decision])).toEqual([
      ["GET", "/customer-apps/acme/store/", "fulfilled"],
      ["POST", `/api/projects/${WORKSPACE_ID}/query`, "fulfilled"],
      ["POST", "/customer-apps/acme/store/fn/save", "aborted"],
      ["GET", "/customer-apps/acme/store/assets/index.js", "fulfilled"],
      ["POST", "/customer-apps/acme/store/__oxy/beacon", "fulfilled"]
    ]);
    expect(desk[0]?.headers).toMatchObject(signed);
    expect(desk[1]?.headers).toMatchObject({ ...signed, "x-oxy-app": APP_ID });
    // The write is a warning on a page that still passes.
    const [loads] = platform.bodies("POST /report") as {
      checks: { status: string; detail: { findings: unknown[] } }[];
    }[];
    expect(loads?.checks[0]?.status).toBe("passed");
    expect(loads?.checks[0]?.detail.findings).toEqual([
      {
        severity: "warning",
        message:
          "desk: not sent: POST /customer-apps/acme/store/fn/save (the checker reads, and sends no write)",
        at: null,
        rule: "write-refused"
      },
      {
        severity: "warning",
        message:
          "phone: not sent: POST /customer-apps/acme/store/fn/save (the checker reads, and sends no write)",
        at: null,
        rule: "write-refused"
      }
    ]);
  });

  it("starts the browser sandboxed, with the job's environment minus every credential", async () => {
    const chromium = new FakeChromium(readyPage);
    await runBuildChecks(
      asChecker(),
      APP_ID,
      { build: BUILD_ROW, report: true, json: true },
      deps(chromium, {
        env: {
          ...GITHUB_ENV,
          PATH: "/usr/bin:/bin",
          HOME: "/home/runner",
          OXY_API_KEY: "oxysvc_checker",
          GITHUB_TOKEN: "ghs_x"
        }
      })
    );
    expect(chromium.launchOptions).toEqual([
      {
        headless: true,
        chromiumSandbox: true,
        // Able to find the deployment's own host and port, and no other.
        args: confinementArgs(platform.url),
        env: { PATH: "/usr/bin:/bin", HOME: "/home/runner" }
      }
    ]);
    // The token still reaches the page's signed requests: by header, not by the environment.
    expect(chromium.sent[0]?.headers).toMatchObject({ "x-api-key": "oxysvc_checker" });
  });

  it("says nothing is left when the attempt is already closed, and exits 0", async () => {
    platform.on("POST /claim", () => ({
      status: 409,
      body: { code: "attempt_closed", message: "every check of attempt 2 has a verdict" }
    }));
    const chromium = new FakeChromium(readyPage);
    await runBuildChecks(
      asChecker(),
      APP_ID,
      { build: BUILD_ROW, attempt: "2", report: true },
      deps(chromium)
    );
    expect(platform.routes()).toEqual(["POST /claim"]);
    expect(chromium.launches).toBe(0);
    expect(printed(stderr)).toContain("already has a verdict: nothing is left to check");
  });

  it("runs only what the claim left open", async () => {
    platform.on("POST /claim", () => ({
      status: 200,
      body: claimOf(platform.pageUrl, { serving: false, open: ["design_system"] })
    }));
    const chromium = new FakeChromium(readyPage);
    await runBuildChecks(
      asChecker(),
      APP_ID,
      { build: BUILD_ROW, report: true, json: true },
      deps(chromium)
    );
    expect(platform.routes()).toEqual(["POST /claim", "GET /stylesheets", "POST /report"]);
    expect(chromium.launches).toBe(0);
    const summary = JSON.parse(printed(stdout)) as BuildChecksSummary;
    expect(summary.checks.map((c) => c.kind)).toEqual(["design_system"]);
  });

  it("reports the principal the platform resolved the credential to, when it says", async () => {
    platform.on("POST /claim", () => ({
      status: 200,
      body: claimOf(platform.pageUrl, { identity: "service:app-checks", open: ["loads"] })
    }));
    await runBuildChecks(
      asChecker(),
      APP_ID,
      { build: BUILD_ROW, report: true, json: true },
      deps(new FakeChromium(readyPage))
    );
    expect(platform.bodies("POST /report")).toMatchObject([
      { checks: [{ kind: "loads", detail: { identity: "service:app-checks" } }] }
    ]);
  });

  it("prints one line per check as it lands, and exits 9 when a verdict is failed", async () => {
    // The page never becomes ready, at either width.
    const chromium = new FakeChromium(() => ({
      app: { environment: "staging", buildId: BUILD_ROW }
    }));
    const failure = await runBuildChecks(
      asChecker(),
      APP_ID,
      { build: BUILD_ROW, attempt: "2", report: true },
      deps(chromium, { env: {} })
    ).catch((cause: unknown) => cause);

    expect(failure).toBeInstanceOf(CliError);
    expect((failure as CliError).code).toBe(ExitCode.CHECK_FAILED);
    expect((failure as CliError).message).toBe(
      `1 of 3 check(s) did not pass on build ${BUILD_ID}: It loads, desk and phone`
    );
    const lines = printed(stderr);
    expect(lines).toContain(
      `checking acme/store · build ${BUILD_ID} · staging · attempt 2 (oxyc, run by hand)`
    );
    expect(lines).toContain("✓ Its checks pass  3 of 3 passed");
    expect(lines).toContain(
      "✗ It loads, desk and phone  desk: the app never sent oxy-app-ready within 0s"
    );
    expect(lines).toContain("✓ Follows the design system  SDK 2.31.0 · guard clean");
    // By hand there is no run to link to.
    expect(platform.bodies("POST /claim")).toMatchObject([
      { runner: { label: "oxyc, run by hand", url: null } }
    ]);
    // The failed verdict was written, and the design-system check still ran.
    expect(platform.bodies("POST /report")).toMatchObject([
      { checks: [{ kind: "loads", status: "failed" }] },
      { checks: [{ kind: "design_system", status: "passed" }] }
    ]);
  });

  it("resolves <org>/<app> by the listing for a person, then claims", async () => {
    vi.stubEnv("OXY_TOKEN", "a-persons-session");
    vi.stubEnv("OXY_API_KEY", "");
    vi.stubEnv("OXY_CREDENTIALS_PATH", join(tmpdir(), "oxyc-no-such-credentials.json"));
    platform.on("GET /api/customer-apps?limit=100&offset=0", () => ({
      status: 200,
      body: { items: [{ id: APP_ID, slug: "store", org_slug: "acme" }], next_offset: null }
    }));
    platform.on("POST /claim", () => ({
      status: 200,
      body: claimOf(platform.pageUrl, { open: ["loads"] })
    }));
    const chromium = new FakeChromium(readyPage);
    await runBuildChecks(
      createContext({ target: platform.url }, tmpdir()),
      "acme/store",
      { build: BUILD_ROW, report: true, json: true },
      deps(chromium)
    );
    expect(platform.routes()).toEqual([
      "GET /api/customer-apps?limit=100&offset=0",
      "POST /claim",
      "POST /report"
    ]);
    expect(platform.calls[1]?.headers.authorization).toBe("Bearer a-persons-session");
    // The page is opened as that person, not with an API key.
    expect(chromium.sent[0]?.headers).toMatchObject({
      authorization: "Bearer a-persons-session",
      "x-oxy-app-env": "staging"
    });
  });
});
