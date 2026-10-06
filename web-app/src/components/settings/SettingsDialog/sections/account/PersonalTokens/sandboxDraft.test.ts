import { describe, expect, it } from "vitest";
import { DEFAULT_SANDBOX_LIMITS as LIMITS } from "@/libs/sandboxAgentToken";
import type { SandboxApp } from "@/types/apiToken";
import {
  effectiveLifetime,
  emptySandboxDraft,
  lifetimeHours,
  lifetimeProblem,
  pickedApps,
  sandboxMintRequest,
  toggleApp
} from "./sandboxDraft";

const app = (id: string): SandboxApp => ({
  id,
  org_id: "o1",
  org_slug: "acme",
  org_name: "Acme",
  slug: id,
  name: id.toUpperCase()
});

const APPS = ["a1", "a2", "a3", "a4", "a5", "a6"].map(app);

describe("the lifetime choice", () => {
  it("is the server's default until the person picks", () => {
    expect(effectiveLifetime(emptySandboxDraft(), LIMITS)).toEqual({ kind: "preset", hours: 8 });
    expect(
      effectiveLifetime(emptySandboxDraft(), { default_hours: 4, max_hours: 48, max_apps: 5 })
    ).toEqual({ kind: "preset", hours: 4 });
    expect(effectiveLifetime({ lifetime: { kind: "preset", hours: 72 } }, LIMITS)).toEqual({
      kind: "preset",
      hours: 72
    });
  });

  it("reads typed hours only when they are a whole number", () => {
    expect(lifetimeHours({ kind: "preset", hours: 24 })).toBe(24);
    expect(lifetimeHours({ kind: "custom", text: " 12 " })).toBe(12);
    expect(lifetimeHours({ kind: "custom", text: "" })).toBeNull();
    expect(lifetimeHours({ kind: "custom", text: "1.5" })).toBeNull();
    expect(lifetimeHours({ kind: "custom", text: "-3" })).toBeNull();
    expect(lifetimeHours({ kind: "custom", text: "soon" })).toBeNull();
  });

  it("treats an empty box as unfinished and anything else unusable as a problem", () => {
    expect(lifetimeProblem({ kind: "custom", text: "" }, LIMITS)).toBeNull();
    expect(lifetimeProblem({ kind: "custom", text: "soon" }, LIMITS)).toBe(
      "Enter a whole number of hours."
    );
    expect(lifetimeProblem({ kind: "custom", text: "0" }, LIMITS)).toMatch(/at least 1 hour/);
    expect(lifetimeProblem({ kind: "custom", text: "500" }, LIMITS)).toMatch(/at most 168 hours/);
    expect(lifetimeProblem({ kind: "custom", text: "12" }, LIMITS)).toBeNull();
  });
});

describe("the picked apps", () => {
  it("keeps the order they were picked in, and drops one no longer on offer", () => {
    expect(pickedApps(["a3", "gone", "a1"], APPS).map((each) => each.id)).toEqual(["a3", "a1"]);
  });

  it("ticks and unticks, and ignores a tick past the limit", () => {
    expect(toggleApp([], "a1", true, LIMITS)).toEqual(["a1"]);
    expect(toggleApp(["a1", "a2"], "a1", false, LIMITS)).toEqual(["a2"]);
    // Ticking what is already ticked doesn't list it twice: the server refuses a repeat.
    expect(toggleApp(["a1"], "a1", true, LIMITS)).toEqual(["a1"]);
    const five = ["a1", "a2", "a3", "a4", "a5"];
    expect(toggleApp(five, "a6", true, LIMITS)).toEqual(five);
  });
});

describe("sandboxMintRequest", () => {
  const picked = [APPS[0], APPS[1]];

  it("builds the pinned body: kind, app ids and hours, and none of a personal token's fields", () => {
    expect(
      sandboxMintRequest("  refunds task ", picked, { kind: "preset", hours: 8 }, LIMITS)
    ).toEqual({
      name: "refunds task",
      kind: "sandbox_agent",
      apps: ["a1", "a2"],
      expires_in_hours: 8
    });
  });

  it("is not built without a name, an app or a usable lifetime", () => {
    const eight = { kind: "preset", hours: 8 } as const;
    expect(sandboxMintRequest("  ", picked, eight, LIMITS)).toBeUndefined();
    expect(sandboxMintRequest("task", [], eight, LIMITS)).toBeUndefined();
    expect(sandboxMintRequest("task", APPS, eight, LIMITS)).toBeUndefined();
    expect(
      sandboxMintRequest("task", picked, { kind: "custom", text: "" }, LIMITS)
    ).toBeUndefined();
    expect(
      sandboxMintRequest("task", picked, { kind: "custom", text: "169" }, LIMITS)
    ).toBeUndefined();
    expect(
      sandboxMintRequest("task", picked, { kind: "custom", text: "168" }, LIMITS)
    ).toMatchObject({ expires_in_hours: 168 });
  });
});
