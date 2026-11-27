/**
 * `oxyc promote` where the app's org holds promotes on a build's checks: the
 * plan's lines about them, the three blockers' words, and `--override-checks`.
 * `globalThis.fetch` is a route table (`testing/stub-fetch.ts`), as in
 * `promote.test.ts`; the plan and the context are `testing/promote-fixtures.ts`.
 *
 * The wait for production's own check after the move:
 * `promote-production-check.test.ts`.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  APPS,
  BUILD,
  bodyOf,
  capture,
  checks,
  fakeContext,
  LIVE,
  moving,
  PROMOTE,
  plan,
  row,
  TARGET
} from "../testing/promote-fixtures.js";
import { stubFetch } from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import { planLines, runPromote } from "./promote.js";

describe("oxyc promote: the plan's checks", () => {
  let io: ReturnType<typeof capture>;
  beforeEach(() => {
    io = capture();
    vi.stubEnv("OXYC_QUIET", "1");
  });
  afterEach(() => {
    io.restore();
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
  });

  it("--dry-run prints one line per check, the required ones marked, and what changed", async () => {
    stubFetch(TARGET, { ...APPS, [PROMOTE]: () => ({ status: 200, body: plan() }) });
    await runPromote(fakeContext(), "acme/store", { dryRun: true });
    const lines = io.out().split("\n");
    const from = lines.indexOf("  checks (attempt 2): passed");
    expect(lines.slice(from, from + 8)).toEqual([
      "  checks (attempt 2): passed",
      "    ✓           Builds and validates       bundle valid",
      "    ✓ required  Its checks pass            3 of 3 passed",
      "    ✓ required  It loads, desk and phone   ready in 0.9s desk, 1.1s phone",
      "    ✓ required  Follows the design system  SDK 2.31.0 · guard clean",
      "    ✓ required  Every screen opens         14 screens, 0 errors",
      "    ✓           What changed               4 of 28 screens differ",
      "    4 of 28 screens differ · not seen by a person yet"
    ]);
  });

  it("prints the same lines before acting, with the summary note marked as a model's", async () => {
    const noted = plan({
      checks: checks({
        changes: { changed: 4, compared: 28, seen: true, seen_required: true },
        summary: {
          author: "model",
          model: "claude-sonnet-4-5",
          text: "Adds a reorder screen.\nRisk: the stores table moved a column."
        }
      })
    });
    stubFetch(TARGET, moving(noted));
    await runPromote(fakeContext(), "acme/store", { yes: true, noWait: true });
    expect(io.out()).toContain(
      [
        "    4 of 28 screens differ · seen by a person",
        "    summary by a model (claude-sonnet-4-5):",
        "      Adds a reorder screen.",
        "      Risk: the stores table moved a column.",
        ""
      ].join("\n")
    );
    expect(io.out()).toContain("production now serves build b-7");
  });

  it("says nothing about checks where the plan carries none: an org with no gate reads as before", () => {
    const lines = planLines(plan({ checks: null }), "acme/store").join("\n");
    expect(lines).not.toMatch(/checks|required|screens differ/);
    expect(planLines(plan({ checks: undefined }))).toEqual(planLines(plan({ checks: null })));
  });

  it("says no checks have run, and marks one still running or failed", () => {
    expect(
      planLines(plan({ checks: checks({ attempt: null, state: "none", rows: [] }) }))
    ).toContain("  checks: none have run on this build");
    const lines = planLines(
      plan({
        checks: checks({
          state: "failed",
          rows: [
            row(
              "design_system",
              "Follows the design system",
              "failed",
              "src/index.css:68 restates --card",
              true
            ),
            { kind: "smoke", label: null, status: "running", summary: null, required: true }
          ],
          changes: null
        })
      })
      // biome-ignore lint/suspicious/noControlCharactersInRegex: stripping ANSI colour
    ).map((line) => line.replace(/\u001b\[[0-9;]*m/g, ""));
    expect(lines).toContain("  checks (attempt 2): failed");
    expect(lines).toContain(
      "    ✗ required  Follows the design system  src/index.css:68 restates --card"
    );
    // A row the server sent without a label still reads by its kind's words.
    expect(lines).toContain("    … required  Every screen opens         running");
  });

  it.each([
    [
      "checks_not_passed",
      "Follows the design system did not pass",
      "fix what the checks found and publish again (`oxyc checks show acme/store --build b-7` prints each finding), or run them again from the app's Builds page"
    ],
    [
      "checks_running",
      "Every screen opens is still running",
      "wait for the checks, then promote again: `oxyc checks show acme/store --build b-7 --wait`"
    ],
    [
      "changes_not_seen",
      "4 screens changed and nobody has marked them seen",
      "open the app's Builds page, look at the screens that differ and mark them seen, then promote again"
    ]
  ])(
    "reads the %s blocker as what to do next, and sends no act (exit 6)",
    async (code, message, next) => {
      const blocked = plan({ blockers: [{ code, message }] });
      const calls = stubFetch(TARGET, {
        ...APPS,
        [PROMOTE]: () => ({ status: 200, body: blocked })
      });
      const failure = await runPromote(fakeContext(), "acme/store", { yes: true }).catch(
        (cause: unknown) => cause
      );
      const override = '; or promote past them with --override-checks "<why>" (audited)';
      expect(io.out()).toContain(`  blocked ${code}: ${message}\n    next: ${next}${override}\n`);
      expect(failure).toBeInstanceOf(CliError);
      expect(failure).toMatchObject({
        code: ExitCode.REQUEST,
        message: `promote refused: ${code}`,
        remedy: `${next}${override}`,
        serverCode: code
      });
      expect(calls.filter((c) => c.method === "POST")).toHaveLength(1);
    }
  );

  it("offers the override only to a caller the plan says may use it", async () => {
    const blocked = plan({
      checks: checks({ overridable: false, unmet: ["design_system"] }),
      blockers: [{ code: "checks_not_passed", message: "Follows the design system did not pass" }]
    });
    stubFetch(TARGET, { ...APPS, [PROMOTE]: () => ({ status: 200, body: blocked }) });
    const failure = await runPromote(fakeContext(), "acme/store", { yes: true }).catch(
      (cause: unknown) => cause
    );
    expect((failure as CliError).remedy).not.toContain("--override-checks");
    expect(io.out()).not.toContain("--override-checks");
  });
});

describe("oxyc promote --override-checks", () => {
  let io: ReturnType<typeof capture>;
  beforeEach(() => {
    io = capture();
    vi.stubEnv("OXYC_QUIET", "1");
  });
  afterEach(() => {
    io.restore();
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
  });

  it.each(["", "   ", "too short", "  nine ch  "])(
    "refuses the reason %j before any request (exit 2)",
    async (reason) => {
      const calls = stubFetch(TARGET, {});
      const failure = await runPromote(fakeContext(), "acme/store", {
        yes: true,
        overrideChecks: reason
      }).catch((cause: unknown) => cause);
      expect(failure).toMatchObject({
        code: ExitCode.USAGE,
        message:
          "--override-checks needs the reason, in at least 10 characters: it is kept in the audit log"
      });
      expect(calls).toEqual([]);
    }
  );

  it("sends override_checks on the plan and on the act, and shows what it cleared", async () => {
    const reason = "hotfix for the checkout outage";
    const overridden = plan({
      checks: checks({ state: "failed", unmet: ["design_system"], overridden: { reason } })
    });
    const calls = stubFetch(TARGET, moving(overridden));
    await runPromote(fakeContext(), "acme/store", {
      yes: true,
      noWait: true,
      // Trimmed before it is sent.
      overrideChecks: `  ${reason}  `
    });
    const posts = calls.filter((c) => c.method === "POST").map(bodyOf);
    expect(posts).toHaveLength(2);
    expect(posts[0]).toEqual({
      from: "staging",
      confirm_schedule_removals: [],
      override_checks: { reason },
      dry_run: true
    });
    expect(posts[1]).toEqual({
      from: "staging",
      confirm_schedule_removals: [],
      override_checks: { reason },
      build: BUILD,
      expect_current: LIVE
    });
    expect(io.out()).toContain(`    overridden the checks do not hold this promote: ${reason}\n`);
  });

  it("sends no override_checks when the flag is absent", async () => {
    const calls = stubFetch(TARGET, moving(plan()));
    await runPromote(fakeContext(), "acme/store", { yes: true, noWait: true });
    for (const post of calls.filter((c) => c.method === "POST")) {
      expect(bodyOf(post)).not.toHaveProperty("override_checks");
    }
  });

  it("goes with a promote to production only (exit 2, before any request)", async () => {
    const calls = stubFetch(TARGET, {});
    const failure = await runPromote(fakeContext(), "acme/store", {
      from: "dev-a1",
      overrideChecks: "a reason long enough"
    }).catch((cause: unknown) => cause);
    expect(failure).toMatchObject({
      code: ExitCode.USAGE,
      message: "--override-checks goes with a promote to production"
    });
    expect(calls).toEqual([]);
  });

  it("answers the server's refusal of an override with its code", async () => {
    stubFetch(TARGET, {
      ...APPS,
      [PROMOTE]: () => ({
        status: 403,
        body: { code: "override_not_allowed", message: "an owner or admin of the org overrides" }
      })
    });
    const failure = await runPromote(fakeContext(), "acme/store", {
      yes: true,
      overrideChecks: "a reason long enough"
    }).catch((cause: unknown) => cause);
    expect(failure).toMatchObject({ code: ExitCode.AUTH, serverCode: "override_not_allowed" });
  });
});
