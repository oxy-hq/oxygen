import { AxiosError, type AxiosResponse } from "axios";
import { describe, expect, it } from "vitest";
import type { MovePlan, MovePlanChecks } from "@/types/appPromote";
import {
  checkBlockerText,
  overrideOffer,
  overrideRefusal,
  planChecks,
  reasonState
} from "./planChecks";

const ROWS = [
  { kind: "validation", label: "Builds and validates", status: "passed", summary: "ok" },
  { kind: "functions", label: "Its checks pass", status: "passed", summary: "3 of 3" },
  { kind: "loads", label: "It loads, desk and phone", status: "running", summary: null },
  { kind: "design_system", label: "Follows the design system", status: "failed", summary: "x" },
  { kind: "smoke", label: "Every screen opens", status: "passed", summary: "14 screens" },
  { kind: "changes", label: "What changed", status: "passed", summary: "4 of 28" }
];
const REQUIRED = ["functions", "loads", "design_system", "smoke"];

const checks = (over: Partial<MovePlanChecks> = {}): MovePlanChecks => ({
  required: REQUIRED,
  attempt: 2,
  state: "failed",
  rows: ROWS.map((r) => ({ ...r, required: REQUIRED.includes(r.kind) })),
  unmet: ["design_system"],
  running: [],
  changes: { changed: 4, compared: 28, seen: false, seen_required: false },
  summary: null,
  overridable: true,
  overridden: null,
  ...over
});
const plan = (c: MovePlanChecks | null | undefined) => ({ checks: c }) as Pick<MovePlan, "checks">;

const refused = (status: number, code: string) =>
  new AxiosError("refused", "ERR", undefined, undefined, {
    status,
    data: { code, message: "no" }
  } as AxiosResponse);

describe("planChecks", () => {
  it("is nothing with the gate off, on a rollback, or from an older server", () => {
    expect(planChecks(undefined)).toBeNull();
    expect(planChecks(plan(null))).toBeNull();
    expect(planChecks(plan(undefined))).toBeNull();
  });

  it("keeps the required rows in the list's order, and names what did not pass", () => {
    const view = planChecks(plan(checks()));
    expect(view?.required.map((r) => r.kind)).toEqual(REQUIRED);
    expect(view?.unmet).toEqual(["Follows the design system"]);
    expect(view?.running).toEqual([]);
    expect(view?.changes).toEqual({ changed: 4, compared: 28, seen: false, seenRequired: false });
    expect(view?.overridable).toBe(true);
  });

  it("names a check still open once, as running, even if the server lists it unmet too", () => {
    const view = planChecks(
      plan(checks({ unmet: ["loads", "design_system"], running: ["loads"] }))
    );
    expect(view?.unmet).toEqual(["Follows the design system"]);
    expect(view?.running).toEqual(["It loads, desk and phone"]);
  });

  it("reads a plan that carries almost none of it", () => {
    const bare = planChecks(plan({} as MovePlanChecks));
    expect(bare).toEqual({
      required: [],
      unmet: [],
      running: [],
      changes: null,
      summary: null,
      overridable: false
    });
    // A row marked required counts even when the list of kinds is missing.
    const flagged = planChecks(
      plan({ rows: [{ ...ROWS[4], required: true }], unmet: ["mystery"] } as MovePlanChecks)
    );
    expect(flagged?.required.map((r) => r.kind)).toEqual(["smoke"]);
    expect(flagged?.unmet).toEqual(["mystery"]);
  });

  it("carries the summary only when it is a note with text", () => {
    const note = { author: "model", model: "m-1", text: "Restyles the Stores table." };
    const view = planChecks(plan(checks({ summary: note as MovePlanChecks["summary"] })));
    expect(view?.summary).toEqual({ ...note, refs: [], created_at: "" });
    expect(
      planChecks(plan(checks({ summary: {} as MovePlanChecks["summary"] })))?.summary
    ).toBeNull();
  });
});

describe("checkBlockerText", () => {
  const view = planChecks(plan(checks({ unmet: ["design_system", "smoke"], running: ["loads"] })));

  it("says each check blocker in a few words, then what it is about", () => {
    expect(checkBlockerText("checks_not_passed", view)).toBe(
      "Required checks did not pass: Follows the design system, Every screen opens"
    );
    expect(checkBlockerText("checks_running", view)).toBe(
      "Required check still running: It loads, desk and phone"
    );
    expect(checkBlockerText("changes_not_seen", view)).toBe(
      "Nobody has seen what changed: 4 screens"
    );
  });

  it("leaves every other blocker, and a plan without checks, to the server's message", () => {
    expect(checkBlockerText("required_secrets_unset", view)).toBeNull();
    expect(checkBlockerText("checks_not_passed", null)).toBeNull();
  });
});

describe("overrideOffer", () => {
  const view = planChecks(plan(checks({ running: ["loads"] })));
  const failed = [{ code: "checks_not_passed" }];

  it("offers a promote past the check blockers, with a chip for each thing passed", () => {
    const held = [...failed, { code: "checks_running" }, { code: "changes_not_seen" }];
    expect(
      overrideOffer(
        held,
        held.map((b) => b.code),
        view
      )
    ).toEqual({
      codes: ["checks_not_passed", "checks_running", "changes_not_seen"],
      past: [
        { label: "Follows the design system", tone: "danger" },
        { label: "It loads, desk and phone · running", tone: "muted" },
        { label: "Changes not seen", tone: "warn" }
      ]
    });
  });

  it("offers nothing when the caller may not, or nothing holds", () => {
    const locked = planChecks(plan(checks({ overridable: false })));
    expect(overrideOffer(failed, ["checks_not_passed"], locked)).toBeUndefined();
    expect(overrideOffer([], [], view)).toBeUndefined();
    expect(overrideOffer(failed, ["checks_not_passed"], null)).toBeUndefined();
  });

  it("offers nothing while another blocker stands, which no reason clears", () => {
    const held = [...failed, { code: "required_secrets_unset" }];
    expect(overrideOffer(held, ["checks_not_passed"], view)).toBeUndefined();
  });
});

describe("reasonState", () => {
  it("counts what is left after trimming, and is ready at ten", () => {
    expect(reasonState("")).toEqual({ count: 0, ok: false, text: "0 / 10" });
    expect(reasonState("  hotfix   ")).toEqual({ count: 6, ok: false, text: "6 / 10" });
    expect(reasonState("123456789")).toMatchObject({ count: 9, ok: false });
    expect(reasonState(" 1234567890 ")).toEqual({ count: 10, ok: true, text: "10" });
    expect(reasonState("         a")).toMatchObject({ count: 1, ok: false });
  });
});

describe("overrideRefusal", () => {
  it("words the two refusals of an override, and no other failure", () => {
    expect(overrideRefusal(refused(403, "override_not_allowed"))).toBe(
      "Only an org owner or admin can promote past a check."
    );
    expect(overrideRefusal(refused(400, "override_reason_required"))).toBe(
      "The reason needs at least 10 characters."
    );
    expect(overrideRefusal(refused(409, "target_moved"))).toBeNull();
    expect(overrideRefusal(refused(403, "forbidden"))).toBeNull();
    expect(overrideRefusal(new Error("offline"))).toBeNull();
  });
});
