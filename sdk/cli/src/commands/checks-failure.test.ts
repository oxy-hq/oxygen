/**
 * The order `oxyc checks run` picks a failure reason in, and the floor beneath
 * every reason it knows.
 *
 * Its own file rather than more cases in `checks.test.ts`: that file pins
 * credential resolution, app resolution and the poll state machine, and needs
 * the whole route-table harness (OIDC, paged app listing) to do it. These cases
 * need one function's run detail and nothing else, so they carry a four-route
 * stub of their own and stay readable. #4410's dead-letter cases moved here for
 * the same reason, and because one file should own the question: `checks.test.ts`
 * had grown past the 1,200-line file limit carrying them.
 *
 * What is pinned here is the *precedence* — a reason that is specific must not
 * be shadowed by one that is generic — and the last resort, which is the line a
 * release gate prints for the one run nobody can otherwise diagnose.
 */

import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetOidcExchanges } from "../auth/oidc.js";
import { type Context, createContext } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import { type CheckResult, type ChecksReport, runChecks } from "./checks.js";

const TARGET = "https://oxy.test";
const APP_ID = "a1a1a1a1-2222-3333-4444-555555555555";
const APP = "oxy-canary/platform-canary";
const INVOCATION = "11111111-1111-4111-8111-111111111111";

/** The real `Context` with a bearer, and no credentials file to find. */
function context(): Context {
  vi.stubEnv("OXY_TOKEN", "tok");
  vi.stubEnv("OXY_API_KEY", "");
  vi.stubEnv("OXY_SERVICE_ACCOUNT", "");
  vi.stubEnv("OXY_CREDENTIALS_PATH", join(tmpdir(), "oxyc-no-such-credentials.json"));
  return createContext({ env: "production", target: TARGET }, tmpdir());
}

/**
 * The four routes `checks run` touches, with `detail` as the run's read-back.
 * One app, one check, one poll — the app listing answers on the first page so
 * nothing here depends on paging.
 */
function stubFetch(detail: Record<string, unknown>): void {
  const routes: Record<string, unknown> = {
    "GET /api/customer-apps?limit=100&offset=0": {
      items: [{ id: APP_ID, slug: "platform-canary", org_slug: "oxy-canary" }],
      next_offset: null
    },
    [`GET /api/customer-apps/${APP_ID}/functions`]: [{ name: "canary", check: true }],
    [`POST /api/customer-apps/${APP_ID}/functions/canary/runs`]: { run_id: "run-1" },
    [`GET /api/customer-apps/${APP_ID}/function-runs/run-1`]: detail
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string | URL, init?: RequestInit) => {
      const key = `${(init?.method ?? "GET").toUpperCase()} ${String(input).replace(TARGET, "")}`;
      const body = routes[key];
      if (body === undefined) {
        return new Response(JSON.stringify({ error: `no stub for ${key}` }), { status: 404 });
      }
      return new Response(JSON.stringify(body), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    })
  );
}

/**
 * Run the one check and return its result. `runChecks` prints the `--json`
 * report before it throws, so the exit code is asserted here and the report
 * still comes back — which keeps these cases off terminal formatting.
 */
async function failing(detail: Record<string, unknown>): Promise<CheckResult> {
  stubFetch(detail);
  let printed = "";
  const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
    printed += String(chunk);
    return true;
  });
  const thrown = await runChecks(context(), APP, {
    json: true,
    timeoutSeconds: 5,
    pollMs: 0
  }).catch((e: unknown) => e as CliError);
  write.mockRestore();

  expect(thrown).toBeInstanceOf(CliError);
  expect((thrown as CliError).code).toBe(ExitCode.CHECK_FAILED);
  return (JSON.parse(printed.trim()) as ChecksReport).checks[0] as CheckResult;
}

/**
 * The shape a dead-lettered check comes back as: status `failed` (the server
 * maps a `dead` queue entry to it), no `error` — nothing ever ran the function,
 * so nothing reported one — and the queue entry that says so.
 *
 * This is the staging canary's actual failure, and `status failed` was the whole
 * of what `oxyc` said about it. `custom-app-checks.yaml` turns this exit code
 * into `release-hold` on the prod bump PR, so this one line is what a person
 * sees over a blocked promotion.
 */
const DEAD_QUEUE = {
  status: "dead",
  claim_count: 3,
  max_claims: 3,
  queued_at: "2026-11-23T10:40:00Z",
  last_heartbeat: "2026-11-23T10:43:28Z",
  updated_at: "2026-11-23T10:44:30Z"
};

describe("a check that does not pass", () => {
  beforeEach(() => {
    resetOidcExchanges();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
  });

  // The abandoned attempt is the reason the run has an invocation at all: it
  // never completed, so only the started event names it. Its call trace is
  // what says *which* host call was hanging when the heartbeat stopped, so the
  // one red line has to carry the id.
  it("names the abandoned attempt's invocation in the dead-letter line", async () => {
    const check = await failing({
      run_id: "run-1",
      status: "failed",
      answer: null,
      error: null,
      invocation_id: INVOCATION,
      queue: DEAD_QUEUE
    });
    const error = check.error ?? "";
    expect(error).toContain("dead-lettered after 3 of 3 claims");
    expect(error).toContain("stopped heartbeating");
    // The two timestamps that bracket the disappearance, plus the enqueue —
    // without them nobody can line this up against a worker rollout.
    expect(error).toContain("queued 2026-11-23T10:40:00Z");
    expect(error).toContain("last heartbeat 2026-11-23T10:43:28Z");
    expect(error).toContain("dead-lettered 2026-11-23T10:44:30Z");
    expect(error).toContain(`last attempt invocation ${INVOCATION}`);
    // What it must no longer be.
    expect(error).not.toBe("status failed");
    // The ✗ line carries the whole reason: a 200-char cap used to cut it off
    // right where the timestamps begin, and `printResultLine` now cuts at 400.
    expect(error.length).toBeLessThanOrEqual(400);
    // An older server reports no invocation for such a run; the clause is
    // omitted rather than naming an undefined one.
    const older = await failing({
      run_id: "run-1",
      status: "failed",
      answer: null,
      error: null,
      queue: DEAD_QUEUE
    });
    expect(older.error).toContain("dead-lettered after 3 of 3 claims");
    expect(older.error).not.toContain("last attempt invocation");
  });

  // The run's own message is the most specific thing there is, so a queue the
  // reaper gave up on must not talk over it.
  it("reports the run's own error ahead of anything the queue says", async () => {
    const check = await failing({
      run_id: "run-1",
      status: "failed",
      answer: null,
      error: "airhouse schema introspection failed: connection closed",
      invocation_id: INVOCATION,
      queue: DEAD_QUEUE
    });
    expect(check.error).toBe("airhouse schema introspection failed: connection closed");

    // And the complementary shape: a queue entry that is not `dead` has
    // nothing to explain, so it does not touch the message either.
    const alive = await failing({
      run_id: "run-1",
      status: "failed",
      answer: null,
      error: "boom",
      queue: { ...DEAD_QUEUE, status: "completed" }
    });
    expect(alive.error).toBe("boom");
  });

  // Nothing recorded a reason and the queue has nothing to say either: the
  // floor must still be actionable, because the run that reaches it is the one
  // nobody can otherwise diagnose.
  it("names the kind, run and invocation when no reason was recorded", async () => {
    const check = await failing({
      run_id: "run-1",
      status: "failed",
      answer: null,
      error: null,
      error_kind: "timeout",
      invocation_id: INVOCATION,
      queue: { ...DEAD_QUEUE, status: "completed" }
    });
    const error = check.error ?? "";
    expect(error).toContain("no reason recorded");
    expect(error).toContain("kind timeout");
    expect(error).toContain("run run-1");
    expect(error).toContain(`invocation ${INVOCATION}`);
  });

  // Nothing but a status: say so plainly, and omit what is missing rather
  // than printing "kind undefined".
  it("omits the parts a bare failure does not have", async () => {
    const check = await failing({ run_id: "run-1", status: "failed", answer: null, error: null });
    expect(check.error).toBe("status failed with no reason recorded (run run-1)");
  });

  // The two verdicts that are read off the status and the answer stay ahead of
  // everything the queue or the floor can offer.
  it("still prefers the timeout and ok:false verdicts", async () => {
    const timedOut = await failing({
      run_id: "run-1",
      status: "timed_out",
      answer: null,
      queue: DEAD_QUEUE
    });
    expect(timedOut.error).toBe("timed out after 5s");

    const lied = await failing({
      run_id: "run-1",
      status: "done",
      answer: JSON.stringify({ ok: false }),
      queue: DEAD_QUEUE
    });
    expect(lied.error).toBe('answer reported "ok": false');
  });
});
