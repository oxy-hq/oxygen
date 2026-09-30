import { describe, expect, it } from "vitest";
import { resolvePlaceholders } from "./inventory";
import { buildPlanPrompt, inCoreFlow, PlanRejected, validatePlan } from "./plan";
import type { ShowcasePlan } from "./types";

const inventory = {
  workspaces: [{ org_slug: "local", name: "Demo", id: "70787bb2-e11b-5488-b2c3-02e60d5fc7d3" }],
  apps: []
};

const plan: ShowcasePlan = {
  verdict: "show",
  reason: "a new button",
  headline: "Automations can be paused",
  start_path: "/local/workspaces/{ws:local/Demo}/automations",
  steps: ["Click the first automation", "Click Pause"],
  expect: "The automation shows a Paused badge",
  media: "video"
};

describe("resolvePlaceholders", () => {
  it("swaps a workspace placeholder for this instance's id", () => {
    expect(resolvePlaceholders(plan.start_path, inventory)).toBe(
      "/local/workspaces/70787bb2-e11b-5488-b2c3-02e60d5fc7d3/automations"
    );
  });
  it("names the workspace it cannot find", () => {
    expect(() => resolvePlaceholders("/acme/workspaces/{ws:acme/Sales}", inventory)).toThrow(
      /Sales/
    );
  });
});

describe("validatePlan", () => {
  it("passes a well-formed plan", () => {
    expect(validatePlan(plan, inventory)).toEqual(plan);
  });
  it("empties the steps of a plan that shows nothing", () => {
    expect(validatePlan({ ...plan, verdict: "not_visual" }, inventory).steps).toEqual([]);
  });
  it.each([
    [{ start_path: "local/x" }, /absolute/],
    [{ start_path: "https://x.test/local" }, /URL/],
    [{ start_path: "/local/workspaces/70787bb2-e11b-5488-b2c3-02e60d5fc7d3" }, /raw id/],
    [{ steps: ["Go to /local/workspaces/abc"] }, /URL or id/],
    [{ steps: Array(9).fill("Click") }, /at most/],
    [{ expect: " " }, /expect/],
    [{ start_path: "/local/workspaces/{ws:local/Nope}" }, /Nope/],
    [{ start_path: "/acme/home" }, /core flow/],
    [{ start_path: "/partners" }, /core flow/],
    [{ start_path: "/localhost/x" }, /core flow/]
  ])("rejects %o", (patch, why) => {
    expect(() => validatePlan({ ...plan, ...patch }, inventory)).toThrow(PlanRejected);
    expect(() => validatePlan({ ...plan, ...patch }, inventory)).toThrow(why);
  });
});

describe("buildPlanPrompt", () => {
  const input = {
    pr: 7,
    title: "feat: pause automations",
    body: "Adds pause.",
    diff: "x".repeat(70_000),
    entries: [],
    appRoutes: "<Routes/>",
    routeBuilders: "const ROUTES = {}",
    inventory
  };
  it("says out loud when the diff was cut", () => {
    expect(buildPlanPrompt(input)).toMatch(/diff truncated: 30000 more characters not shown/);
  });
  it("carries the author's steer only when there is one", () => {
    expect(buildPlanPrompt(input)).not.toMatch(/author's steer/);
    expect(buildPlanPrompt({ ...input, hint: "Show the badge" })).toMatch(
      /author's steer[\s\S]*Show the badge/
    );
  });
  it("names workspaces by placeholder, never by id", () => {
    const prompt = buildPlanPrompt(input);
    expect(prompt).toContain("{ws:local/Demo}");
    expect(prompt).not.toContain("70787bb2");
  });
});

describe("inCoreFlow", () => {
  it("admits home, the admin console and the showcase org — segment-bounded", () => {
    expect(inCoreFlow("/")).toBe(true);
    expect(inCoreFlow("/admin/apps?tab=all")).toBe(true);
    expect(inCoreFlow("/local/workspaces/{ws:local/Demo}/ide")).toBe(true);
    expect(inCoreFlow("/local")).toBe(true);
    expect(inCoreFlow("/localhost")).toBe(false);
    expect(inCoreFlow("/acme")).toBe(false);
    expect(inCoreFlow("/partners/clients")).toBe(false);
  });
});
