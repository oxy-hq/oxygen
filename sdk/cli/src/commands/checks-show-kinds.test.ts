/**
 * `oxyc checks show` on the kinds the promote gate adds: every screen opens,
 * what changed (and whether a person has seen it), production's check after a
 * promote (`--environment production`), and a model's notes, marked as a
 * model's. Same harness as `checks-show.test.ts`: `fetch` is a route table.
 */

import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, type MockInstance, vi } from "vitest";
import { resetOidcExchanges } from "../auth/oidc.js";
import { type Context, createContext } from "../context/resolve.js";
import { type Routes, stubFetch } from "../testing/stub-fetch.js";
import { ExitCode } from "../util/errors.js";
import { type BuildCheck, type BuildChecks, runChecksShow } from "./checks-show.js";

const TARGET = "https://oxy.test";
const APP_ID = "a1a1a1a1-2222-3333-4444-555555555555";
/** The build staging serves, and the one production serves. */
const STAGING_ROW = "b2b2b2b2-2222-3333-4444-555555555555";
const PRODUCTION_ROW = "c3c3c3c3-2222-3333-4444-555555555555";
const checksOf = (row: string) => `/api/customer-apps/${APP_ID}/builds/${row}/checks`;

function contextWith(bearer: string): Context {
  vi.stubEnv("OXY_TOKEN", bearer);
  vi.stubEnv("OXY_API_KEY", "");
  vi.stubEnv("OXY_SERVICE_ACCOUNT", "");
  vi.stubEnv("OXY_CREDENTIALS_PATH", join(tmpdir(), "oxyc-no-such-credentials.json"));
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_URL", "");
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "");
  return createContext({ env: "production", target: TARGET }, tmpdir());
}

const build = (id: string, buildId: string, isDraft: boolean) => ({
  id,
  build_id: buildId,
  created_at: "2026-11-20T10:00:00Z",
  is_draft: isDraft,
  is_published: !isDraft,
  published_by_email: null,
  published_via: null,
  source_repo: null,
  commit_sha: null,
  source_branch: null
});

const APP: Routes = {
  "GET /api/customer-apps?limit=100&offset=0": () => ({
    status: 200,
    body: { items: [{ id: APP_ID, slug: "store", org_slug: "acme" }], next_offset: null }
  }),
  [`GET /api/customer-apps/${APP_ID}/builds`]: () => ({
    status: 200,
    body: {
      promoted_at: null,
      promoted_by_email: null,
      builds: [build(PRODUCTION_ROW, "60e2f19", false), build(STAGING_ROW, "9087d7b", true)]
    }
  })
};

function check(
  kind: string,
  status: string,
  summary: string | null,
  over: Partial<BuildCheck> = {}
): BuildCheck {
  return {
    kind,
    // The server's label; a row without one falls back to this command's own words.
    label: null,
    status,
    advisory: false,
    summary,
    detail: { findings: [] },
    attempt: 2,
    duration_ms: null,
    ...over
  };
}

function body(
  environment: string,
  state: string,
  checks: BuildCheck[],
  over: Partial<BuildChecks> = {}
): BuildChecks {
  const row = environment === "production" ? PRODUCTION_ROW : STAGING_ROW;
  return {
    app_id: APP_ID,
    build: {
      id: row,
      build_id: environment === "production" ? "60e2f19" : "9087d7b",
      created_at: "2026-11-20T10:00:00Z"
    },
    attempt: 2,
    environment,
    state,
    checks,
    attempts: [{ attempt: 2, state, created_at: "2026-11-20T10:01:00Z" }],
    ...over
  };
}

describe("oxyc checks show: the gate's kinds", () => {
  let stdout: MockInstance;

  function printed(): string[] {
    // oxlint-disable-next-line typescript/no-base-to-string -- every write here is a string
    const text = stdout.mock.calls.map(([chunk]) => String(chunk)).join("");
    // biome-ignore lint/suspicious/noControlCharactersInRegex: stripping ANSI colour
    return text.replace(/\u001b\[[0-9;]*m/g, "").split("\n");
  }

  beforeEach(() => {
    resetOidcExchanges();
    stdout = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    vi.stubEnv("OXYC_QUIET", "1");
  });

  afterEach(() => {
    stdout.mockRestore();
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
  });

  it("renders the new kinds like the others, and says whether a person has seen what changed", async () => {
    const staging = body("staging", "passed", [
      check("validation", "passed", "bundle valid"),
      check("smoke", "passed", "14 screens, 0 errors"),
      check("changes", "passed", "4 of 28 screens differ", {
        advisory: true,
        detail: { findings: [], compared: 28, changed: 4, seen: null }
      })
    ]);
    stubFetch(TARGET, {
      ...APP,
      [`GET ${checksOf(STAGING_ROW)}`]: () => ({ status: 200, body: staging })
    });
    await runChecksShow(contextWith("user-token"), "acme/store", { timeoutSeconds: 600 });
    expect(printed()).toEqual([
      "acme/store · build 9087d7b · staging · attempt 2",
      "  ✓ Builds and validates  bundle valid",
      "  ✓ Every screen opens    14 screens, 0 errors",
      "  ✓ What changed          4 of 28 screens differ · not seen by a person yet (advisory)",
      "passed · 2 passed",
      ""
    ]);
  });

  it("names the linked screens the checks did not open, under the row that opened the rest", async () => {
    const smoke = (detail: Record<string, unknown>) =>
      body("staging", "passed", [
        check("smoke", "passed", "2 screens, 0 errors", { detail: { findings: [], ...detail } })
      ]);
    const lines = async (answer: BuildChecks) => {
      stdout.mockClear();
      vi.unstubAllGlobals();
      stubFetch(TARGET, {
        ...APP,
        [`GET ${checksOf(STAGING_ROW)}`]: () => ({ status: 200, body: answer })
      });
      await runChecksShow(contextWith("user-token"), "acme/store", { timeoutSeconds: 600 });
      return printed().slice(1, -2);
    };
    const paths = ["plans", "stores/12", "settings", "a", "b", "c"];
    expect(await lines(smoke({ screens_unlisted: paths, screens_unlisted_total: 9 }))).toEqual([
      "  ✓ Every screen opens  2 screens, 0 errors",
      "      ! 9 linked screens not opened: /plans, /stores/12, /settings, /a, /b, +4 more (declare the ones that matter in checks.screens)"
    ]);
    expect(await lines(smoke({ screens_unlisted: ["plans"], screens_unlisted_total: 1 }))).toEqual([
      "  ✓ Every screen opens  2 screens, 0 errors",
      "      ! 1 linked screen not opened: /plans (declare the ones that matter in checks.screens)"
    ]);
    // None named (or a runner from before it named any): the row alone.
    expect(await lines(smoke({}))).toEqual(["  ✓ Every screen opens  2 screens, 0 errors"]);
  });

  it("names who saw the changed screens, and says nothing of seeing when none differ or it was skipped", async () => {
    const changes = (
      detail: Record<string, unknown>,
      status = "passed",
      summary: string | null = null
    ) =>
      body("staging", "passed", [
        check("changes", status, summary, { advisory: true, detail: { findings: [], ...detail } })
      ]);
    const line = async (answer: BuildChecks) => {
      stdout.mockClear();
      vi.unstubAllGlobals();
      stubFetch(TARGET, {
        ...APP,
        [`GET ${checksOf(STAGING_ROW)}`]: () => ({ status: 200, body: answer })
      });
      await runChecksShow(contextWith("user-token"), "acme/store", { timeoutSeconds: 600 });
      return printed()[1];
    };
    expect(
      await line(
        changes({
          compared: 28,
          changed: 4,
          seen: { by: "ana@oxy.tech", at: "2026-11-22T10:00:00Z" }
        })
      )
    ).toBe("  ✓ What changed  4 of 28 screens differ · seen by ana@oxy.tech (advisory)");
    expect(await line(changes({ compared: 28, changed: 0, seen: null }))).toBe(
      "  ✓ What changed  0 of 28 screens differ (advisory)"
    );
    expect(
      await line(
        changes(
          { compared: 0, changed: 0, seen: null },
          "skipped",
          "nothing to compare: production serves no other build"
        )
      )
    ).toBe("  - What changed  nothing to compare: production serves no other build (advisory)");
  });

  it("prints a model's notes under their check, marked as a model's, and the build's summary note", async () => {
    const noted = body(
      "staging",
      "failed",
      [
        check("design_system", "failed", "assets/index.css:68 restates --card", {
          detail: {
            findings: [
              {
                severity: "error",
                message: "restates --card, which theme.css defines; remove it",
                at: "assets/index.css:68",
                rule: "restates-sdk-token"
              }
            ]
          },
          notes: [
            {
              author: "model",
              model: "claude-sonnet-4-5",
              text: "The app redefines --card in its own :root block; delete that declaration."
            }
          ]
        }),
        check("changes", "passed", "4 of 28 screens differ", {
          advisory: true,
          detail: { findings: [], compared: 28, changed: 4, seen: null },
          notes: [
            {
              author: "model",
              model: null,
              text: "Looks intended: /reorder.\nLook here first: /stores at phone."
            }
          ]
        }),
        check("design_audit", "passed", "2 notes", {
          advisory: true,
          notes: [{ author: "rule", text: "A 13px label set by hand." }]
        })
      ],
      { summary: { author: "model", model: "claude-sonnet-4-5", text: "Adds a reorder screen." } }
    );
    stubFetch(TARGET, {
      ...APP,
      [`GET ${checksOf(STAGING_ROW)}`]: () => ({ status: 200, body: noted })
    });
    await expect(
      runChecksShow(contextWith("user-token"), "acme/store", { timeoutSeconds: 600 })
    ).rejects.toMatchObject({ code: ExitCode.CHECK_FAILED });
    expect(printed()).toEqual([
      "acme/store · build 9087d7b · staging · attempt 2",
      "  ✗ Follows the design system  assets/index.css:68 restates --card",
      "      assets/index.css:68  restates --card, which theme.css defines; remove it",
      "      note by a model (claude-sonnet-4-5): The app redefines --card in its own :root block; delete that declaration.",
      "  ✓ What changed               4 of 28 screens differ · not seen by a person yet (advisory)",
      "      note by a model:",
      "        Looks intended: /reorder.",
      "        Look here first: /stores at phone.",
      "  ✓ Design audit               2 notes (advisory)",
      "      note by a rule: A 13px label set by hand.",
      "  summary by a model (claude-sonnet-4-5): Adds a reorder screen.",
      // The roll-up counts the checks that hold a verdict: notes and advisory rows do not.
      "failed · 1 failed",
      ""
    ]);
  });

  it("--environment production refuses a deployment that answers staging's checks instead", async () => {
    // An older server ignores `environment` and answers the build's staging rows.
    const stagingRows = body("staging", "passed", [check("loads", "passed", "ready in 1.2s")]);
    stubFetch(TARGET, {
      ...APP,
      [`GET ${checksOf(PRODUCTION_ROW)}?environment=production`]: () => ({
        status: 200,
        body: stagingRows
      })
    });
    await expect(
      runChecksShow(contextWith("user-token"), "acme/store", {
        environment: "production",
        timeoutSeconds: 600
      })
    ).rejects.toMatchObject({
      code: ExitCode.FAILURE,
      message: "the platform answered a staging attempt, not the production one this asked for"
    });
    // Staging's rows are never printed as production's.
    expect(printed().join("")).toBe("");
  });

  it("--environment production reads the check that followed a promote, on the build production serves", async () => {
    const after = body("production", "failed", [
      check("production_smoke", "failed", "1 new error on 1 of 14 screens", {
        detail: {
          findings: [
            {
              severity: "error",
              message: "/stores at desk: TypeError: rows is undefined",
              at: null,
              rule: null
            }
          ]
        }
      })
    ]);
    const calls = stubFetch(TARGET, {
      ...APP,
      [`GET ${checksOf(PRODUCTION_ROW)}?environment=production`]: () => ({
        status: 200,
        body: after
      })
    });
    await expect(
      runChecksShow(contextWith("user-token"), "acme/store", {
        environment: "production",
        timeoutSeconds: 600
      })
    ).rejects.toMatchObject({
      code: ExitCode.CHECK_FAILED,
      message: "1 of 1 check(s) did not pass on build 60e2f19: Production opens after promote"
    });
    // The published row, not staging's draft; and never staging's attempts.
    expect(calls.map((c) => c.path)).toContain(
      `${checksOf(PRODUCTION_ROW)}?environment=production`
    );
    expect(calls.some((c) => c.path.startsWith(checksOf(STAGING_ROW)))).toBe(false);
    expect(printed()).toEqual([
      "acme/store · build 60e2f19 · production · attempt 2",
      "  ✗ Production opens after promote  1 new error on 1 of 14 screens",
      "      /stores at desk: TypeError: rows is undefined",
      "failed · 1 failed",
      ""
    ]);
  });

  it("names the attempt and the environment together, and staging by leaving it out", async () => {
    const calls = stubFetch(TARGET, {
      ...APP,
      [`GET ${checksOf("9087d7b")}?attempt=1&environment=production`]: () => ({
        status: 200,
        body: body("production", "passed", [
          check("production_smoke", "passed", "14 screens, no new errors")
        ])
      }),
      [`GET ${checksOf("9087d7b")}?attempt=1`]: () => ({
        status: 200,
        body: body("staging", "passed", [check("smoke", "passed", "14 screens, 0 errors")])
      })
    });
    const show = (environment?: string) =>
      runChecksShow(contextWith("user-token"), "acme/store", {
        build: "9087d7b",
        attempt: "1",
        environment,
        timeoutSeconds: 600
      });
    await show("production");
    await show("staging");
    await show();
    expect(calls.filter((c) => c.path.includes("/checks")).map((c) => c.path)).toEqual([
      `${checksOf("9087d7b")}?attempt=1&environment=production`,
      `${checksOf("9087d7b")}?attempt=1`,
      `${checksOf("9087d7b")}?attempt=1`
    ]);
  });

  it("refuses any other environment before any request (exit 2)", async () => {
    const calls = stubFetch(TARGET, APP);
    await expect(
      runChecksShow(contextWith("user-token"), "acme/store", {
        environment: "dev-a1",
        timeoutSeconds: 600
      })
    ).rejects.toMatchObject({
      code: ExitCode.USAGE,
      message: '--environment must be staging or production, not "dev-a1"'
    });
    expect(calls).toEqual([]);
  });

  it("says production serves no build when it serves none (exit 5)", async () => {
    stubFetch(TARGET, {
      ...APP,
      [`GET /api/customer-apps/${APP_ID}/builds`]: () => ({
        status: 200,
        body: {
          promoted_at: null,
          promoted_by_email: null,
          builds: [build(STAGING_ROW, "9087d7b", true)]
        }
      })
    });
    await expect(
      runChecksShow(contextWith("user-token"), "acme/store", {
        environment: "production",
        timeoutSeconds: 600
      })
    ).rejects.toMatchObject({
      code: ExitCode.NOT_FOUND,
      message: "production serves no build of acme/store"
    });
  });
});
