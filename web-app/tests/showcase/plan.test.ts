import { describe, expect, it } from "vitest";
import { createMeter } from "../agentic/runner/budget";
import { resolvePlaceholders } from "./inventory";
import {
  buildPlanPrompt,
  foreignPlaceholders,
  inCoreFlow,
  type PlanClient,
  PlanRejected,
  requestPlanWith,
  validatePlan
} from "./plan";
import type { ShowcasePlan } from "./types";

/** `${name}` as a step would carry it — written this way so it is not mistaken for a template. */
const ph = (name: string) => `\${${name}}`;

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
    [{ start_path: "/localhost/x" }, /core flow/],
    [
      { steps: [`Type '${ph("SLACK_BOT_TOKEN")}' into the search box`] },
      /only placeholder.*SLACK_BOT_TOKEN/
    ],
    [{ expect: `The page shows ${ph("ANTHROPIC_API_KEY")}` }, /only placeholder.*ANTHROPIC_API_KEY/]
  ])("rejects %o", (patch, why) => {
    expect(() => validatePlan({ ...plan, ...patch }, inventory)).toThrow(PlanRejected);
    expect(() => validatePlan({ ...plan, ...patch }, inventory)).toThrow(why);
  });
});

// The runner types whatever `${NAME}` a step carries, read from the environment,
// into a page that is filmed and posted. A plan is model output over PR text.
describe("foreignPlaceholders", () => {
  it("lets a plan name the per-run token and nothing else", () => {
    const steps = [`Type 'Front counter ${ph("SHOWCASE_RUN")}' into the Name field`];
    expect(foreignPlaceholders({ ...plan, steps })).toEqual([]);
    expect(validatePlan({ ...plan, steps }, inventory).steps).toEqual(steps);
  });
  it("names every other one, wherever in the plan it is", () => {
    const sneaky = {
      ...plan,
      start_path: `/local?x=${ph("GH_TOKEN")}`,
      steps: [`Type ${ph("SLACK_BOT_TOKEN")}`, `Type ${ph("SLACK_BOT_TOKEN")} again`],
      headline: `Now with ${ph("ANTHROPIC_API_KEY")}`
    };
    expect(foreignPlaceholders(sneaky).sort()).toEqual([
      "ANTHROPIC_API_KEY",
      "GH_TOKEN",
      "SLACK_BOT_TOKEN"
    ]);
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
  it("tells the planner what the Demo workspace holds, when that is known", () => {
    expect(buildPlanPrompt(input)).not.toMatch(/Demo workspace's contents/);
    const demo = {
      databases: [{ name: "local", type: "duckdb", hasData: true }],
      files: [{ kind: "Automations", paths: ["procedures/active_users.automation.yml"] }]
    };
    expect(buildPlanPrompt({ ...input, demo })).toMatch(
      /Demo workspace's contents[\s\S]*`local` \(duckdb\)[\s\S]*active_users\.automation\.yml/
    );
  });
  it("names workspaces by placeholder, never by id", () => {
    const prompt = buildPlanPrompt(input);
    expect(prompt).toContain("{ws:local/Demo}");
    expect(prompt).not.toContain("70787bb2");
  });
});

describe("requestPlanWith", () => {
  const input = {
    pr: 7,
    title: "fix: the rest of the noticed bugs",
    body: "",
    diff: "",
    entries: [],
    appRoutes: "",
    routeBuilders: "",
    inventory
  };
  const usage = { input_tokens: 1000, output_tokens: 500 };
  /** A planner that answers each call in turn, and remembers how hard it was asked to think. */
  function planner(...replies: object[]) {
    const efforts: string[] = [];
    const parse = async (req: { output_config: { effort: string } }) => {
      efforts.push(req.output_config.effort);
      return { usage, ...replies[efforts.length - 1] };
    };
    return { client: { messages: { parse } } as unknown as PlanClient, efforts };
  }
  const wrote = { stop_reason: "end_turn", parsed_output: plan };
  const thoughtItAllAway = { stop_reason: "max_tokens", parsed_output: null };

  it("asks once when the plan comes back", async () => {
    const { client, efforts } = planner(wrote);
    expect((await requestPlanWith(client, input)).plan).toEqual(plan);
    expect(efforts).toEqual(["medium"]);
  });
  // #3456, a PR of many unrelated fixes: the whole allowance went on thinking,
  // no plan was written, and the one PR in the release with a screen to show was lost.
  it("asks again with less thinking when the first call wrote no plan", async () => {
    const { client, efforts } = planner(thoughtItAllAway, wrote);
    expect((await requestPlanWith(client, input)).plan).toEqual(plan);
    expect(efforts).toEqual(["medium", "low"]);
  });
  it("gives up after that one retry, and charges both calls", async () => {
    const { client, efforts } = planner(thoughtItAllAway, thoughtItAllAway);
    const meter = createMeter(0.5);
    await expect(requestPlanWith(client, input, meter)).rejects.toThrow(/ran out of output tokens/);
    expect(efforts).toEqual(["medium", "low"]);
    expect(meter.spentUsd).toBeGreaterThan(0);
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
