// Turn a plan into media with the agentic runner, in two passes:
//
//   record  the model drives each step under the capture profile (no video);
//           every step's actions are written to the record's actions.json.
//   replay  no model: the recording is replayed, slowed down and filmed, and
//           the final screen is judged against the plan's `expect`.
//
// Every picture ever posted comes out of the replay pass — at review time and
// at release — so they all come from the same code under the same profile.
// A release starts at replay; only when that fails (the UI moved on) does it
// re-record, and a picture the judge rejects is never kept.
//
// Both passes run on one instance, so anything a plan creates is named with
// ${SHOWCASE_RUN}: a fresh token per pass, recorded as the placeholder, so the
// replay makes a new row instead of colliding with the one the record made.

import { randomBytes } from "node:crypto";
import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { CostMeter } from "../agentic/runner/budget";
import type { CaptureProfile } from "../agentic/runner/capture-profile";
import { bespokeRuntime } from "../agentic/runner/runtimes/bespoke";
import type { CaseRunResult, FlowTest } from "../agentic/runner/types";
import { DEFAULT_SETTINGS } from "../agentic/runner/yaml-loader";
import type { Session } from "./session";
import { ACTIONS_FILE, type ShowcasePlan } from "./types";

export interface CaptureInput {
  pr: number;
  plan: ShowcasePlan;
  /** `plan.start_path` with placeholders resolved for this instance. */
  startPath: string;
  session: Session;
  /** The record directory: actions.json lives here, media under media/. */
  dir: string;
  apiKey: string;
  /** False: replay only. True: re-record when there is no recording or it no longer replays. */
  allowModel: boolean;
  /** Every model turn and judge call is charged here; the run stops at its limit. */
  meter: CostMeter;
}

export interface CaptureFailure {
  outcome: "rejected" | "failed";
  reason: string;
  /** A fault outside the plan (an API error): the next push may try again. */
  retryable?: boolean;
}

export type CaptureResult =
  | {
      ok: true;
      screenshot: string;
      video?: string;
      videoStartMs: number;
      recorded: boolean;
      cost_usd: number;
    }
  | ({ ok: false; cost_usd: number } & CaptureFailure);

export const VIEWPORT = { width: 1440, height: 900 };
export const RUN_TOKEN_VAR = "SHOWCASE_RUN";
const SLOW_MO_MS = 400;
// Sonnet, not the suite's Haiku: on a detailed claim Haiku passed a screen in
// the record pass and rejected the same screen in the replay. A wrong verdict
// costs a picture; a judge call costs a cent.
export const JUDGE_MODEL = "claude-sonnet-4-6";
const MAX_TURNS_PER_STEP = 6;
// An error toast from a background call the page makes is not the
// feature, and a person sees it vanish in seconds. Hidden in the posted frame
// only — the judge still sees the page as it is.
const HIDE_IN_SCREENSHOT =
  '[data-sonner-toast][data-type="error"] { visibility: hidden !important; }';
// The seeded Demo workspace's id is a v5 UUID of a fixed name, the same on every
// instance; any other id in a recorded URL was minted by one seed.
const STABLE_ID = "70787bb2-e11b-5488-b2c3-02e60d5fc7d3";
const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi;

export function showcaseFlow(pr: number, plan: ShowcasePlan, runToken = ""): FlowTest {
  return {
    name: `showcase pr-${pr}`,
    // The cache key hashes this string, so it must not depend on where the
    // checkout lives — a recording made in PR CI replays at release.
    file: `showcase:pr-${pr}`,
    target: "any",
    settings: {
      ...DEFAULT_SETTINGS,
      judge_model: JUDGE_MODEL,
      // Model turns per step. The suite allows 30; a showcase step is one
      // plain click or keystroke, and an agent that cannot find its control
      // in six turns is exploring — one run that wandered cost $8 at 30.
      max_steps: MAX_TURNS_PER_STEP,
      trace: "never",
      cache_actions: true,
      backend_mode: "cloud"
    },
    setup: [],
    cases: [
      {
        name: "showcase",
        tags: [],
        // Steps keep the placeholder: the cache key hashes the step text, and
        // the runner expands it at the model and at Playwright.
        steps: [...plan.steps.map((act) => ({ act })), { wait_for: "network_idle" }],
        // The judge is not expanded by the runner, so the claim names this pass's token.
        expect: [{ judge: plan.expect.split(`\${${RUN_TOKEN_VAR}}`).join(runToken) }]
      }
    ]
  };
}

function profile(dir: string, startPath: string, filmed: boolean): CaptureProfile {
  return {
    dir,
    startPath,
    viewport: VIEWPORT,
    deviceScaleFactor: 2,
    slowMoMs: filmed ? SLOW_MO_MS : 0,
    video: filmed,
    screenshotStyle: HIDE_IN_SCREENSHOT
  };
}

async function runPass(input: CaptureInput, mode: "record" | "replay"): Promise<CaseRunResult> {
  const token = randomBytes(4).toString("hex");
  process.env[RUN_TOKEN_VAR] = token;
  const flow = showcaseFlow(input.pr, input.plan, token);
  const filmed = mode === "replay";
  return bespokeRuntime.runCase({
    flow,
    testCase: flow.cases[0],
    apiKey: input.apiKey,
    debug: Boolean(process.env.DEBUG),
    headless: true,
    cachePath: join(input.dir, ACTIONS_FILE),
    cacheMode: mode,
    session: input.session,
    meter: input.meter,
    capture: profile(join(input.dir, filmed ? "media" : "record-pass"), input.startPath, filmed)
  });
}

/** Why a pass did not produce keepable media, or undefined when it did. */
export function passFailure(result: CaseRunResult): CaptureFailure | undefined {
  if (result.error) return { outcome: "failed", reason: result.error };
  const judged = result.expect_results.find((e) => !e.passed);
  if (judged) {
    const why = judged.rationale ?? judged.claim;
    // judge.ts fails closed on an API error or a reply it cannot parse — a
    // fault of the call, not a verdict on the screen.
    if (/^(judge API error|unparsable judge response)/.test(why)) {
      return { outcome: "failed", reason: why, retryable: true };
    }
    return { outcome: "rejected", reason: `the final screen did not show it: ${why}` };
  }
  if (!result.passed) return { outcome: "failed", reason: "the run did not pass" };
  return undefined;
}

/**
 * A recording must replay on another instance. Rewrite a navigation the model
 * made to an absolute URL down to its path (a PR run serves the app on another
 * port than a release), and refuse one that carries an id a seed minted.
 */
export function portableRecording(actionsPath: string): string | undefined {
  if (!existsSync(actionsPath)) return undefined;
  const cache = JSON.parse(readFileSync(actionsPath, "utf-8")) as {
    entries: Record<string, { actions: { tool: string; args: Record<string, unknown> }[] }>;
  };
  for (const entry of Object.values(cache.entries)) {
    for (const action of entry.actions) {
      const url = action.args.url;
      if (action.tool !== "browser_navigate" || typeof url !== "string") continue;
      const path = /^[a-z]+:\/\//i.test(url) ? url.replace(/^[a-z]+:\/\/[^/]+/i, "") || "/" : url;
      const ids = (path.match(UUID) ?? []).filter((id) => id.toLowerCase() !== STABLE_ID);
      if (ids.length > 0) {
        return `the agent navigated by URL to an id this instance's seed minted (${path}) — it would not replay elsewhere`;
      }
      action.args.url = path;
    }
  }
  writeFileSync(actionsPath, JSON.stringify(cache, null, 2));
  return undefined;
}

async function attempt(input: CaptureInput, mode: "record" | "replay") {
  try {
    const result = await runPass(input, mode);
    return { result, failure: passFailure(result), cost: result.cost_usd };
  } catch (err) {
    const reason = err instanceof Error ? err.message : String(err);
    return { result: undefined, failure: { outcome: "failed" as const, reason }, cost: 0 };
  }
}

/** Once the meter has stopped a call, nothing after it may run. */
function budgetStop(input: CaptureInput, cost: number): CaptureResult | undefined {
  const stopped = input.meter.stopped;
  return stopped ? { ok: false, outcome: "failed", reason: stopped, cost_usd: cost } : undefined;
}

export async function capture(input: CaptureInput): Promise<CaptureResult> {
  let cost = 0;
  const actions = join(input.dir, ACTIONS_FILE);

  // No steps, nothing to record: the start page is the picture, and its one
  // (filmed, judged) pass is the replay.
  if (existsSync(actions) || input.plan.steps.length === 0) {
    const replay = await attempt(input, "replay");
    cost += replay.cost;
    if (!replay.failure && replay.result?.capture) {
      return { ok: true, ...replay.result.capture, recorded: false, cost_usd: cost };
    }
    const stop = budgetStop(input, cost);
    if (stop) return stop;
    // Re-record only a recording that broke. `rejected` means it replayed and
    // the judge saw the wrong screen — the same plan re-recorded goes to the
    // same place, so that is a planning problem, not a model-spend one.
    const broke = replay.failure?.outcome !== "rejected";
    if (!input.allowModel || !broke) {
      return { ok: false, ...(replay.failure ?? noMedia()), cost_usd: cost };
    }
  } else if (!input.allowModel) {
    return {
      ok: false,
      outcome: "failed",
      reason: "there is no recording to replay",
      cost_usd: cost
    };
  }

  rmSync(actions, { force: true });
  const record = await attempt(input, "record");
  cost += record.cost;
  if (record.failure)
    return budgetStop(input, cost) ?? { ok: false, ...record.failure, cost_usd: cost };
  const unportable = portableRecording(actions);
  if (unportable) return { ok: false, outcome: "failed", reason: unportable, cost_usd: cost };

  const replay = await attempt(input, "replay");
  cost += replay.cost;
  if (replay.failure || !replay.result?.capture) {
    return (
      budgetStop(input, cost) ?? { ok: false, ...(replay.failure ?? noMedia()), cost_usd: cost }
    );
  }
  return { ok: true, ...replay.result.capture, recorded: true, cost_usd: cost };
}

function noMedia(): CaptureFailure {
  return { outcome: "failed", reason: "the replay produced no media" };
}
