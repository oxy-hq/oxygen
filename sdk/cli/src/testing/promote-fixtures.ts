/**
 * What the tests of `oxyc promote` on a gated app share: a staff context, a
 * plan that carries the build's checks, a production attempt's body, and the
 * two output streams caught. Test support only: nothing in the shipped bundle
 * imports this.
 */

import { vi } from "vitest";
import type { Plan } from "../commands/promote.js";
import type { PlanChecks, ProductionWaitDeps } from "../commands/promote-checks.js";
import type { Context } from "../context/resolve.js";
import type { Call, Routes } from "./stub-fetch.js";

export const TARGET = "https://oxy.test";
export const APP_ID = "a1a1a1a1-2222-3333-4444-555555555555";
export const BUILD = "b0b0b0b0-2222-3333-4444-555555555555";
export const LIVE = "c0c0c0c0-2222-3333-4444-555555555555";
export const PROMOTE = `POST /api/customer-apps/${APP_ID}/environments/production/promote`;
export const PRODUCTION_CHECKS = `GET /api/customer-apps/${APP_ID}/builds/${BUILD}/checks?environment=production`;

export function fakeContext(bearer = "tok"): Context {
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

export const APPS: Routes = {
  "GET /api/customer-apps?limit=100&offset=0": () => ({
    status: 200,
    body: { items: [{ id: APP_ID, slug: "store", org_slug: "acme" }], next_offset: null }
  })
};

export const row = (
  kind: string,
  label: string,
  status: string,
  summary: string,
  required: boolean
) => ({
  kind,
  label,
  status,
  summary,
  required
});

export function checks(overrides: Partial<PlanChecks> = {}): PlanChecks {
  return {
    required: ["functions", "loads", "design_system", "smoke"],
    attempt: 2,
    state: "passed",
    rows: [
      row("validation", "Builds and validates", "passed", "bundle valid", false),
      row("functions", "Its checks pass", "passed", "3 of 3 passed", true),
      row("loads", "It loads, desk and phone", "passed", "ready in 0.9s desk, 1.1s phone", true),
      row("design_system", "Follows the design system", "passed", "SDK 2.31.0 · guard clean", true),
      row("smoke", "Every screen opens", "passed", "14 screens, 0 errors", true),
      row("changes", "What changed", "passed", "4 of 28 screens differ", false)
    ],
    unmet: [],
    running: [],
    changes: { changed: 4, compared: 28, seen: false, seen_required: false },
    summary: null,
    overridable: true,
    overridden: null,
    ...overrides
  };
}

export function plan(overrides: Partial<Plan> = {}): Plan {
  return {
    action: "promote",
    app_id: APP_ID,
    from: "staging",
    to: "production",
    build: {
      id: BUILD,
      build_id: "b-7",
      commit_sha: null,
      source_branch: null,
      source_repo: null,
      published_by: null,
      created_at: "2026-11-22T09:00:00Z",
      published_to: "staging"
    },
    current: { id: LIVE, build_id: "b-6" },
    expect_current: LIVE,
    served_since: null,
    served_for_seconds: null,
    functions: { added: [], removed: [], changed: [] },
    schedules: null,
    migrations: { when: "before_move", oltp: [], airhouse: [] },
    required_secrets_unset: [],
    blockers: [],
    warnings: [],
    dry_run: true,
    moved: false,
    migrations_applied: null,
    checks: checks(),
    ...overrides
  };
}

/** A production attempt's body, as `GET …/checks?environment=production` answers it. */
export function production(state: string, status: string, summary: string | null) {
  return {
    app_id: APP_ID,
    build: { id: BUILD, build_id: "b-7", created_at: "2026-11-22T09:00:00Z" },
    attempt: state === "none" ? null : 1,
    environment: "production",
    state,
    checks:
      state === "none"
        ? []
        : [
            {
              kind: "production_smoke",
              label: "Production opens after promote",
              status,
              advisory: false,
              summary,
              detail: {
                findings:
                  status === "failed"
                    ? [
                        {
                          severity: "error",
                          message: "/stores at desk: TypeError: x",
                          at: null,
                          rule: null
                        }
                      ]
                    : []
              },
              attempt: 1,
              duration_ms: null
            }
          ],
    attempts: state === "none" ? [] : [{ attempt: 1, state, created_at: "2026-11-22T09:05:00Z" }]
  };
}

/** No real wait: the poll and the grace are as short as a test can make them. */
export const QUICK: Partial<ProductionWaitDeps> = {
  pollMs: 1,
  timeoutMs: 400,
  graceMs: 0,
  sleep: async () => undefined
};

export function capture(): { out: () => string; err: () => string; restore: () => void } {
  let out = "";
  let err = "";
  // biome-ignore lint/suspicious/noControlCharactersInRegex: stripping ANSI colour
  const plain = (text: string) => text.replace(/\u001b\[[0-9;]*m/g, "");
  const stdout = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
    out += String(chunk);
    return true;
  });
  const stderr = vi.spyOn(process.stderr, "write").mockImplementation((chunk) => {
    err += String(chunk);
    return true;
  });
  return {
    out: () => plain(out),
    err: () => plain(err),
    restore: () => {
      stdout.mockRestore();
      stderr.mockRestore();
    }
  };
}

export const bodyOf = (call: Call | undefined) =>
  JSON.parse(call?.body ?? "{}") as Record<string, unknown>;

/** A promote whose act moves production: the plan first, then the move. */
export function moving(planned: Plan): Routes {
  return {
    ...APPS,
    [PROMOTE]: (call) =>
      bodyOf(call).dry_run
        ? { status: 200, body: planned }
        : { status: 200, body: { ...planned, dry_run: false, moved: true } }
  };
}
