import { describe, expect, it } from "vitest";
import type { BuildCheck, BuildChecks } from "@/types/appBuildChecks";
import type { ReleaseTimeline } from "@/types/appReleases";
import type { AppBuild } from "@/types/apps";
import { checksRollup } from "./buildChecksModel";
import {
  cardGate,
  gatedRollup,
  hasProductionAttempt,
  productionSmokeLine,
  rollbackTarget
} from "./promoteGate";

const REQUIRED = ["functions", "loads", "design_system", "smoke"];
const ALL = ["validation", ...REQUIRED];

const row = (kind: string, status: string, over: Partial<BuildCheck> = {}): BuildCheck => ({
  kind,
  label: kind,
  status,
  advisory: false,
  summary: null,
  detail: null,
  notes: [],
  attempt: 1,
  run_id: "r",
  created_at: "2026-11-22T10:00:00Z",
  started_at: null,
  finished_at: null,
  duration_ms: null,
  ...over
});

/** A staging attempt whose five rows have these statuses, in `ALL`'s order. */
const staging = (state: string, statuses: string[]): BuildChecks =>
  ({
    state,
    environment: "staging",
    attempt: 1,
    checks: statuses.map((status, i) => row(ALL[i], status)),
    runner: { label: "run", url: null, started_at: "2026-11-22T10:00:10Z" }
  }) as BuildChecks;

describe("cardGate on a check nobody ran", () => {
  it("leaves a required kind unmet when its skip says staging moved on before it ran", () => {
    const moved = { detail: { findings: [], not_checked: true } };
    const checks = {
      state: "failed",
      checks: [
        row("validation", "passed"),
        row("functions", "skipped", moved),
        row("loads", "skipped", moved),
        row("design_system", "failed"),
        row("smoke", "skipped", moved)
      ]
    } as BuildChecks;
    expect(cardGate(checks, REQUIRED)).toEqual({
      state: "unmet",
      unmet: ["functions", "loads", "design_system", "smoke"],
      running: []
    });
    // A skip the check decided itself (an app with no check functions) still satisfies.
    const own = { ...checks, checks: checks.checks.map((c) => ({ ...c, detail: null })) };
    expect(cardGate(own as BuildChecks, REQUIRED)?.unmet).toEqual(["design_system"]);
  });
});

const production = (status: string, over: Partial<BuildCheck> = {}): BuildChecks =>
  ({
    state: status === "running" ? "running" : status,
    environment: "production",
    attempt: 1,
    checks: [row("production_smoke", status, over)]
  }) as BuildChecks;

const screens = (n: number) =>
  Array.from({ length: n }, (_, i) => ({ path: `s${i}`, viewports: [] }));

describe("cardGate", () => {
  it("holds nothing back until the checks are read", () => {
    expect(cardGate(undefined, REQUIRED)).toBeNull();
  });

  it("is open when every required check passed or was skipped", () => {
    const gate = cardGate(
      staging("passed", ["passed", "passed", "skipped", "passed", "passed"]),
      REQUIRED
    );
    expect(gate).toEqual({ state: "open", unmet: [], running: [] });
  });

  it("waits while a required check is queued or running", () => {
    const gate = cardGate(
      staging("running", ["passed", "passed", "running", "passed", "queued"]),
      REQUIRED
    );
    expect(gate).toEqual({ state: "waiting", unmet: [], running: ["loads", "smoke"] });
  });

  it("waits, not fails, while one failed and another still runs", () => {
    const gate = cardGate(
      staging("running", ["passed", "failed", "running", "passed", "passed"]),
      REQUIRED
    );
    expect(gate).toEqual({ state: "waiting", unmet: ["functions"], running: ["loads"] });
  });

  it("is unmet on a failed or errored required check", () => {
    const gate = cardGate(
      staging("failed", ["passed", "passed", "error", "failed", "passed"]),
      REQUIRED
    );
    expect(gate).toEqual({ state: "unmet", unmet: ["loads", "design_system"], running: [] });
  });

  it("does not hold on a check the app does not require", () => {
    const data = staging("failed", ["passed", "passed", "passed", "failed", "passed"]);
    expect(cardGate(data, ["functions", "loads", "smoke"])?.state).toBe("open");
    // Still open while that check runs: only a required one makes Promote wait.
    const running = staging("running", ["passed", "passed", "passed", "running", "passed"]);
    expect(cardGate(running, ["functions", "loads", "smoke"])?.state).toBe("open");
  });

  it("waits on the comparison with production only while the app holds Promote on it", () => {
    const six = (changes: string): BuildChecks => {
      const data = staging("running", ["passed", "passed", "passed", "passed", "passed"]);
      data.checks.push(row("changes", changes, { advisory: true }));
      return data;
    };
    expect(cardGate(six("running"), REQUIRED)?.state).toBe("open");
    expect(cardGate(six("running"), REQUIRED, true)).toEqual({
      state: "waiting",
      unmet: [],
      running: ["changes"]
    });
    // Once compared, what holds is whether a person has seen it: the confirm's to say.
    expect(cardGate(six("passed"), REQUIRED, true)?.state).toBe("open");
    expect(cardGate(six("skipped"), REQUIRED, true)?.state).toBe("open");
  });

  it("counts a build never checked, and a required check with no row, as unmet", () => {
    expect(cardGate(staging("none", []), REQUIRED)).toEqual({
      state: "unmet",
      unmet: REQUIRED,
      running: []
    });
    // An attempt from before the gate: four kinds, no smoke row.
    const before = staging("passed", ["passed", "passed", "passed", "passed"]);
    expect(cardGate(before, REQUIRED)).toEqual({ state: "unmet", unmet: ["smoke"], running: [] });
    expect(cardGate(staging("none", []), [])?.state).toBe("open");
  });
});

describe("gatedRollup", () => {
  it("is the roll-up the card always showed while nothing required is unmet", () => {
    for (const data of [
      staging("passed", ["passed", "passed", "passed", "passed", "passed"]),
      staging("running", ["passed", "passed", "running", "passed", "queued"]),
      // Failed, but not a check this app requires.
      staging("failed", ["failed", "passed", "passed", "passed", "passed"])
    ]) {
      expect(gatedRollup(data, REQUIRED)).toEqual(checksRollup(data));
    }
  });

  it("says a required check failed, in one chip", () => {
    const one = staging("failed", ["passed", "passed", "passed", "failed", "passed"]);
    expect(gatedRollup(one, REQUIRED)).toEqual({
      state: "failed",
      tone: "danger",
      label: "1 required check failed"
    });
    const two = staging("failed", ["passed", "failed", "error", "passed", "passed"]);
    expect(gatedRollup(two, REQUIRED).label).toBe("2 required checks failed");
  });

  it("says could not run when no required check found a fault, and not run for a missing one", () => {
    const errored = staging("failed", ["passed", "error", "passed", "passed", "passed"]);
    expect(gatedRollup(errored, REQUIRED).label).toBe("1 required check could not run");
    const missing = staging("passed", ["passed", "passed", "passed", "passed"]);
    expect(gatedRollup(missing, REQUIRED)).toMatchObject({
      tone: "danger",
      label: "1 required check not run"
    });
  });

  it("reads a build never checked as the problem it is", () => {
    expect(gatedRollup(staging("none", []), REQUIRED)).toEqual({
      state: "none",
      tone: "danger",
      label: "Not checked"
    });
    expect(gatedRollup(staging("none", []), [])).toEqual(checksRollup(staging("none", [])));
  });
});

describe("productionSmokeLine", () => {
  it("has no line without a production attempt", () => {
    expect(productionSmokeLine(undefined)).toBeNull();
    // A server that ignores `environment` answers the staging attempt.
    expect(
      productionSmokeLine(staging("passed", ["passed", "passed", "passed", "passed", "passed"]))
    ).toBeNull();
    expect(productionSmokeLine({ ...production("passed"), checks: [] })).toBeNull();
    // Never read as production's on its rows alone: the answer says whose it is.
    expect(productionSmokeLine({ ...production("failed"), environment: "staging" })).toBeNull();
  });

  it("waits for the new build while the attempt is queued or running", () => {
    for (const status of ["queued", "running"]) {
      expect(productionSmokeLine(production(status))).toMatchObject({
        state: "waiting",
        tone: "muted",
        label: "Waiting for the new build"
      });
    }
  });

  it("says how many screens opened with no new error", () => {
    const passed = production("passed", { detail: { screens: screens(14), new_errors: [] } });
    expect(productionSmokeLine(passed)).toMatchObject({
      state: "passed",
      tone: "ok",
      label: "14 screens, 0 new errors"
    });
    // No screens in the detail: the check's own summary.
    expect(productionSmokeLine(production("passed", { summary: "opened" }))?.label).toBe("opened");
  });

  it("says how many screens have new errors", () => {
    const failed = production("failed", {
      summary: "2 of 14 screens have new errors",
      detail: {
        screens: screens(14),
        new_errors: [
          { path: "stores", viewport: "desk", error: "TypeError: x" },
          { path: "stores", viewport: "phone", error: "TypeError: x" },
          { path: "reorder", viewport: "desk", error: "500" }
        ]
      }
    });
    expect(productionSmokeLine(failed)).toEqual({
      state: "failed",
      tone: "danger",
      label: "2 screens with new errors",
      title: "2 of 14 screens have new errors"
    });
    // Failed with no error listed (a screen stopped getting ready): the summary.
    expect(productionSmokeLine(production("failed", { summary: "stores not ready" }))?.label).toBe(
      "stores not ready"
    );
    expect(productionSmokeLine(production("failed"))?.label).toBe("Smoke failed");
  });

  it("keeps a smoke that could not run apart from one that failed", () => {
    expect(productionSmokeLine(production("error"))).toMatchObject({
      state: "error",
      tone: "danger",
      label: "Could not run"
    });
    expect(productionSmokeLine(production("skipped"))).toMatchObject({
      state: "other",
      tone: "muted",
      label: "skipped"
    });
  });
});

describe("hasProductionAttempt", () => {
  it("needs a production answer with rows", () => {
    expect(hasProductionAttempt(production("passed"))).toBe(true);
    expect(hasProductionAttempt(undefined)).toBe(false);
    expect(hasProductionAttempt({ ...production("passed"), attempt: null, checks: [] })).toBe(
      false
    );
    expect(
      hasProductionAttempt(staging("passed", ["passed", "passed", "passed", "passed", "passed"]))
    ).toBe(false);
  });
});

describe("rollbackTarget", () => {
  const build = (id: string, over: Partial<AppBuild> = {}): AppBuild => ({
    id,
    build_id: `${id}-build`,
    created_at: "2026-11-22T09:00:00Z",
    is_draft: false,
    is_published: false,
    ...over
  });
  const release = (index: number, id: string | null, over: object = {}) => ({
    index,
    build: id ? { id, build_id: `${id}-build` } : null,
    current: false,
    previous: null,
    ...over
  });
  const timeline = (releases: object[]) => ({ releases }) as Pick<ReleaseTimeline, "releases">;
  const builds = [build("new", { is_published: true }), build("old"), build("older")];

  it("is the retained build of the release before the current one", () => {
    // The release it is compared with, which need not be the one listed next.
    const t = timeline([
      release(2, "new", { current: true, previous: 0 }),
      release(1, "old"),
      release(0, "older")
    ]);
    expect(rollbackTarget(t, builds)?.id).toBe("older");
  });

  it("is nothing when the timeline names none, or its build is gone", () => {
    expect(rollbackTarget(undefined, builds)).toBeNull();
    expect(rollbackTarget(timeline([release(0, "new", { current: true })]), builds)).toBeNull();
    const pruned = timeline([
      release(1, "new", { current: true, previous: 0 }),
      release(0, "gone")
    ]);
    expect(rollbackTarget(pruned, builds)).toBeNull();
    const noBuild = timeline([release(1, "new", { current: true, previous: 0 }), release(0, null)]);
    expect(rollbackTarget(noBuild, builds)).toBeNull();
  });
});
