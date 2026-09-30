// The planner: one model call that reads a PR and decides whether it has
// something to show, and if so exactly how to reach it. Its output is data —
// a plan the runner executes, records, and later replays without a model —
// so everything after this call is deterministic.

import Anthropic from "@anthropic-ai/sdk";
import { jsonSchemaOutputFormat } from "@anthropic-ai/sdk/helpers/json-schema";
import {
  bytesOf,
  type CostMeter,
  charge,
  ensurePriced,
  reserve,
  worstCaseUsd
} from "../agentic/runner/budget";
import { computeCost } from "../agentic/runner/pricing";
import type { EntryHit } from "./imports";
import { describeInventory, type Inventory, resolvePlaceholders } from "./inventory";
import { SHOWCASE_ORG, type ShowcasePlan } from "./types";

export interface PlanInput {
  pr: number;
  title: string;
  body: string;
  hint?: string;
  diff: string;
  entries: EntryHit[];
  appRoutes: string;
  routeBuilders: string;
  inventory: Inventory;
  /** A plan already tried on this instance, how its run ended, and the start page's accessibility tree. */
  previous?: { plan: ShowcasePlan; ended: string; startPage?: string };
}

const DIFF_BUDGET = 40_000;
const MAX_STEPS = 6;
const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i;

const PLAN_SCHEMA = {
  type: "object",
  additionalProperties: false,
  required: ["verdict", "reason", "headline", "start_path", "steps", "expect", "media"],
  properties: {
    verdict: { type: "string", enum: ["show", "not_visual", "needs_seed"] },
    reason: { type: "string" },
    headline: { type: "string" },
    start_path: { type: "string" },
    steps: { type: "array", items: { type: "string" } },
    expect: { type: "string" },
    media: { type: "string", enum: ["screenshot", "video"] }
  }
} as const;

const SYSTEM = `You plan one screenshot or short screen recording that shows a product change in Oxygen, a data and AI platform (web app: React). The picture goes in the team's release channel under the release note, read by product, support and leadership — not engineers.

You get a pull request (title, description, diff), the page components its changed files are rendered by, the app's route table, and what a freshly seeded demo instance contains. A browser agent will follow your plan on that instance: it is already signed in as a platform Global Admin who also owns org \`${SHOWCASE_ORG}\`, so every product page and nearly every /admin page is open to it; it opens start_path, then does each step by clicking and typing.

Show the change in the core flow: org \`${SHOWCASE_ORG}\`, its "Demo" workspace (built from the repository's examples: agents, automations, data apps, the semantic model, seeded chat threads), its example custom apps, and the admin console at /admin. Anything else is out of scope and its verdict is "needs_seed", decided now rather than tried: other orgs, the partner console (/partners), the store tablet (/kiosk) and crew sign-in, and the two owner-only admin pages (the billing queue and Global-admin management).

Decide the verdict first:
- "show": a reader would see the change on screen, and the seeded instance can reach it.
- "not_visual": nothing a reader would notice changed on screen (plumbing, error handling nobody triggers, performance, copy nobody reaches).
- "needs_seed": it is visible, but only with data or state the seed does not create (say what is missing in reason).

For "show":
- start_path: an absolute path. Workspace pages live at /<org_slug>/workspaces/<workspace>/..., and a workspace id is written as the placeholder {ws:<org_slug>/<workspace name>} — never a raw id. For the Demo workspace that is exactly /${SHOWCASE_ORG}/workspaces/{ws:${SHOWCASE_ORG}/Demo}/… (e.g. /${SHOWCASE_ORG}/workspaces/{ws:${SHOWCASE_ORG}/Demo}/automations). The URL builders' empty-org-slug branch is the legacy single-workspace mode; this instance is not that.
- steps: at most ${MAX_STEPS}, each one plain action ("Click the Automations tab", "Type 'revenue' into the search box"). Click by visible label; no URLs, no ids, no CSS. Zero steps is right when the start page already shows the change. Stop at the FIRST screen where the change is visible — a new option shown in a list is the picture; do not go on to pick it and fill in what follows. Every extra step is one more place the run can fail. Prefer showing the change on data the seed already has. When a step submits a form, first fill every field the form requires — read the diff and the components for which ones; a submit button stays disabled otherwise.
- The plan runs more than once on the same instance (recorded, then replayed on film, then again at release), so it must still work the second time. Anything a step creates gets \${SHOWCASE_RUN} in its name, written literally — "Type 'Front counter \${SHOWCASE_RUN}' into the Name field" — which becomes a fresh short code on every run. expect may use it too. Never depend on something that can only happen once.
- expect: a short claim a judge can confirm at a glance from a screenshot — the one or two visible things new in this change, named by their on-screen text ("The Preview panel with Device sizes, Draft channel and Request log links"). No layout judgements (widths, columns, order), no claims that something is absent, at most two conditions: a true claim that is hard to see still gets rejected.
- media: "video" only when the change is a behaviour you have to watch (an interaction, a transition, a sequence); otherwise "screenshot".
- headline: one plain sentence for the channel, present tense, no jargon.

For other verdicts fill every field anyway: empty steps, start_path "/", and a headline naming the change.`;

export function buildPlanPrompt(input: PlanInput): string {
  const diff =
    input.diff.length > DIFF_BUDGET
      ? `${input.diff.slice(0, DIFF_BUDGET)}\n[diff truncated: ${input.diff.length - DIFF_BUDGET} more characters not shown]`
      : input.diff;
  const entries = input.entries.length
    ? input.entries.map(
        (e) => `- ${e.entry} (${e.file}) ← ${e.via.slice(1).join(" ← ") || "changed directly"}`
      )
    : ["- none found (the change may not be under a routed page)"];
  return [
    `# Pull request #${input.pr}: ${input.title}`,
    input.body.trim() || "(no description)",
    input.hint ? `## The author's steer (follow it)\n${input.hint}` : "",
    "## Pages that render the changed files",
    ...entries,
    "## Seeded instance",
    describeInventory(input.inventory),
    "## Route table (web-app/src/App.tsx)",
    "```tsx",
    input.appRoutes,
    "```",
    "## URL builders (web-app/src/libs/utils/routes.ts)",
    "```ts",
    input.routeBuilders,
    "```",
    "## Diff",
    "```diff",
    diff,
    "```",
    input.previous ? previousAttempt(input.previous) : ""
  ]
    .filter(Boolean)
    .join("\n\n");
}

function previousAttempt(previous: NonNullable<PlanInput["previous"]>): string {
  return [
    "## A plan was already tried on this instance, and it did not work",
    "```json",
    JSON.stringify(previous.plan, null, 2),
    "```",
    `How it ended: ${previous.ended}`,
    ...(previous.startPage
      ? [
          "",
          "The start page as the agent sees it (accessibility tree) — name controls exactly as they appear here:",
          "```yaml",
          previous.startPage,
          "```"
        ]
      : []),
    "",
    "The browser agent does exactly what the steps say and nothing more. Return a corrected plan — or a different verdict if the change cannot be reached."
  ].join("\n");
}

export class PlanRejected extends Error {}

/** Home, the admin console, or the showcase org — segment-bounded, so `/localhost` is not `/local`. */
export function inCoreFlow(path: string): boolean {
  const bare = path.split(/[?#]/)[0];
  return (
    bare === "/" ||
    [`/admin`, `/${SHOWCASE_ORG}`].some((root) => bare === root || bare.startsWith(`${root}/`))
  );
}

/** Checks the schema cannot express. Throws `PlanRejected` with a reason the model can act on. */
export function validatePlan(plan: ShowcasePlan, inventory: Inventory): ShowcasePlan {
  if (plan.verdict !== "show") return { ...plan, steps: [] };
  const fail = (why: string) => {
    throw new PlanRejected(why);
  };
  if (/https?:/.test(plan.start_path)) fail("start_path must be a path, not a URL");
  if (!plan.start_path.startsWith("/")) fail("start_path must be an absolute path");
  if (UUID.test(plan.start_path)) fail("start_path carries a raw id; use {ws:org/name}");
  if (!inCoreFlow(plan.start_path)) {
    fail(
      `start_path "${plan.start_path}" is outside the core flow: it must be "/", /admin/…, ` +
        `or /${SHOWCASE_ORG}/… (the Demo workspace is /${SHOWCASE_ORG}/workspaces/{ws:${SHOWCASE_ORG}/Demo}/…)`
    );
  }
  if (plan.steps.length > MAX_STEPS) fail(`at most ${MAX_STEPS} steps`);
  for (const step of plan.steps) {
    if (/https?:|\/workspaces\//.test(step) || UUID.test(step))
      fail(`step names a URL or id: "${step}"`);
  }
  if (!plan.expect.trim()) fail("expect is empty");
  if (!plan.headline.trim()) fail("headline is empty");
  try {
    resolvePlaceholders(plan.start_path, inventory);
  } catch (err) {
    fail(err instanceof Error ? err.message : String(err));
  }
  return plan;
}

// Sonnet 4.6 at medium effort: the runner's own model, priced in pricing.ts
// (a metered run refuses an unpriced one), and a few cents a plan. Opus 5 can
// be named with SHOWCASE_PLAN_MODEL; the budget still bounds it.
const MODEL = process.env.SHOWCASE_PLAN_MODEL ?? "claude-sonnet-4-6";
// Thinking counts against it, and the plan itself is a few hundred tokens.
const PLAN_MAX_TOKENS = 6_000;

export interface PlanResult {
  plan: ShowcasePlan;
  model: string;
  cost_usd: number;
}

async function ask(client: Anthropic, user: string, meter?: CostMeter): Promise<PlanResult> {
  ensurePriced(meter, MODEL);
  reserve(meter, worstCaseUsd(MODEL, bytesOf(SYSTEM, user), PLAN_MAX_TOKENS));
  const res = await client.messages.parse({
    model: MODEL,
    max_tokens: PLAN_MAX_TOKENS,
    thinking: { type: "adaptive" },
    output_config: { effort: "medium", format: jsonSchemaOutputFormat(PLAN_SCHEMA) },
    system: SYSTEM,
    messages: [{ role: "user", content: user }]
  });
  const cost_usd = computeCost(MODEL, {
    input: res.usage.input_tokens,
    cached_input: res.usage.cache_read_input_tokens ?? 0,
    cache_creation: res.usage.cache_creation_input_tokens ?? 0,
    output: res.usage.output_tokens
  });
  charge(meter, cost_usd);
  if (res.stop_reason === "refusal") throw new Error("the planner declined this PR");
  if (res.stop_reason === "max_tokens") throw new Error("the planner ran out of output tokens");
  if (!res.parsed_output) throw new Error("the planner returned no parseable plan");
  return { plan: res.parsed_output as ShowcasePlan, model: MODEL, cost_usd };
}

/**
 * Plan and validate, giving the model one chance to fix a plan that fails
 * validation. Every call is charged to `meter` and refused once it is spent.
 */
export async function requestPlan(
  apiKey: string,
  input: PlanInput,
  meter?: CostMeter
): Promise<PlanResult> {
  const client = new Anthropic({ apiKey });
  const prompt = buildPlanPrompt(input);
  const first = await ask(client, prompt, meter);
  try {
    return { ...first, plan: validatePlan(first.plan, input.inventory) };
  } catch (err) {
    if (!(err instanceof PlanRejected)) throw err;
    const retry = `${prompt}\n\n## Your previous plan was rejected\n${err.message}\nReturn a corrected plan.`;
    const second = await ask(client, retry, meter);
    const plan = validatePlan(second.plan, input.inventory);
    return { ...second, plan, cost_usd: first.cost_usd + second.cost_usd };
  }
}
