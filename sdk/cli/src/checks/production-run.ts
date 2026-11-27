/**
 * `production_smoke`: after a promote, does production open?
 *
 * A production attempt is about ONE build, the one the move pointed
 * production at. So the check first waits until production's page reports
 * that build (`window.__OXY_APP__.buildId`, re-read in a fresh browser
 * context each time, every signed read asking for an uncached copy): a page
 * of the build before, still cached somewhere, is never what gets judged.
 * Three minutes without it is `error`: the check has no verdict on a build
 * production does not serve.
 *
 * Then every screen is opened at desk and phone width, exactly as `smoke`
 * opens them on staging, and what each shows is compared with the claim's
 * baseline (`smoke-score.ts`).
 *
 * THE SAME READER, ON PRODUCTION. No environment header is sent (production
 * is what the URL serves unnamed); the requests signed are the ones staging's
 * check signs and no others; every write is refused; and no function is
 * called. Nothing a page showed is kept: no picture is taken here.
 */

import type { Creds } from "../apps/resolve.js";
import { guardedContext, withGuardedBrowser } from "./loads-browser.js";
import { type LoadsTarget, VIEWPORTS } from "./loads-target.js";
import { errorOutcome, type Outcome } from "./outcome.js";
import type { PwBrowser } from "./playwright.js";
import { baselineOf, type Claim, limitsOf } from "./promote-api.js";
import { type BuildChecksDeps, claimTarget, SCREENS_BUDGET_MS } from "./run-deps.js";
import { openScreens, SCREEN_SETTLE_MS } from "./screens-browser.js";
import { scoreProductionSmoke } from "./smoke-score.js";

/** How long production has to serve the attempt's build. */
export const PRODUCTION_WAIT_MS = 3 * 60_000;
/** How long between two reads of what production serves. */
const PRODUCTION_POLL_MS = 5_000;
/** How long one of those reads waits for a page that is not coming up. */
const POLL_READY_MS = 10_000;
/** The browser's own start and stop, beside the wait and the pass. */
const SESSION_MARGIN_MS = 60_000;

export interface ProductionSmokeInput {
  claim: Claim;
  creds: Creds;
  deps: BuildChecksDeps;
}

export async function runProductionSmoke(input: ProductionSmokeInput): Promise<Outcome> {
  const { claim, creds, deps } = input;
  if (!claim.url) return errorOutcome("the platform named no page to open for this build");
  const target = claimTarget(claim, creds, deps, "production", claim.build.id);
  const baseline = baselineOf(claim);
  const waitMs = deps.productionWaitMs ?? PRODUCTION_WAIT_MS;
  const budget = deps.screensBudgetMs ?? SCREENS_BUDGET_MS;
  return withGuardedBrowser(
    target.origin,
    { load: deps.loadChromium, env: deps.env, deadlineMs: waitMs + budget + SESSION_MARGIN_MS },
    async (browser) => {
      const served = await waitForBuild(browser, target, deps, waitMs);
      if (served !== true) {
        // A person knows the build by the id it was published under; a page reports its row id.
        const instead =
          served === null ? "" : ` (its page reports build ${served}, not ${target.expectedBuild})`;
        return errorOutcome(
          `production did not serve build ${claim.build.build_id} within ${Math.floor(waitMs / 1000)}s${instead}`,
          {
            baseline_at: baseline?.at ?? null,
            served_build_id: served,
            screens: [],
            new_errors: []
          }
        );
      }
      const pass = await openScreens(browser, target, claim.screens, limitsOf(claim).screens, {
        settleMs: deps.pageSettleMs ?? SCREEN_SETTLE_MS,
        capture: false,
        until: Date.now() + budget
      });
      return scoreProductionSmoke(target, pass, baseline);
    }
  );
}

/**
 * Read which build production's page reports until it is the attempt's.
 * `true` when it is; otherwise the build it last reported (`null`: none).
 */
async function waitForBuild(
  browser: PwBrowser,
  target: LoadsTarget,
  deps: BuildChecksDeps,
  waitMs: number
): Promise<true | string | null> {
  const deadline = Date.now() + waitMs;
  const poll: LoadsTarget = { ...target, waitMs: Math.min(target.waitMs, POLL_READY_MS) };
  for (;;) {
    let served: string | null = null;
    // A fresh context each time: nothing the read before fetched is cached in it.
    const context = await guardedContext(browser, poll, VIEWPORTS[0]);
    try {
      const { observed, page } = await context.open(poll.url, 0);
      await page?.close().catch(() => undefined);
      served = observed.app?.buildId ?? null;
    } finally {
      await context.close();
    }
    if (served === target.expectedBuild) return true;
    if (Date.now() >= deadline) return served;
    await deps.sleep(deps.productionPollMs ?? PRODUCTION_POLL_MS);
  }
}
