import type { CostMeter } from "../budget";
import type { CaptureProfile } from "../capture-profile";
import type { CaseRunResult, FlowCase, FlowTest } from "../types";

export interface RuntimeContext {
  flow: FlowTest;
  testCase: FlowCase;
  apiKey: string;
  debug: boolean;
  headless: boolean;
  /** Action cache file. Defaults to the suite's shared `.cache/bespoke-actions.json`. */
  cachePath?: string;
  /**
   * For callers that own their cache file (the release showcase). Unset keeps
   * the suite's behaviour: replay when cached, the model otherwise, Tier-2
   * staging when a replay breaks.
   * - `record`: the model drives every step and every step's recording is
   *   written — an empty one too, so a replay can tell "changed nothing" from
   *   "never recorded". A recording that cannot replay fails the step.
   * - `replay`: no model at all. A step with no recording, or one that no
   *   longer resolves, fails the case.
   */
  cacheMode?: "record" | "replay";
  /** Sign in as this session instead of OXY_SESSION_TOKEN / OXY_SESSION_USER. */
  session?: { token: string; user: string };
  /** Record a screenshot (and optionally video) under a fixed browser profile. */
  capture?: CaptureProfile;
  /** Hard spend limit: every model turn is charged, and the one that crosses it fails the step. */
  meter?: CostMeter;
}

export interface Runtime {
  readonly name: "bespoke";
  /**
   * Open a browser session, run setup, execute the case steps, evaluate
   * `expect[]`, and tear down. The runtime owns the page lifecycle so a
   * future alternative implementation can manage its own browser if needed.
   */
  runCase(ctx: RuntimeContext): Promise<CaseRunResult>;
}
