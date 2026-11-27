/**
 * `oxyc checks run <app> --build … --report` when something goes wrong: a
 * check that throws, a verdict the platform does not take, functions that
 * never settle, a refused claim, a wrong invocation. Same stand-ins as
 * `checks-build.test.ts`.
 *
 * The rule under test: a runner's check is never left open by a failure here,
 * and the exit code says whether the answer was "no" (9) or the run broke.
 */

import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, type MockInstance, vi } from "vitest";
import { type Context, createContext } from "../context/resolve.js";
import {
  APP_ID,
  BUILD_ID,
  BUILD_ROW,
  claimOf,
  type FakePlatform,
  startFakePlatform
} from "../testing/fake-checks-platform.js";
import { FakeChromium, type PageScript } from "../testing/fake-playwright.js";
import { CliError, ExitCode } from "../util/errors.js";
import {
  type BuildChecksDeps,
  type BuildChecksOptions,
  type BuildChecksSummary,
  runBuildChecks
} from "./checks-build.js";

describe("oxyc checks run --build, when something goes wrong", () => {
  let platform: FakePlatform;
  let stdout: MockInstance;
  let stderr: MockInstance;
  let waits: number[];

  function readyPage(): PageScript {
    return {
      requests: [
        {
          method: "POST",
          url: `${platform.pageUrl}__oxy/beacon`,
          postData: '{"events":[{"n":"oxy-app-ready"}]}'
        }
      ],
      app: { environment: "staging", buildId: BUILD_ROW }
    };
  }

  function asChecker(): Context {
    vi.stubEnv("OXY_API_KEY", "oxysvc_checker");
    vi.stubEnv("OXY_CHECKS_NO_BEARER", "");
    vi.stubEnv("OXY_CREDENTIALS_PATH", join(tmpdir(), "oxyc-no-such-credentials.json"));
    return createContext({ target: platform.url, tokenEnv: "OXY_CHECKS_NO_BEARER" }, tmpdir());
  }

  /** Run it; the summary printed, and the error it ended with (if any). */
  async function run(
    overrides: Partial<BuildChecksDeps> = {},
    opts: BuildChecksOptions = { build: BUILD_ROW, attempt: "2", report: true, json: true }
  ): Promise<{ summary?: BuildChecksSummary; failure?: CliError }> {
    const failure = await runBuildChecks(asChecker(), APP_ID, opts, {
      loadChromium: async () => new FakeChromium(readyPage),
      pageWaitMs: 40,
      pageSettleMs: 0,
      sleep: async (ms: number) => {
        waits.push(ms);
      },
      env: {},
      ...overrides
    }).then(
      () => undefined,
      (cause: unknown) => cause
    );
    if (failure !== undefined && !(failure instanceof CliError)) throw failure;
    // oxlint-disable-next-line typescript/no-base-to-string -- every write here is a string
    const text = stdout.mock.calls.map(([chunk]) => String(chunk)).join("");
    return { summary: text ? (JSON.parse(text) as BuildChecksSummary) : undefined, failure };
  }

  /** What one kind was reported as. */
  function reported(kind: string): Record<string, unknown> | undefined {
    const bodies = platform.bodies("POST /report") as { checks: Record<string, unknown>[] }[];
    return bodies.flatMap((body) => body.checks).find((check) => check.kind === kind);
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

  it("reports a check that throws as error, with why, and still runs the next", async () => {
    const { summary, failure } = await run({
      loadChromium: async () => {
        throw new Error(`no browser here\n${"x".repeat(900)}`);
      }
    });
    expect(reported("loads")).toEqual({
      kind: "loads",
      status: "error",
      summary: "no browser here",
      detail: {
        findings: [{ severity: "error", message: "no browser here", at: null, rule: null }]
      }
    });
    expect(reported("design_system")).toMatchObject({ status: "passed" });
    expect(summary?.checks.map((c) => [c.kind, c.status, c.reported])).toEqual([
      ["functions", "passed", undefined],
      ["loads", "error", true],
      ["design_system", "passed", true]
    ]);
    // An error is an answer of "no", not a broken run.
    expect(failure?.code).toBe(ExitCode.CHECK_FAILED);
  });

  it("reports loads as error, naming the sandbox, when Chromium cannot start with it", async () => {
    const chromium = new FakeChromium(readyPage);
    chromium.launchError =
      "browserType.launch: Target page, context or browser has been closed\nBrowser logs:\n[err] No usable sandbox! … you can try using --no-sandbox.";
    const { failure } = await run({ loadChromium: async () => chromium });
    expect(reported("loads")).toMatchObject({
      status: "error",
      summary:
        "Chromium could not start with its sandbox on (no usable sandbox on this machine); the page is not opened without one"
    });
    // Asked for once, with the sandbox: never again without it.
    expect(chromium.launchOptions.map((o) => o.chromiumSandbox)).toEqual([true]);
    expect(reported("design_system")).toMatchObject({ status: "passed" });
    expect(failure?.code).toBe(ExitCode.CHECK_FAILED);
  });

  it("caps a thrown message at a summary's length", async () => {
    await run({
      loadChromium: async () => {
        throw new Error("y".repeat(900));
      }
    });
    const summary = reported("loads")?.summary as string;
    expect(Array.from(summary)).toHaveLength(200);
    expect(summary.endsWith("…")).toBe(true);
  });

  it("reports loads as error, without opening a browser, when the page is on another origin", async () => {
    platform.on("POST /claim", () => ({
      status: 200,
      body: claimOf("https://elsewhere.example.net/customer-apps/acme/store/", { open: ["loads"] })
    }));
    const chromium = new FakeChromium(readyPage);
    const { failure } = await run({ loadChromium: async () => chromium });
    expect(chromium.launches).toBe(0);
    expect(reported("loads")).toMatchObject({
      status: "error",
      summary: expect.stringMatching(
        /^the platform named a page on https:\/\/elsewhere\.example\.net, but this run targets http:\/\/127\.0\.0\.1:\d+/
      )
    });
    expect(failure?.code).toBe(ExitCode.CHECK_FAILED);
  });

  it("reports design_system as error when the build's CSS cannot be read", async () => {
    platform.on("GET /stylesheets", () => ({ status: 404, body: { code: "not_found" } }));
    const { failure } = await run();
    expect(reported("design_system")).toMatchObject({
      status: "error",
      summary: expect.stringMatching(/^the bundle's CSS could not be read: 404 Not Found/)
    });
    expect(failure?.code).toBe(ExitCode.CHECK_FAILED);
  });

  it("sends a report again when the platform could not take it, with a growing wait", async () => {
    platform.on("POST /report", (_, nth) =>
      nth <= 2
        ? { status: 503, body: { code: "unavailable" } }
        : { status: 200, body: { state: "passed", open: [] } }
    );
    platform.on("POST /claim", () => ({
      status: 200,
      body: claimOf(platform.pageUrl, { open: ["design_system"] })
    }));
    const { summary, failure } = await run();
    expect(failure).toBeUndefined();
    const bodies = platform.bodies("POST /report");
    expect(bodies).toHaveLength(3);
    expect(bodies[1]).toEqual(bodies[0]);
    expect(bodies[2]).toEqual(bodies[0]);
    expect(waits).toEqual([1000, 2000]);
    expect(summary?.checks).toMatchObject([{ kind: "design_system", reported: true }]);
  });

  it("gives up on a report after three repeats, says the verdict was not written, and exits as the request did", async () => {
    platform.on("POST /report", (call) =>
      (call.body as { checks: { kind: string }[] }).checks[0]?.kind === "loads"
        ? { status: 503, body: { code: "unavailable" } }
        : { status: 200, body: { state: "running", open: ["loads"] } }
    );
    const { summary, failure } = await run();
    // Once, then three repeats: 1 s, 2 s and 4 s apart.
    const loads = platform
      .bodies("POST /report")
      .filter((b) => JSON.stringify(b).includes("loads"));
    expect(loads).toHaveLength(4);
    expect(waits).toEqual([1000, 2000, 4000]);
    // The next check still ran and was written.
    expect(reported("design_system")).toMatchObject({ status: "passed" });
    expect(summary?.checks.map((c) => [c.kind, c.reported])).toEqual([
      ["functions", undefined],
      ["loads", false],
      ["design_system", true]
    ]);
    expect(summary?.open).toEqual(["loads"]);
    // A broken run, not a verdict: the platform was unavailable.
    expect(failure?.code).toBe(ExitCode.UNAVAILABLE);
  });

  it("does not repeat a report the platform refused", async () => {
    platform.on("POST /report", () => ({
      status: 400,
      body: { code: "kind_not_reportable", message: "only loads and design_system" }
    }));
    platform.on("POST /claim", () => ({
      status: 200,
      body: claimOf(platform.pageUrl, { open: ["design_system"] })
    }));
    const { failure } = await run();
    expect(platform.bodies("POST /report")).toHaveLength(1);
    expect(waits).toEqual([]);
    expect(failure?.code).toBe(ExitCode.REQUEST);
  });

  it("exits 9 when the platform's verdict on the functions is failed", async () => {
    platform.on("POST /functions/settle", () => ({
      status: 200,
      body: { done: true, status: "failed", summary: "1 of 3 failed: canary" }
    }));
    const { summary, failure } = await run();
    expect(summary?.checks[0]).toEqual({
      kind: "functions",
      by: "platform",
      status: "failed",
      summary: "1 of 3 failed: canary"
    });
    expect(failure?.code).toBe(ExitCode.CHECK_FAILED);
    expect(failure?.message).toContain("Its checks pass");
  });

  it("stops asking about the functions after the wait, having reported its own checks, and exits 7", async () => {
    platform.on("POST /functions/settle", () => ({
      status: 200,
      body: { done: false, status: "running", summary: "1 of 3 done" }
    }));
    const { summary, failure } = await run({ settleTimeoutMs: 0 });
    expect(platform.bodies("POST /functions/settle")).toHaveLength(1);
    expect(reported("loads")).toMatchObject({ status: "passed" });
    expect(reported("design_system")).toMatchObject({ status: "passed" });
    expect(summary?.checks[0]).toEqual({
      kind: "functions",
      by: "platform",
      status: "running",
      summary: "1 of 3 done"
    });
    expect(failure?.code).toBe(ExitCode.UNAVAILABLE);
    expect(failure?.message).toBe(
      `the check functions of build ${BUILD_ID} had no verdict after 0s`
    );
  });

  it("answers a refused claim with that request's own code, and runs nothing", async () => {
    for (const [status, code] of [
      [404, ExitCode.NOT_FOUND],
      [401, ExitCode.AUTH],
      [409, ExitCode.REQUEST],
      [500, ExitCode.UNAVAILABLE]
    ] as const) {
      platform.calls.length = 0;
      platform.on("POST /claim", () => ({ status, body: { code: "refused", message: "no" } }));
      const { failure } = await run();
      expect(failure?.code, String(status)).toBe(code);
      expect(new Set(platform.routes()), String(status)).toEqual(new Set(["POST /claim"]));
    }
  });

  it("refuses a wrong invocation before any request", async () => {
    const usage = async (opts: BuildChecksOptions) => {
      const { failure } = await run({}, opts);
      expect(failure?.code).toBe(ExitCode.USAGE);
      return failure?.message;
    };
    // It writes, so it has to be told to.
    expect(await usage({ build: BUILD_ROW, attempt: "2" })).toMatch(/pass --report to say so$/);
    expect(await usage({ report: true, attempt: "2" })).toBe(
      "--report, --attempt and --environment need --build <build>"
    );
    expect(await usage({ environment: "production" })).toBe(
      "--report, --attempt and --environment need --build <build>"
    );
    // Two environments have attempts; anything else is refused by name.
    expect(await usage({ build: BUILD_ROW, report: true, environment: "dev-a1" })).toBe(
      '--environment must be staging or production, not "dev-a1"'
    );
    expect(await usage({ build: BUILD_ROW, attempt: "two", report: true })).toMatch(
      /--attempt must be a positive whole number/
    );
    expect(await usage({ build: BUILD_ROW, report: true, browser: true })).toMatch(
      /do not go with --build/
    );
    expect(await usage({ build: BUILD_ROW, report: true, appEnv: "staging" })).toMatch(
      /do not go with --build/
    );
    expect(platform.calls).toEqual([]);
  });

  it("needs a credential: the checker's token or a person's login", async () => {
    vi.stubEnv("OXY_API_KEY", "");
    vi.stubEnv("OXY_CHECKS_NO_BEARER", "");
    const ctx = createContext({ target: platform.url, tokenEnv: "OXY_CHECKS_NO_BEARER" }, tmpdir());
    const failure = await runBuildChecks(ctx, APP_ID, { build: BUILD_ROW, report: true }).catch(
      (cause: unknown) => cause
    );
    expect((failure as CliError).code).toBe(ExitCode.AUTH);
    expect(platform.calls).toEqual([]);
  });
});
