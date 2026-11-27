import { AxiosError, type AxiosResponse } from "axios";
import { describe, expect, it } from "vitest";
import type { BuildCheck, BuildChecks } from "@/types/appBuildChecks";
import type { AppBuild } from "@/types/apps";
import {
  belowFloor,
  checkedWhen,
  checkStatus,
  checksProgress,
  checksRollup,
  checksTarget,
  clockText,
  durationText,
  openByDefault,
  readyText,
  rerunRefusal,
  runnerHref,
  stalledLine,
  viewportName
} from "./buildChecksModel";

const KINDS = ["validation", "functions", "loads", "design_system"] as const;

const row = (status: string, over: Partial<BuildCheck> = {}): BuildCheck => ({
  kind: "validation",
  label: "Builds and validates",
  status,
  advisory: false,
  summary: null,
  detail: { findings: [] },
  notes: [],
  attempt: 1,
  run_id: "r",
  created_at: "2026-11-20T10:00:00Z",
  started_at: null,
  finished_at: null,
  duration_ms: null,
  ...over
});

/** Four phase-1 rows with these statuses, in the server's order. */
const four = (...statuses: string[]): BuildCheck[] =>
  statuses.map((s, i) => row(s, { kind: KINDS[i] }));

const rollup = (state: BuildChecks["state"], checks: BuildCheck[]) =>
  checksRollup({ state, checks });

describe("checksRollup — the Live build card's Checks row", () => {
  it("says a build checked never is not checked, in muted words", () => {
    expect(rollup("none", [])).toEqual({ state: "none", tone: "muted", label: "Not checked" });
  });

  it("counts what is done while the checks run, neutral", () => {
    expect(rollup("running", four("passed", "passed", "running", "queued"))).toEqual({
      state: "running",
      tone: "muted",
      label: "2 of 4 done"
    });
    // A failure already in counts as done: the row says progress, not the verdict.
    expect(rollup("running", four("failed", "running", "queued", "queued")).label).toBe(
      "1 of 4 done"
    );
    expect(rollup("running", []).label).toBe("Queued");
  });

  it("names the failures in red, one and many", () => {
    expect(rollup("failed", four("passed", "passed", "passed", "failed"))).toEqual({
      state: "failed",
      tone: "danger",
      label: "1 check failed"
    });
    expect(rollup("failed", four("failed", "passed", "failed", "passed")).label).toBe(
      "2 checks failed"
    );
  });

  it("counts an error as a failure, and says 'could not run' only when every failure is one", () => {
    expect(rollup("failed", four("passed", "error", "passed", "passed"))).toEqual({
      state: "failed",
      tone: "danger",
      label: "1 check could not run"
    });
    expect(rollup("failed", four("error", "error", "passed", "passed")).label).toBe(
      "2 checks could not run"
    );
    expect(rollup("failed", four("failed", "error", "passed", "passed")).label).toBe(
      "2 checks failed"
    );
  });

  it("says all passed in green, and names the skips when there are any", () => {
    expect(rollup("passed", four("passed", "passed", "passed", "passed"))).toEqual({
      state: "passed",
      tone: "ok",
      label: "All 4 passed"
    });
    expect(rollup("passed", four("passed", "skipped", "passed", "passed"))).toEqual({
      state: "passed",
      tone: "ok",
      label: "3 passed, 1 skipped"
    });
    // Nothing verified anything: not green.
    expect(rollup("passed", four("skipped", "skipped", "skipped", "skipped"))).toEqual({
      state: "passed",
      tone: "muted",
      label: "4 skipped"
    });
  });

  it("leaves advisory rows out of every count", () => {
    const rows = [
      ...four("passed", "passed", "passed", "passed"),
      row("failed", { kind: "design_audit", advisory: true })
    ];
    expect(rollup("passed", rows).label).toBe("All 4 passed");
    const running = [
      ...four("passed", "passed", "passed", "passed"),
      row("running", { advisory: true })
    ];
    expect(rollup("running", running).label).toBe("4 of 4 done");
  });

  it("never says the words phase 1 may not say", () => {
    const labels = [
      rollup("none", []),
      rollup("running", four("queued", "queued", "queued", "queued")),
      rollup("running", four("passed", "running", "queued", "queued")),
      rollup("failed", four("failed", "error", "passed", "skipped")),
      rollup("passed", four("passed", "passed", "passed", "skipped"))
    ].map((r) => r.label.toLowerCase());
    for (const label of labels) expect(label).not.toMatch(/required|blocked|waiting/);
  });

  it("says 'Starting' while the runner is not up: no runner, every row queued", () => {
    const queued = four("queued", "queued", "queued", "queued");
    const starting = { state: "running", tone: "muted", label: "Starting" };
    expect(checksRollup({ state: "running", checks: queued, runner: null })).toEqual(starting);
    // An older server names no runner at all: the same reading.
    expect(checksRollup({ state: "running", checks: queued })).toEqual(starting);
    // A runner that claimed it is at work, even before a row has a verdict.
    expect(checksRollup({ state: "running", checks: queued, runner: RUNNER }).label).toBe(
      "0 of 4 done"
    );
    // A row that moved means somebody started, runner named or not.
    expect(rollup("running", four("running", "queued", "queued", "queued")).label).toBe(
      "0 of 4 done"
    );
    // No rows at all stays as it was.
    expect(checksRollup({ state: "running", checks: [], runner: null }).label).toBe("Queued");
  });
});

/** The newest attempt was queued at `T0`; `at(19)` is 19 seconds later. */
const T0 = "2026-11-20T10:00:00Z";
const at = (seconds: number) => new Date(Date.parse(T0) + seconds * 1000).toISOString();
const RUNNER = {
  label: "GitHub Actions run #1482",
  url: "https://github.com/oxy-hq/oxygen-internal/actions/runs/123",
  started_at: at(19)
};

type Answer = Pick<BuildChecks, "state" | "checks" | "attempts" | "runner" | "stalled">;
const answer = (state: BuildChecks["state"], checks: BuildCheck[], over: Partial<Answer> = {}) => ({
  state,
  checks,
  attempts: [
    { attempt: 2, state, created_at: T0 },
    { attempt: 1, state: "failed", created_at: "2026-11-20T09:00:00Z" }
  ],
  runner: null,
  stalled: null,
  ...over
});

/** Four rows with these statuses, each one with a verdict finishing at its own second. */
const finished = (statuses: string[], seconds: number[]) =>
  four(...statuses).map((c, i) => ({ ...c, finished_at: at(seconds[i]) }));

/** The console mock's five states (`checks-on-github-actions.html`, `STATES`). */
const MOCK = {
  starting: answer("running", four("queued", "queued", "queued", "queued")),
  running: answer("running", four("passed", "running", "running", "passed"), { runner: RUNNER }),
  passed: answer("passed", finished(["passed", "passed", "passed", "passed"], [20, 61, 98, 33]), {
    runner: RUNNER
  }),
  failed: answer("failed", finished(["passed", "passed", "failed", "passed"], [20, 61, 112, 33]), {
    runner: RUNNER
  }),
  noanswer: answer("failed", four("error", "error", "error", "error"), { stalled: "no_runner" })
} satisfies Record<string, Answer>;

/** `[state, label, value]` per step, the shape the mock writes its steps in. */
const steps = (data: Answer) => checksProgress(data)?.map((s) => [s.state, s.label, s.value]);

describe("checksProgress — the run's four steps", () => {
  it("starting: queued, and the runner has not answered", () => {
    expect(steps(MOCK.starting)).toEqual([
      ["done", "Queued", "0:00"],
      ["active", "Runner", "starting"],
      ["todo", "Checking", ""],
      ["todo", "Done", ""]
    ]);
  });

  it("running: the runner took 19 seconds to come up, and two of four have a verdict", () => {
    expect(steps(MOCK.running)).toEqual([
      ["done", "Queued", "0:00"],
      ["done", "Runner", "0:19"],
      ["active", "Checking", "2 of 4"],
      ["todo", "Done", ""]
    ]);
  });

  it("passed: done, with the time from queued to the last verdict", () => {
    expect(steps(MOCK.passed)).toEqual([
      ["done", "Queued", "0:00"],
      ["done", "Runner", "0:19"],
      ["done", "Checking", "4 of 4"],
      ["done", "Done", "1:38"]
    ]);
  });

  it("failed: every check has a verdict, and Done carries the failure with its time", () => {
    expect(steps(MOCK.failed)).toEqual([
      ["done", "Queued", "0:00"],
      ["done", "Runner", "0:19"],
      ["done", "Checking", "4 of 4"],
      ["failed", "Done", "1:52"]
    ]);
  });

  it("no answer: the strip ends at the runner, though the rows read 'error'", () => {
    expect(steps(MOCK.noanswer)).toEqual([
      ["done", "Queued", "0:00"],
      ["failed", "Runner", "no answer"],
      ["todo", "Checking", ""],
      ["todo", "Done", ""]
    ]);
  });

  it("runner stopped: it came up, and the strip ends at Checking", () => {
    const stopped = answer("failed", four("passed", "error", "error", "passed"), {
      runner: RUNNER,
      stalled: "runner_stopped"
    });
    expect(steps(stopped)).toEqual([
      ["done", "Queued", "0:00"],
      ["done", "Runner", "0:19"],
      ["failed", "Checking", "stopped"],
      ["todo", "Done", ""]
    ]);
  });

  it("reads an older server's answer, which has no `runner` and no `stalled`", () => {
    const older = (state: BuildChecks["state"], checks: BuildCheck[]): Answer => ({
      state,
      checks,
      attempts: [{ attempt: 1, state, created_at: T0 }]
    });
    // Nothing moved yet: the same as starting.
    expect(steps(older("running", four("queued", "queued", "queued", "queued")))).toEqual([
      ["done", "Queued", "0:00"],
      ["active", "Runner", "starting"],
      ["todo", "Checking", ""],
      ["todo", "Done", ""]
    ]);
    // Rows moved, so something runs them: no spinner on Runner, and no time to give.
    expect(steps(older("running", four("passed", "running", "queued", "queued")))).toEqual([
      ["done", "Queued", "0:00"],
      ["done", "Runner", ""],
      ["active", "Checking", "1 of 4"],
      ["todo", "Done", ""]
    ]);
    expect(
      steps(older("passed", finished(["passed", "passed", "passed", "skipped"], [5, 9, 98, 2])))
    ).toEqual([
      ["done", "Queued", "0:00"],
      ["done", "Runner", ""],
      ["done", "Checking", "4 of 4"],
      ["done", "Done", "1:38"]
    ]);
  });

  it("has no steps for a build never checked", () => {
    expect(checksProgress(answer("none", [], { attempts: [] }))).toBeNull();
  });

  it("counts and times the rows that are not advisory", () => {
    const rows = [
      ...finished(["passed", "passed", "passed", "passed"], [20, 61, 98, 33]),
      row("running", { kind: "design_audit", advisory: true, finished_at: at(400) })
    ];
    expect(steps(answer("passed", rows, { runner: RUNNER }))?.slice(2)).toEqual([
      ["done", "Checking", "4 of 4"],
      ["done", "Done", "1:38"]
    ]);
  });

  it("leaves a time out rather than invent one", () => {
    // No attempt to count from, and no verdict time on the rows.
    const bare = answer("passed", four("passed", "passed", "passed", "passed"), {
      attempts: [],
      runner: RUNNER
    });
    expect(steps(bare)).toEqual([
      ["done", "Queued", "0:00"],
      ["done", "Runner", ""],
      ["done", "Checking", "4 of 4"],
      ["done", "Done", ""]
    ]);
  });

  it("never says the words phase 1 may not say", () => {
    const words = [...Object.values(MOCK).flatMap((data) => steps(data) ?? [])].flat();
    words.push(stalledLine("no_runner") ?? "", stalledLine("runner_stopped") ?? "");
    for (const word of words) expect(word.toLowerCase()).not.toMatch(/required|blocked|waiting/);
  });
});

describe("clockText", () => {
  it("reads minutes and seconds the way a stopwatch does", () => {
    expect(clockText(T0, T0)).toBe("0:00");
    expect(clockText(T0, at(19))).toBe("0:19");
    expect(clockText(T0, at(19.9))).toBe("0:19");
    expect(clockText(T0, at(98))).toBe("1:38");
    expect(clockText(T0, at(3725))).toBe("62:05");
  });

  it("is empty without both instants, and never negative", () => {
    expect(clockText(null, at(19))).toBe("");
    expect(clockText(T0, undefined)).toBe("");
    expect(clockText(T0, "not a date")).toBe("");
    expect(clockText(at(19), T0)).toBe("0:00");
  });
});

describe("the heading's words", () => {
  it("says a stalled attempt in one line, and nothing otherwise", () => {
    expect(stalledLine("no_runner")).toBe("The runner did not answer. Nothing was checked.");
    expect(stalledLine("runner_stopped")).toBe("The runner stopped before it finished.");
    expect(stalledLine(null)).toBeNull();
    expect(stalledLine(undefined)).toBeNull();
    // A reason a later server adds gets no invented words.
    expect(stalledLine("a_later_reason" as BuildChecks["stalled"])).toBeNull();
  });

  it("says what last happened to the attempt, and when", () => {
    expect(checkedWhen(MOCK.starting)).toEqual({ verb: "queued", at: T0 });
    expect(checkedWhen(MOCK.running)).toEqual({ verb: "started", at: at(19) });
    expect(checkedWhen(MOCK.passed)).toEqual({ verb: "checked", at: T0 });
    expect(checkedWhen(MOCK.failed)).toEqual({ verb: "checked", at: T0 });
    // Stalled is `failed` on the wire, and nothing was checked.
    expect(checkedWhen(MOCK.noanswer)).toEqual({ verb: "queued", at: T0 });
    expect(checkedWhen({ ...MOCK.noanswer, runner: RUNNER, stalled: "runner_stopped" })).toEqual({
      verb: "started",
      at: at(19)
    });
    expect(checkedWhen(answer("none", [], { attempts: [] }))).toBeNull();
  });

  it("does not say a build was checked when no runner was started for its attempt", () => {
    // The server answers `none` and still lists the attempt and its four skipped rows.
    const notStarted = answer("none", four("skipped", "skipped", "skipped", "skipped"));
    expect(notStarted.attempts[0]?.created_at).toBe(T0);
    expect(checkedWhen(notStarted)).toBeNull();
    expect(checksRollup(notStarted).label).toBe("Not checked");
    expect(checksProgress(notStarted)).toBeNull();
  });

  it("links the runner only to a web address", () => {
    expect(runnerHref(RUNNER)).toBe(RUNNER.url);
    expect(runnerHref({ url: null })).toBeNull();
    expect(runnerHref({ url: "javascript:alert(1)" })).toBeNull();
  });
});

describe("checkStatus", () => {
  it("colours only a failure red, and calls an error 'could not run'", () => {
    expect(checkStatus("passed")).toEqual({ label: "passed", tone: "ok" });
    expect(checkStatus("failed")).toEqual({ label: "failed", tone: "danger" });
    expect(checkStatus("error")).toEqual({ label: "could not run", tone: "danger" });
    for (const s of ["queued", "running", "skipped", "a_later_status"]) {
      expect(checkStatus(s)).toEqual({ label: s, tone: "muted" });
    }
  });

  it("opens failures first", () => {
    expect(openByDefault({ status: "failed" })).toBe(true);
    expect(openByDefault({ status: "error" })).toBe(true);
    expect(openByDefault({ status: "passed" })).toBe(false);
    expect(openByDefault({ status: "running" })).toBe(false);
  });

  it("keeps a row the stall answered for closed: it has no evidence to open to", () => {
    for (const stalled of ["no_runner", "runner_stopped"] as const) {
      expect(openByDefault({ status: "error" }, stalled)).toBe(false);
      // A check that did fail before the runner stopped still has its evidence.
      expect(openByDefault({ status: "failed" }, stalled)).toBe(true);
      expect(openByDefault({ status: "passed" }, stalled)).toBe(false);
    }
    expect(openByDefault({ status: "error" }, null)).toBe(true);
    expect(openByDefault({ status: "error" }, undefined)).toBe(true);
  });
});

describe("evidence words", () => {
  it("says a viewport's readiness and its name", () => {
    expect(readyText({ ready: true, ready_ms: 1200 })).toBe("ready in 1.2s");
    expect(readyText({ ready: true, ready_ms: 850 })).toBe("ready in 850ms");
    expect(readyText({ ready: true, ready_ms: null })).toBe("ready");
    expect(readyText({ ready: false, ready_ms: null })).toBe("never ready");
    expect(viewportName({ name: "desk", width: 1440 })).toBe("Desk 1440");
    expect(viewportName({ name: "phone", width: 390 })).toBe("Phone 390");
    expect(durationText(30)).toBe("30ms");
  });

  it("compares the SDK against its floor by number, not by text", () => {
    expect(belowFloor("2.30.0", "2.30.0")).toBe(false);
    expect(belowFloor("2.9.0", "2.30.0")).toBe(true);
    expect(belowFloor("2.31.0", "2.30.0")).toBe(false);
    expect(belowFloor("3.0.0-beta.1", "2.30.0")).toBe(false);
    expect(belowFloor(null, "2.30.0")).toBe(true);
    expect(belowFloor("1.0.0", null)).toBe(false);
  });
});

const httpError = (status: number, data: unknown) =>
  new AxiosError("Request failed", "ERR_BAD_REQUEST", undefined, undefined, {
    status,
    data
  } as AxiosResponse);

describe("rerunRefusal", () => {
  it("says the two 409s in the console's words", () => {
    expect(rerunRefusal(httpError(409, { code: "build_not_on_staging", message: "x" }))).toBe(
      "Staging doesn't serve this build, so its checks can't run again."
    );
    expect(rerunRefusal(httpError(409, { code: "checks_running", message: "x" }))).toBe(
      "Its checks are still running."
    );
  });

  it("carries the server's message for anything else", () => {
    expect(rerunRefusal(httpError(403, { message: "publish tokens may not rerun" }))).toBe(
      "publish tokens may not rerun"
    );
    expect(rerunRefusal(httpError(500, {}))).toBe("Couldn't run the checks again.");
  });
});

const build = (over: Partial<AppBuild>): AppBuild => ({
  id: "row",
  build_id: "0123456789abcdef",
  created_at: "2026-11-20T10:00:00Z",
  is_draft: false,
  is_published: false,
  ...over
});

describe("checksTarget", () => {
  const STAGING = build({ id: "row-s", build_id: "9087d7b0000000", is_draft: true });
  const LIVE = build({ id: "row-p", build_id: "60e2f190000000", is_published: true });

  it("is the build staging serves when nothing is asked for", () => {
    expect(checksTarget([LIVE, STAGING], null)).toEqual({
      ref: "row-s",
      build: STAGING,
      staging: true
    });
    expect(checksTarget([LIVE], null)).toBeNull();
  });

  it("resolves an asked-for build_id to its row id, so every surface shares one read", () => {
    expect(checksTarget([LIVE, STAGING], "60e2f190000000")).toEqual({
      ref: "row-p",
      build: LIVE,
      staging: false
    });
    expect(checksTarget([LIVE, STAGING], "row-s")?.staging).toBe(true);
  });

  it("passes a build it does not retain through, for the server to resolve", () => {
    expect(checksTarget([LIVE], "pruned-sha")).toEqual({
      ref: "pruned-sha",
      build: null,
      staging: false
    });
  });
});
