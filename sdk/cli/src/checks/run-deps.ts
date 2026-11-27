/**
 * What a run of a build's promote checks is given beside the claim: its
 * waits, and where Chromium comes from. The tests shorten and replace them.
 * And the one way a claim becomes the page a check opens.
 */

import type { Creds } from "../apps/resolve.js";
import { credentialHeaders } from "./browser-step.js";
import { type LoadsTarget, loadsTarget } from "./loads-target.js";
import { RUN_CLOCK } from "./page-clock.js";
import type { PwChromium } from "./playwright.js";
import { type Claim, DEFAULT_BACKOFF_MS } from "./promote-api.js";

export interface BuildChecksDeps {
  loadChromium?: () => Promise<PwChromium>;
  /** How often the build's check functions are asked about. */
  settlePollMs: number;
  /** How long they are asked for. */
  settleTimeoutMs: number;
  /** The waits before each repeat of a failed request. */
  backoffMs: readonly number[];
  /** How long the page has to become ready, and how long its own calls then get to answer. */
  pageWaitMs?: number;
  pageSettleMs?: number;
  /** How long one pass over every screen may keep opening screens. */
  screensBudgetMs?: number;
  /** A production attempt: how long production has to serve the attempt's build, and how often it is asked. */
  productionWaitMs?: number;
  productionPollMs?: number;
  /** The instant every page of the run starts its `Date` at; {@link RUN_CLOCK} unless a test names one. */
  clockAt?: number;
  sleep: (ms: number) => Promise<void>;
  env: NodeJS.ProcessEnv;
}

export const DEFAULT_DEPS: BuildChecksDeps = {
  settlePollMs: 2_000,
  settleTimeoutMs: 6 * 60_000,
  backoffMs: DEFAULT_BACKOFF_MS,
  sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
  env: process.env
};

/** One pass over every screen stops opening screens after this long. */
export const SCREENS_BUDGET_MS = 150_000;

/**
 * The page of the claim's app in `environment`, held to `expectedBuild`.
 * The URL is the claim's for every environment; what differs is the header
 * that names the environment (`loads-target.ts`).
 */
export function claimTarget(
  claim: Claim,
  creds: Creds,
  deps: BuildChecksDeps,
  environment: string,
  expectedBuild: string
): LoadsTarget {
  if (!claim.url) throw new Error("the platform named no page to open for this build");
  return loadsTarget({
    pageUrl: claim.url,
    deployment: creds.target,
    credential: credentialHeaders(creds),
    appId: claim.app.id,
    workspaceId: claim.app.workspace_id ?? undefined,
    environment,
    expectedBuild,
    waitMs: deps.pageWaitMs,
    clockAt: deps.clockAt ?? RUN_CLOCK
  });
}
