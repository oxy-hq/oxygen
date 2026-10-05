// One PR through the showcase: filter → plan → capture → record. A release
// calls `showcasePr` for each PR it shipped, on the build it shipped; a preview
// run by hand calls it for one PR.

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { BudgetExceeded, type CostMeter, remaining } from "../agentic/runner/budget";
import { capture } from "./capture";
import { readDemoWorkspace } from "./demo-workspace";
import { detect, isUiSource } from "./detect";
import { type PrSnapshot, prDiff, prFacts } from "./github";
import { APP_FILE, entriesReaching, pageEntries, reverseGraph, routeTable } from "./imports";
import { type Inventory, onlyOrg, readInventory, resolvePlaceholders } from "./inventory";
import { toMp4 } from "./media";
import { startPageTree } from "./page-snapshot";
import { type PlanInput, requestPlan } from "./plan";
import { mintSession, type Session } from "./session";
import { ACTIONS_FILE, SHOWCASE_ORG, type ShowcasePlan, type ShowcaseRecord } from "./types";

export interface PipelineEnv {
  repo: string;
  apiKey: string;
  /** Where the browser points (the SPA). */
  baseUrl: string;
  /** Where dev-login is asked; the API server. */
  backendUrl: string;
  databaseUrl: string;
}

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
// The web-app source the planner reads its routes and screens from. A release
// points this at a checkout of the commit it shipped: this tooling runs from
// the default branch, whose routes may already have moved on.
const SOURCE_ROOT = process.env.SHOWCASE_SOURCE_ROOT
  ? resolve(process.env.SHOWCASE_SOURCE_ROOT)
  : REPO_ROOT;
// The directory `oxy seed` built the Demo workspace from (showcase-boot.sh reads the same variable).
const EXAMPLES_DIR = process.env.SHOWCASE_EXAMPLES ?? join(REPO_ROOT, "examples");
const ROUTE_BUILDERS = "web-app/src/libs/utils/routes.ts";
// A corrected plan is only worth starting with enough left for a plan and a
// short capture; below this it would stop half-way and buy nothing.
const RETRY_FLOOR_USD = 0.25;

function readRepoFile(path: string): string | undefined {
  const full = join(SOURCE_ROOT, path);
  return existsSync(full) ? readFileSync(full, "utf-8") : undefined;
}

function webAppSources(): string[] {
  const res = spawnSync("git", ["ls-files", "web-app/src"], {
    cwd: SOURCE_ROOT,
    encoding: "utf-8"
  });
  if (res.status !== 0) throw new Error(`git ls-files failed: ${res.stderr}`);
  return res.stdout.split("\n").filter((f) => /\.(tsx?)$/.test(f));
}

function screensFor(changed: string[]) {
  const files = webAppSources();
  const importers = reverseGraph(files, readRepoFile);
  const entries = pageEntries(readRepoFile(APP_FILE) ?? "", new Set(files));
  return entriesReaching(changed.filter(isUiSource), importers, entries);
}

function baseRecord(pr: number, title: string, headSha: string): ShowcaseRecord {
  return {
    version: 1,
    pr,
    title,
    head_sha: headSha,
    outcome: "failed",
    reason: "",
    cost_usd: 0,
    captured_at: new Date().toISOString()
  };
}

/**
 * Everything one PR's showcase spends is charged to `meter`, which stops the
 * run at its limit; the record's `cost_usd` is what the meter saw.
 */
export async function showcasePr(
  env: PipelineEnv,
  pr: number,
  dir: string,
  meter: CostMeter,
  facts: PrSnapshot = prFacts(env.repo, pr)
): Promise<ShowcaseRecord> {
  mkdirSync(dir, { recursive: true });
  const record = baseRecord(pr, facts.title, facts.headSha);
  const detection = detect(facts);
  if (!detection.candidate)
    return { ...record, outcome: "not_candidate", reason: detection.reason };

  const diff = prDiff(env.repo, pr);
  const input: PlanInput = {
    pr,
    title: facts.title,
    body: facts.body,
    hint: detection.hint,
    diff,
    entries: screensFor(facts.files),
    appRoutes: routeTable(readRepoFile(APP_FILE) ?? ""),
    routeBuilders: readRepoFile(ROUTE_BUILDERS) ?? "",
    inventory: onlyOrg(readInventory(env.databaseUrl), SHOWCASE_ORG),
    demo: readDemoWorkspace(EXAMPLES_DIR)
  };
  const result = await attemptTwice(env, input, record, dir, meter);
  return settleCost(result, meter);
}

async function attemptTwice(
  env: PipelineEnv,
  input: PlanInput,
  record: ShowcaseRecord,
  dir: string,
  meter: CostMeter
): Promise<ShowcaseRecord> {
  const first = await guarded(record, () => planAndCapture(env, input, record, dir, meter));
  // A corrected plan fixes a plan: a wrong screen, a step that found nothing.
  // Not a fault outside it — a refused sign-in, an API error — where a second
  // planner call would buy the same failure at twice the price.
  const planFault =
    first.outcome === "rejected" || (first.outcome === "failed" && !first.retryable);
  const worthRetrying = planFault && !meter.stopped;
  if (!worthRetrying || !first.plan || remaining(meter) < RETRY_FLOOR_USD) return first;

  // One corrected plan when the first ran and ended on the wrong screen. The
  // planner wrote its steps from code; now it gets what it could not know
  // from there — where the run ended, and the start page as it really is.
  // Bounded to one, so a PR that cannot be shown costs two attempts, not a loop.
  rmSync(join(dir, ACTIONS_FILE), { force: true });
  const previous = {
    plan: first.plan,
    ended: first.reason,
    startPage: await startPageFor(env, first.plan, input.inventory)
  };
  const retried = await guarded(record, () =>
    planAndCapture(env, { ...input, previous }, record, dir, meter)
  );
  return { ...retried, reason: `${retried.reason} (a first plan ended: ${first.reason})` };
}

/** A throw (the planner over budget, an API error) becomes a record that says why. */
async function guarded(
  record: ShowcaseRecord,
  run: () => Promise<ShowcaseRecord>
): Promise<ShowcaseRecord> {
  try {
    return await run();
  } catch (err) {
    // Anything thrown here is outside the plan — the API, gh, psql — except
    // the meter, whose stop `settleCost` turns into `over_budget`.
    const retryable = !(err instanceof BudgetExceeded);
    return { ...record, outcome: "failed", reason: message(err), retryable };
  }
}

/** The meter is the record of spend; a run it stopped is `over_budget`, not a flake. */
function settleCost(record: ShowcaseRecord, meter: CostMeter): ShowcaseRecord {
  if (record.outcome !== "captured" && meter.stopped) {
    return { ...record, outcome: "over_budget", reason: meter.stopped, cost_usd: meter.spentUsd };
  }
  return { ...record, cost_usd: meter.spentUsd };
}

async function startPageFor(env: PipelineEnv, plan: ShowcasePlan, inventory: Inventory) {
  try {
    const startPath = resolvePlaceholders(plan.start_path, inventory);
    const session = await mintSession(env.backendUrl);
    return await startPageTree(env.baseUrl, startPath, session);
  } catch (err) {
    return `(the start page could not be read: ${message(err)})`;
  }
}

async function planAndCapture(
  env: PipelineEnv,
  input: PlanInput,
  record: ShowcaseRecord,
  dir: string,
  meter: CostMeter
): Promise<ShowcaseRecord> {
  const planned = await requestPlan(env.apiKey, input, meter);
  const withPlan = { ...record, plan: planned.plan };
  if (planned.plan.verdict !== "show") {
    return { ...withPlan, outcome: planned.plan.verdict, reason: planned.plan.reason };
  }
  return captureInto(env, withPlan, planned.plan, dir, meter);
}

async function captureInto(
  env: PipelineEnv,
  record: ShowcaseRecord,
  plan: ShowcasePlan,
  dir: string,
  meter: CostMeter
): Promise<ShowcaseRecord> {
  let startPath: string;
  try {
    startPath = resolvePlaceholders(plan.start_path, readInventory(env.databaseUrl));
  } catch (err) {
    return { ...record, outcome: "needs_seed", reason: message(err) };
  }
  let session: Session;
  try {
    session = await mintSession(env.backendUrl);
  } catch (err) {
    return { ...record, outcome: "failed", reason: message(err), retryable: true };
  }
  const result = await capture({
    pr: record.pr,
    plan,
    startPath,
    session,
    dir,
    apiKey: env.apiKey,
    meter
  });
  if (!result.ok) {
    return {
      ...record,
      outcome: result.outcome,
      reason: result.reason,
      retryable: result.retryable
    };
  }

  const video = result.video
    ? toMp4(result.video, join(dir, "media", "video.mp4"), result.videoStartMs)
    : undefined;
  return {
    ...record,
    outcome: "captured",
    reason: result.how,
    // A plan with no steps records nothing; its replay is the start page.
    actions: existsSync(join(dir, ACTIONS_FILE)) ? ACTIONS_FILE : undefined,
    screenshot: "media/screenshot.png",
    video: video ? "media/video.mp4" : undefined
  };
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
