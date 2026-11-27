/**
 * `oxyc promote`, after the move: where the plan says the move is checked,
 * the command waits for production's own check ("Production opens after
 * promote"), prints it, and exits 9 when it failed. `--no-wait` skips that.
 * Same stand-ins as `promote-checks.test.ts`.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  APP_ID,
  capture,
  fakeContext,
  moving,
  PRODUCTION_CHECKS,
  PROMOTE,
  plan,
  production,
  QUICK,
  TARGET
} from "../testing/promote-fixtures.js";
import { stubFetch } from "../testing/stub-fetch.js";
import { type CliError, ExitCode } from "../util/errors.js";
import { type Plan, runPromote, runRollback } from "./promote.js";

describe("oxyc promote: production's check after the move", () => {
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

  it("waits for the production attempt's verdict, prints it, and exits 0 when it passed", async () => {
    let reads = 0;
    const calls = stubFetch(TARGET, {
      ...moving(plan()),
      [PRODUCTION_CHECKS]: () => {
        reads += 1;
        // Not queued yet, then running, then its verdict.
        if (reads === 1) return { status: 200, body: production("none", "", null) };
        if (reads === 2) return { status: 200, body: production("running", "running", null) };
        return { status: 200, body: production("passed", "passed", "14 screens, no new errors") };
      }
    });
    await runPromote(fakeContext(), "acme/store", { yes: true }, { ...QUICK, graceMs: 60_000 });
    expect(reads).toBe(3);
    // The move first, then the attempt read by the promoted build's row id.
    const order = calls.map((c) => `${c.method} ${c.path}`);
    expect(order.slice(-4)).toEqual([
      PROMOTE,
      PRODUCTION_CHECKS,
      PRODUCTION_CHECKS,
      PRODUCTION_CHECKS
    ]);
    expect(io.out()).toContain(
      [
        "production now serves build b-7",
        "acme/store · build b-7 · production · attempt 1",
        "  ✓ Production opens after promote  14 screens, no new errors",
        "passed · 1 passed",
        ""
      ].join("\n")
    );
  });

  it("exits 9 when production's check failed, saying how to move it back", async () => {
    stubFetch(TARGET, {
      ...moving(plan()),
      [PRODUCTION_CHECKS]: () => ({
        status: 200,
        body: production("failed", "failed", "1 new error on 1 of 14 screens")
      })
    });
    const failure = await runPromote(fakeContext(), "acme/store", { yes: true }, QUICK).catch(
      (cause: unknown) => cause
    );
    expect(failure).toMatchObject({
      code: ExitCode.CHECK_FAILED,
      message: "production serves build b-7, and its check after the promote failed",
      remedy: "oxyc rollback acme/store --app-env production"
    });
    expect(io.out()).toContain(
      [
        "  ✗ Production opens after promote  1 new error on 1 of 14 screens",
        "      /stores at desk: TypeError: x",
        "failed · 1 failed",
        ""
      ].join("\n")
    );
  });

  it("--no-wait reads nothing after the move", async () => {
    const calls = stubFetch(TARGET, moving(plan()));
    await runPromote(fakeContext(), "acme/store", { yes: true, noWait: true }, QUICK);
    expect(calls.filter((c) => c.method === "GET" && c.path.includes("/checks"))).toEqual([]);
    expect(io.out()).toContain("production now serves build b-7");
  });

  it("waits for nothing where the plan carries no checks, on a promote to staging, or on a rollback", async () => {
    const toStaging = `POST /api/customer-apps/${APP_ID}/environments/staging/promote`;
    const rollback = `POST /api/customer-apps/${APP_ID}/environments/production/rollback`;
    const calls = stubFetch(TARGET, {
      ...moving(plan({ checks: null })),
      [toStaging]: () => ({
        status: 200,
        body: plan({ to: "staging", from: "dev-a1", dry_run: false, moved: true })
      }),
      [rollback]: () => ({
        status: 200,
        body: plan({ action: "rollback", from: null, dry_run: false, moved: true, checks: null })
      })
    });
    await runPromote(fakeContext(), "acme/store", { yes: true }, QUICK);
    await runPromote(fakeContext(), "acme/store", { yes: true, from: "dev-a1" }, QUICK);
    await runRollback(fakeContext(), "acme/store", { yes: true, appEnv: "production" });
    expect(calls.filter((c) => c.path.includes("/checks"))).toEqual([]);
  });

  it("says so, and exits 0, when no production check was started or none had a verdict in time", async () => {
    stubFetch(TARGET, {
      ...moving(plan()),
      [PRODUCTION_CHECKS]: () => ({ status: 200, body: production("none", "", null) })
    });
    vi.stubEnv("OXYC_QUIET", "");
    await runPromote(fakeContext(), "acme/store", { yes: true }, QUICK);
    expect(io.err()).toContain("no production check was started for this build");

    vi.unstubAllGlobals();
    stubFetch(TARGET, {
      ...moving(plan()),
      [PRODUCTION_CHECKS]: () => ({ status: 200, body: production("running", "running", null) })
    });
    await runPromote(fakeContext(), "acme/store", { yes: true }, { ...QUICK, timeoutMs: 0 });
    expect(io.err()).toContain(
      "production's check had no verdict after 0s: `oxyc checks show acme/store --build b-7 --environment production --wait` reads where it is"
    );
  });

  it("--json prints the plan once, with how production's check ended", async () => {
    stubFetch(TARGET, {
      ...moving(plan()),
      [PRODUCTION_CHECKS]: () => ({
        status: 200,
        body: production("failed", "failed", "1 new error on 1 of 14 screens")
      })
    });
    const failure = await runPromote(
      fakeContext(),
      "acme/store",
      { yes: true, json: true },
      QUICK
    ).catch((cause: unknown) => cause);
    expect((failure as CliError).code).toBe(ExitCode.CHECK_FAILED);
    const printed = JSON.parse(io.out()) as Plan & { production_check: unknown };
    expect(printed.moved).toBe(true);
    expect(printed.production_check).toEqual({ state: "failed", attempt: 1 });
  });
});
