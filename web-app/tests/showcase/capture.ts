// Turn a plan into media with the agentic runner.
//
//   drive   the model follows the plan's steps; the screen it ends on is
//           settled, judged against the plan's `expect`, and kept. For a
//           screenshot plan that is the whole capture.
//   replay  no model: what the drive recorded is replayed, slowed down and
//           filmed, and the final screen is judged. Only a video plan needs
//           it. (A plan with no steps is one such pass over its start page.)
//
// A still does NOT wait on a replay. It used to: every capture recorded, then
// had to replay before anything was kept. On real PRs that threw away pictures
// the model had already reached and the judge had already passed — a replay
// judged before a dashboard finished loading; a step whose click worked but
// had no selector durable enough to record. That rule was there so a recording
// made at review time would replay at release; a release that captures for
// itself has no use for it. So the driven pass's judged frame is the picture,
// and a replay that fails costs the video, never the still.
//
// A recording is only ever replayed by the run that made it, on the instance
// it was made on.
//
// Passes share one instance, so anything a plan creates is named with
// ${SHOWCASE_RUN}: a fresh token per pass, recorded as the placeholder, so a
// replay makes a new row instead of colliding with the one the drive made.

import { randomBytes } from "node:crypto";
import { copyFileSync, mkdirSync, rmSync } from "node:fs";
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
      /** How the picture was made, for the record's `reason`. */
      how: string;
      cost_usd: number;
    }
  | ({ ok: false; cost_usd: number } & CaptureFailure);

export const VIEWPORT = { width: 1440, height: 900 };
const RUN_TOKEN_VAR = "SHOWCASE_RUN";
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

/** `recorded: false` is a drive nothing will replay: no step needs a selector durable enough to record. */
export function showcaseFlow(
  pr: number,
  plan: ShowcasePlan,
  runToken = "",
  recorded = true
): FlowTest {
  return {
    name: `showcase pr-${pr}`,
    // The cache key hashes this string; a name of its own keeps it off the
    // suite's flows and off wherever the checkout happens to live.
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
      cache_actions: recorded,
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

/**
 * `drive`: the model follows the steps and nothing is recorded. `record`: the
 * same, and every step must leave a recording a replay can follow. `replay`:
 * no model, filmed.
 */
type Pass = "drive" | "record" | "replay";

async function runPass(input: CaptureInput, mode: Pass): Promise<CaseRunResult> {
  const token = randomBytes(4).toString("hex");
  process.env[RUN_TOKEN_VAR] = token;
  const flow = showcaseFlow(input.pr, input.plan, token, mode !== "drive");
  const filmed = mode === "replay";
  return bespokeRuntime.runCase({
    flow,
    testCase: flow.cases[0],
    apiKey: input.apiKey,
    debug: Boolean(process.env.DEBUG),
    headless: true,
    cachePath: join(input.dir, ACTIONS_FILE),
    // Unset is the runner's plain mode: with `cache_actions` off, the model
    // drives every step and a step it cannot record is not an error.
    cacheMode: mode === "drive" ? undefined : mode,
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

async function attempt(input: CaptureInput, mode: Pass) {
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

/** The driven pass's judged frame, put where posted media lives. */
function keepStill(input: CaptureInput, frame: string): string {
  const media = join(input.dir, "media");
  // A replay that failed may have left its own last frame and film there.
  rmSync(media, { recursive: true, force: true });
  mkdirSync(media, { recursive: true });
  const still = join(media, "screenshot.png");
  copyFileSync(frame, still);
  return still;
}

/**
 * Film what the drive just recorded. `filmed` is absent when the replay broke
 * or its judge disagreed, and the caller keeps the still.
 */
async function film(input: CaptureInput) {
  const replay = await attempt(input, "replay");
  const filmed = !replay.failure && replay.result?.capture ? replay.result.capture : undefined;
  return {
    cost: replay.cost,
    filmed,
    why: replay.failure?.reason ?? "the replay produced no media"
  };
}

export async function capture(input: CaptureInput): Promise<CaptureResult> {
  // Whatever a first plan recorded here is not this plan's.
  rmSync(join(input.dir, ACTIONS_FILE), { force: true });

  // No steps: the start page is the picture, and there is nothing for a model
  // to do. One filmed, judged pass over it.
  if (input.plan.steps.length === 0) {
    const only = await attempt(input, "replay");
    if (!only.failure && only.result?.capture) {
      const how = "the start page shows it";
      return { ok: true, ...only.result.capture, how, cost_usd: only.cost };
    }
    return (
      budgetStop(input, only.cost) ?? {
        ok: false,
        ...(only.failure ?? noMedia()),
        cost_usd: only.cost
      }
    );
  }

  const wantVideo = input.plan.media === "video";
  const driven = await attempt(input, wantVideo ? "record" : "drive");
  let cost = driven.cost;
  const frame = driven.result?.capture?.screenshot;
  if (driven.failure || !frame) {
    return (
      budgetStop(input, cost) ?? { ok: false, ...(driven.failure ?? noMedia()), cost_usd: cost }
    );
  }
  if (!wantVideo) {
    const screenshot = keepStill(input, frame);
    return { ok: true, screenshot, videoStartMs: 0, how: "driven and judged", cost_usd: cost };
  }

  const filming = await film(input);
  cost += filming.cost;
  if (filming.filmed) {
    return { ok: true, ...filming.filmed, how: "driven, then replayed on film", cost_usd: cost };
  }
  const screenshot = keepStill(input, frame);
  const how = `driven and judged; no video — ${filming.why}`;
  return { ok: true, screenshot, videoStartMs: 0, how, cost_usd: cost };
}

function noMedia(): CaptureFailure {
  return { outcome: "failed", reason: "the replay produced no media" };
}
