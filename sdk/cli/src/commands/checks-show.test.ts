/**
 * `oxyc checks show`, driven with `globalThis.fetch` stubbed by a route table —
 * the same harness as `checks.test.ts`. What is pinned: which build is read
 * (staging's, or `--build`), the human lines for each state, `--json` as the
 * server sent it, `--wait`'s poll, and the exit code a script gates on.
 */

import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, type MockInstance, vi } from "vitest";
import { resetOidcExchanges } from "../auth/oidc.js";
import { type Context, createContext } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import { type BuildCheck, type BuildChecks, runChecksShow } from "./checks-show.js";

const TARGET = "https://oxy.test";
const APP_ID = "a1a1a1a1-2222-3333-4444-555555555555";
/** The row id of the build staging serves. */
const STAGING_ROW = "b2b2b2b2-2222-3333-4444-555555555555";
const CHECKS_PATH = `/api/customer-apps/${APP_ID}/builds/${STAGING_ROW}/checks`;

function contextWith(bearer: string): Context {
  vi.stubEnv("OXY_TOKEN", bearer);
  vi.stubEnv("OXY_API_KEY", "");
  vi.stubEnv("OXY_SERVICE_ACCOUNT", "");
  vi.stubEnv("OXY_CREDENTIALS_PATH", join(tmpdir(), "oxyc-no-such-credentials.json"));
  // Not GitHub Actions: nothing may be minted.
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_URL", "");
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "");
  return createContext({ env: "production", target: TARGET }, tmpdir());
}

type RouteTable = Record<string, () => { status: number; body: unknown }>;

function stubFetch(routes: RouteTable, calls: string[]) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string | URL, init?: RequestInit) => {
      const path = String(input).replace(TARGET, "");
      const key = `${(init?.method ?? "GET").toUpperCase()} ${path}`;
      calls.push(key);
      const handler = routes[key];
      if (!handler) {
        return new Response(JSON.stringify({ error: `no stub for ${key}` }), { status: 404 });
      }
      const { status, body } = handler();
      return new Response(JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json" }
      });
    })
  );
}

/** A user credential resolves `acme/store` by the listing, then reads its builds. */
function appRoutes(): RouteTable {
  return {
    "GET /api/customer-apps?limit=100&offset=0": () => ({
      status: 200,
      body: { items: [{ id: APP_ID, slug: "store", org_slug: "acme" }], next_offset: null }
    }),
    [`GET /api/customer-apps/${APP_ID}/builds`]: () => ({
      status: 200,
      body: {
        promoted_at: null,
        promoted_by_email: null,
        builds: [
          build("c3c3c3c3-2222-3333-4444-555555555555", "older", false, true),
          build(STAGING_ROW, "9087d7b", true, false)
        ]
      }
    })
  };
}

function build(id: string, buildId: string, isDraft: boolean, isPublished: boolean) {
  return {
    id,
    build_id: buildId,
    created_at: isDraft ? "2026-11-20T10:00:00Z" : "2026-11-19T10:00:00Z",
    is_draft: isDraft,
    is_published: isPublished,
    published_by_email: null,
    published_via: null,
    source_repo: null,
    commit_sha: null,
    source_branch: null
  };
}

function check(
  kind: string,
  label: string,
  status: string,
  summary: string | null,
  findings: BuildCheck["detail"]["findings"] = []
): BuildCheck {
  return {
    kind,
    label,
    status,
    advisory: false,
    summary,
    detail: { findings },
    attempt: 2,
    duration_ms: status === "passed" ? 1200 : null
  };
}

const VALIDATION = check("validation", "Builds and validates", "passed", "bundle valid");
const FUNCTIONS = check("functions", "Its checks pass", "passed", "3 of 3 passed");
const LOADS = check("loads", "It loads, desk and phone", "passed", "desk 1.2s · phone 1.4s");
const DESIGN = check("design_system", "Follows the design system", "passed", "theme guard clean");

function body(state: string, checks: BuildCheck[], attempt: number | null = 2): BuildChecks {
  return {
    app_id: APP_ID,
    build: { id: STAGING_ROW, build_id: "9087d7b", created_at: "2026-11-20T10:00:00Z" },
    attempt,
    environment: "staging",
    state,
    checks,
    attempts:
      attempt === null
        ? []
        : [
            { attempt, state, created_at: "2026-11-20T10:01:00Z" },
            { attempt: 1, state: "failed", created_at: "2026-11-20T10:00:30Z" }
          ]
  };
}

function finding(at: string | null, message: string) {
  return { severity: "error" as const, message, at, rule: "restates-sdk-token" };
}

const FAILED = body("failed", [
  VALIDATION,
  FUNCTIONS,
  LOADS,
  check(
    "design_system",
    "Follows the design system",
    "failed",
    "src/index.css:68 restates --card",
    [
      finding("src/index.css:68", "restates --card, which theme.css defines; remove it"),
      finding("src/index.css:70", "restates --border, which theme.css defines; remove it"),
      finding(null, "loads a second font stack"),
      finding("src/app.css:4", "restates --ring, which theme.css defines; remove it")
    ]
  )
]);

describe("runChecksShow", () => {
  let calls: string[];
  let stdout: MockInstance;

  /** What the command wrote to stdout, without colour. */
  function printed(): string {
    // oxlint-disable-next-line typescript/no-base-to-string -- every write here is a string
    const text = stdout.mock.calls.map(([chunk]) => String(chunk)).join("");
    // biome-ignore lint/suspicious/noControlCharactersInRegex: stripping ANSI colour
    return text.replace(/\u001b\[[0-9;]*m/g, "");
  }

  beforeEach(() => {
    calls = [];
    resetOidcExchanges();
    stdout = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    vi.stubEnv("OXYC_QUIET", "1");
  });

  afterEach(() => {
    stdout.mockRestore();
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
  });

  it("reads the build staging serves and prints a passed build", async () => {
    stubFetch(
      {
        ...appRoutes(),
        [`GET ${CHECKS_PATH}`]: () => ({
          status: 200,
          body: body("passed", [VALIDATION, FUNCTIONS, LOADS, DESIGN])
        })
      },
      calls
    );
    await runChecksShow(contextWith("user-token"), "acme/store", { timeoutSeconds: 600 });

    // Staging's build is the `is_draft` row, read by its row id.
    expect(calls).toContain(`GET ${CHECKS_PATH}`);
    expect(printed()).toBe(
      [
        "acme/store · build 9087d7b · staging · attempt 2",
        "  ✓ Builds and validates       bundle valid",
        "  ✓ Its checks pass            3 of 3 passed",
        "  ✓ It loads, desk and phone   desk 1.2s · phone 1.4s",
        "  ✓ Follows the design system  theme guard clean",
        "passed · 4 passed",
        ""
      ].join("\n")
    );
  });

  it("lists a failed check's findings, a few of them, and exits CHECK_FAILED", async () => {
    stubFetch(
      { ...appRoutes(), [`GET ${CHECKS_PATH}`]: () => ({ status: 200, body: FAILED }) },
      calls
    );
    const run = runChecksShow(contextWith("user-token"), "acme/store", { timeoutSeconds: 600 });

    await expect(run).rejects.toMatchObject({
      code: ExitCode.CHECK_FAILED,
      message: "1 of 4 check(s) did not pass on build 9087d7b: Follows the design system"
    });
    const lines = printed().split("\n");
    expect(lines).toEqual([
      "acme/store · build 9087d7b · staging · attempt 2",
      "  ✓ Builds and validates       bundle valid",
      "  ✓ Its checks pass            3 of 3 passed",
      "  ✓ It loads, desk and phone   desk 1.2s · phone 1.4s",
      "  ✗ Follows the design system  src/index.css:68 restates --card",
      "      src/index.css:68  restates --card, which theme.css defines; remove it",
      "      src/index.css:70  restates --border, which theme.css defines; remove it",
      "      loads a second font stack",
      "      +1 more",
      "failed · 1 failed, 3 passed",
      ""
    ]);
    // Phase 1 changes nothing about promote: no line may say otherwise.
    expect(printed()).not.toMatch(/required|blocked|waiting on/i);
  });

  it("prints a running attempt and exits 0 without --wait", async () => {
    const running = body("running", [
      VALIDATION,
      check("functions", "Its checks pass", "running", null),
      check("loads", "It loads, desk and phone", "queued", null),
      DESIGN
    ]);
    stubFetch(
      { ...appRoutes(), [`GET ${CHECKS_PATH}`]: () => ({ status: 200, body: running }) },
      calls
    );
    await runChecksShow(contextWith("user-token"), "acme/store", { timeoutSeconds: 600 });

    expect(printed().split("\n")).toEqual([
      "acme/store · build 9087d7b · staging · attempt 2",
      "  ✓ Builds and validates       bundle valid",
      "  … Its checks pass            running",
      "  … It loads, desk and phone   queued",
      "  ✓ Follows the design system  theme guard clean",
      "running · 2 of 4 finished",
      ""
    ]);
  });

  it("says who ran the checks, with the link to the run when there is one", async () => {
    const claimed = {
      ...body("passed", [VALIDATION, FUNCTIONS, LOADS, DESIGN]),
      runner: {
        label: "GitHub Actions run #1482",
        url: "https://github.com/oxy-hq/oxygen-internal/actions/runs/123",
        started_at: "2026-11-20T10:01:05Z"
      },
      stalled: null
    };
    stubFetch(
      { ...appRoutes(), [`GET ${CHECKS_PATH}`]: () => ({ status: 200, body: claimed }) },
      calls
    );
    await runChecksShow(contextWith("user-token"), "acme/store", { timeoutSeconds: 600 });
    expect(printed().split("\n").slice(0, 3)).toEqual([
      "acme/store · build 9087d7b · staging · attempt 2",
      "  run by GitHub Actions run #1482 · https://github.com/oxy-hq/oxygen-internal/actions/runs/123",
      "  ✓ Builds and validates       bundle valid"
    ]);

    // By hand there is no run to link to.
    stdout.mockClear();
    stubFetch(
      {
        ...appRoutes(),
        [`GET ${CHECKS_PATH}`]: () => ({
          status: 200,
          body: { ...claimed, runner: { label: "oxyc, run by hand", url: null } }
        })
      },
      calls
    );
    await runChecksShow(contextWith("user-token"), "acme/store", { timeoutSeconds: 600 });
    expect(printed().split("\n")[1]).toBe("  run by oxyc, run by hand");
    expect(printed()).not.toMatch(/did not answer/);
  });

  it("says the runner did not answer when the attempt stalled, and still exits CHECK_FAILED", async () => {
    const waiting = "the runner did not answer in time";
    const stalled = (why: string, runner: unknown) => ({
      ...body("failed", [
        VALIDATION,
        check("functions", "Its checks pass", "error", waiting),
        check("loads", "It loads, desk and phone", "error", waiting),
        check("design_system", "Follows the design system", "error", waiting)
      ]),
      runner,
      stalled: why
    });
    stubFetch(
      {
        ...appRoutes(),
        [`GET ${CHECKS_PATH}`]: () => ({ status: 200, body: stalled("no_runner", null) })
      },
      calls
    );
    await expect(
      runChecksShow(contextWith("user-token"), "acme/store", { timeoutSeconds: 600 })
    ).rejects.toMatchObject({ code: ExitCode.CHECK_FAILED });
    // No runner claimed it, so there is no "run by" line; the stalled line is the last before the result.
    expect(printed().split("\n")).toEqual([
      "acme/store · build 9087d7b · staging · attempt 2",
      "  ✓ Builds and validates       bundle valid",
      `  ! Its checks pass            ${waiting}`,
      `  ! It loads, desk and phone   ${waiting}`,
      `  ! Follows the design system  ${waiting}`,
      "  ! the runner did not answer (no runner picked these checks up): run the checks again from the app's Builds page",
      "failed · 3 error, 1 passed",
      ""
    ]);

    stdout.mockClear();
    stubFetch(
      {
        ...appRoutes(),
        [`GET ${CHECKS_PATH}`]: () => ({
          status: 200,
          body: stalled("runner_stopped", { label: "GitHub Actions run #7", url: null })
        })
      },
      calls
    );
    await expect(
      runChecksShow(contextWith("user-token"), "acme/store", { timeoutSeconds: 600 })
    ).rejects.toMatchObject({ code: ExitCode.CHECK_FAILED });
    expect(printed()).toContain("  run by GitHub Actions run #7\n");
    expect(printed()).toContain(
      "  ! the runner did not answer (the runner stopped before it finished): run the checks again from the app's Builds page"
    );
  });

  it("says a build that was never checked was not, and exits 0", async () => {
    stubFetch(
      {
        ...appRoutes(),
        [`GET ${CHECKS_PATH}`]: () => ({ status: 200, body: body("none", [], null) })
      },
      calls
    );
    await runChecksShow(contextWith("user-token"), "acme/store", { timeoutSeconds: 600 });

    expect(printed()).toBe(
      [
        "acme/store · build 9087d7b · staging · not checked",
        "none · this build was not checked",
        ""
      ].join("\n")
    );
  });

  it("marks a skipped check and counts it apart from the passes", async () => {
    const skipped = body("passed", [
      VALIDATION,
      check("functions", "Its checks pass", "skipped", "the build declares no checks"),
      LOADS,
      DESIGN
    ]);
    stubFetch(
      { ...appRoutes(), [`GET ${CHECKS_PATH}`]: () => ({ status: 200, body: skipped }) },
      calls
    );
    await runChecksShow(contextWith("user-token"), "acme/store", { timeoutSeconds: 600 });

    const lines = printed().split("\n");
    expect(lines[2]).toBe("  - Its checks pass            the build declares no checks");
    expect(lines.at(-2)).toBe("passed · 3 passed, 1 skipped");
  });

  it("prints the server's body as-is with --json, and still exits CHECK_FAILED", async () => {
    stubFetch(
      { ...appRoutes(), [`GET ${CHECKS_PATH}`]: () => ({ status: 200, body: FAILED }) },
      calls
    );
    const run = runChecksShow(contextWith("user-token"), "acme/store", {
      timeoutSeconds: 600,
      json: true
    });

    await expect(run).rejects.toMatchObject({ code: ExitCode.CHECK_FAILED });
    const text = printed();
    // One object on one line, nothing else on stdout.
    expect(text.endsWith("\n")).toBe(true);
    expect(text.trimEnd().split("\n")).toHaveLength(1);
    expect(JSON.parse(text)).toEqual(FAILED);
  });

  it("reads --build by its build_id, without listing builds", async () => {
    const byBuildId = `/api/customer-apps/${APP_ID}/builds/9087d7b/checks`;
    stubFetch(
      {
        ...appRoutes(),
        [`GET ${byBuildId}`]: () => ({ status: 200, body: body("passed", [VALIDATION]) })
      },
      calls
    );
    await runChecksShow(contextWith("user-token"), "acme/store", {
      build: "9087d7b",
      timeoutSeconds: 600
    });

    expect(calls).toContain(`GET ${byBuildId}`);
    expect(calls).not.toContain(`GET /api/customer-apps/${APP_ID}/builds`);
  });

  it("asks for an earlier attempt with ?attempt=", async () => {
    stubFetch(
      {
        ...appRoutes(),
        [`GET ${CHECKS_PATH}?attempt=1`]: () => ({ status: 200, body: { ...FAILED, attempt: 1 } })
      },
      calls
    );
    const run = runChecksShow(contextWith("user-token"), "acme/store", {
      attempt: "1",
      timeoutSeconds: 600
    });

    await expect(run).rejects.toMatchObject({ code: ExitCode.CHECK_FAILED });
    expect(printed().split("\n")[0]).toBe(
      "acme/store · build 9087d7b · staging · attempt 1 (latest is 2)"
    );
  });

  it("refuses a malformed --attempt before any request", async () => {
    stubFetch(appRoutes(), calls);
    const run = runChecksShow(contextWith("user-token"), "acme/store", {
      attempt: "0",
      timeoutSeconds: 600
    });

    await expect(run).rejects.toMatchObject({ code: ExitCode.USAGE });
    expect(calls).toEqual([]);
  });

  it("with --wait, polls the same build until the checks finish, then exits by the result", async () => {
    const answers = [
      body("running", [VALIDATION, check("loads", "It loads, desk and phone", "running", null)]),
      body("running", [VALIDATION, check("loads", "It loads, desk and phone", "running", null)]),
      FAILED
    ];
    let reads = 0;
    stubFetch(
      {
        ...appRoutes(),
        [`GET ${CHECKS_PATH}`]: () => ({ status: 200, body: answers[Math.min(reads++, 2)] })
      },
      calls
    );
    const run = runChecksShow(contextWith("user-token"), "acme/store", {
      wait: true,
      timeoutSeconds: 5,
      pollMs: 1
    });

    await expect(run).rejects.toMatchObject({ code: ExitCode.CHECK_FAILED });
    expect(reads).toBe(3);
    // Staging's build was looked up once; every poll re-read that same row.
    expect(calls.filter((c) => c === `GET /api/customer-apps/${APP_ID}/builds`)).toHaveLength(1);
    // Only the final answer is printed.
    expect(printed().split("\n").at(-2)).toBe("failed · 1 failed, 3 passed");
  });

  it("with --wait, exits UNAVAILABLE when --timeout passes with checks still running", async () => {
    const running = body("running", [check("loads", "It loads, desk and phone", "running", null)]);
    stubFetch(
      { ...appRoutes(), [`GET ${CHECKS_PATH}`]: () => ({ status: 200, body: running }) },
      calls
    );
    const run = runChecksShow(contextWith("user-token"), "acme/store", {
      wait: true,
      timeoutSeconds: 0.05,
      pollMs: 5
    });

    await expect(run).rejects.toMatchObject({ code: ExitCode.UNAVAILABLE });
    expect(printed().split("\n").at(-2)).toBe("running · 0 of 1 finished");
  });

  it("is NOT_FOUND when staging serves no build", async () => {
    stubFetch(
      {
        ...appRoutes(),
        [`GET /api/customer-apps/${APP_ID}/builds`]: () => ({
          status: 200,
          body: { promoted_at: null, promoted_by_email: null, builds: [] }
        })
      },
      calls
    );
    const run = runChecksShow(contextWith("user-token"), "acme/store", { timeoutSeconds: 600 });

    await expect(run).rejects.toMatchObject({ code: ExitCode.NOT_FOUND });
    await expect(run).rejects.toThrow("staging serves no build of acme/store");
  });

  it("takes a publish token by app UUID, without the app listing", async () => {
    stubFetch(
      {
        ...appRoutes(),
        [`GET ${CHECKS_PATH}`]: () => ({ status: 200, body: body("passed", [VALIDATION]) })
      },
      calls
    );
    await runChecksShow(contextWith("oxypublish_stored"), APP_ID, { timeoutSeconds: 600 });

    expect(calls.some((c) => c.startsWith("GET /api/customer-apps?"))).toBe(false);
    expect(calls).toContain(`GET ${CHECKS_PATH}`);
  });

  it("refuses a publish token's slug, and a sandbox agent token, before any request", async () => {
    stubFetch(appRoutes(), calls);
    await expect(
      runChecksShow(contextWith("oxypublish_stored"), "acme/store", { timeoutSeconds: 600 })
    ).rejects.toMatchObject({ code: ExitCode.USAGE });
    const sandbox = runChecksShow(contextWith("oxy_sbx_abc"), "acme/store", {
      timeoutSeconds: 600
    });
    await expect(sandbox).rejects.toBeInstanceOf(CliError);
    await expect(sandbox).rejects.toMatchObject({ code: ExitCode.USAGE });
    expect(calls).toEqual([]);
  });
});
