/**
 * The two staging checks that open every screen of a build, run as ONE
 * browser session:
 *
 *   smoke     every screen at desk and phone width renders and throws nothing
 *   changes   each of those screens pictured on staging and on production,
 *             compared, and the evidence kept on the platform
 *
 * One session because they look at the same pages: the staging pass that
 * `smoke` judges takes the pictures `changes` compares, so staging is opened
 * once. `smoke` gets its verdict as soon as that pass ends, before production
 * is opened at all, so the console fills in as each check finishes.
 *
 * `changes` is `skipped` when the claim names no production build (it serves
 * nothing, or this same build): then no picture is taken anywhere. The CSS
 * digest is kept either way, before the check gets its verdict: the
 * platform's design notes read it, and a closed attempt takes no file.
 *
 * NEVER THROWS. A check this cannot finish gets `error` with why.
 */

import type { Creds } from "../apps/resolve.js";
import {
  type ChangeRow,
  compareScreen,
  EvidenceFiles,
  pictureComparer,
  scoreChanges,
  skippedChanges
} from "./changes.js";
import { cssDigestFile } from "./css-digest.js";
import type { StyleSource } from "./design-system.js";
import { withGuardedBrowser } from "./loads-browser.js";
import { type LoadsTarget, VIEWPORTS } from "./loads-target.js";
import { errorOutcome, messageOf, type Outcome } from "./outcome.js";
import type { PwBrowser } from "./playwright.js";
import { type Claim, limitsOf, type PromoteApi, putFile } from "./promote-api.js";
import { type BuildChecksDeps, claimTarget, SCREENS_BUDGET_MS } from "./run-deps.js";
import { CSS_DIGEST_NAME, screenSlugs } from "./screens.js";
import { openScreens, SCREEN_SETTLE_MS, type ScreensPass } from "./screens-browser.js";
import { scoreSmoke } from "./smoke-score.js";

export type ScreenKind = "smoke" | "changes";

export interface ScreenChecksInput {
  api: PromoteApi;
  claim: Claim;
  creds: Creds;
  deps: BuildChecksDeps;
  /** Which of the two the attempt still has open. */
  wants: Record<ScreenKind, boolean>;
  /** The build's `index.html` and CSS, read at most once for the whole run. */
  stylesheets: () => Promise<StyleSource[]>;
  /** Hand a check its verdict, as soon as it is known. Must not throw. */
  land: (kind: ScreenKind, verdict: Outcome) => Promise<void>;
}

/** Compares, uploads and the browser's own start and stop, beside the two passes. */
const SESSION_MARGIN_MS = 90_000;

/** Run what the attempt has open of `smoke` and `changes`, landing each verdict once. */
export async function runScreenChecks(input: ScreenChecksInput): Promise<void> {
  const landed = new Set<ScreenKind>();
  const land = async (kind: ScreenKind, verdict: Outcome) => {
    if (!input.wants[kind] || landed.has(kind)) return;
    landed.add(kind);
    await input.land(kind, verdict);
  };
  try {
    await drive(input, land);
  } catch (cause) {
    const failed = errorOutcome(messageOf(cause));
    await land("smoke", failed);
    await land("changes", failed);
  }
}

async function drive(
  input: ScreenChecksInput,
  land: (kind: ScreenKind, verdict: Outcome) => Promise<void>
): Promise<void> {
  const { api, claim, creds, deps, wants } = input;
  if (!wants.smoke && !wants.changes) return;
  const limits = limitsOf(claim);
  const files = new EvidenceFiles(limits, (name, type, bytes) =>
    putFile(api, claim.attempt, name, type, bytes)
  );
  // The build production serves, when `changes` has one to compare with. Its page
  // reports the build's row id (`window.__OXY_APP__.buildId`), so that is the id
  // the page is held to; the detail names it as it was published.
  const live = wants.changes && claim.production?.id ? claim.production : null;
  if (wants.changes) {
    await keepDigest(input, files);
    if (!live) await land("changes", skippedChanges(files.warnings()));
  }
  if (!wants.smoke && !live) return;

  const staging = claimTarget(claim, creds, deps, claim.environment || "staging", claim.build.id);
  const budget = deps.screensBudgetMs ?? SCREENS_BUDGET_MS;
  const settleMs = deps.pageSettleMs ?? SCREEN_SETTLE_MS;
  await withGuardedBrowser(
    staging.origin,
    { load: deps.loadChromium, env: deps.env, deadlineMs: 2 * budget + SESSION_MARGIN_MS },
    async (browser) => {
      const pass = await openScreens(browser, staging, claim.screens, limits.screens, {
        settleMs,
        capture: live !== null,
        until: Date.now() + budget
      });
      await land("smoke", scoreSmoke(staging, pass));
      if (!live) return;
      const target = claimTarget(claim, creds, deps, "production", live.id);
      const verdict = await compareWithProduction(browser, pass, files, {
        target,
        name: live.build_id || live.id,
        cap: limits.screens,
        settleMs,
        budget
      });
      await land("changes", verdict);
    }
  );
}

/** Keep the CSS digest; a build whose CSS cannot be read has none, and the detail says so. */
async function keepDigest(input: ScreenChecksInput, files: EvidenceFiles): Promise<void> {
  try {
    const sources = await input.stylesheets();
    await files.keep(CSS_DIGEST_NAME, "application/json", cssDigestFile(sources));
  } catch (cause) {
    files.warn(
      `${CSS_DIGEST_NAME} was not made: the bundle's CSS could not be read (${messageOf(cause)})`
    );
  }
}

interface ProductionPass {
  target: LoadsTarget;
  /** Production's build as people name it: the id it was published under. */
  name: string;
  cap: number;
  settleMs: number;
  budget: number;
}

/** Open on production every screen staging showed, comparing each as soon as it is pictured. */
async function compareWithProduction(
  browser: PwBrowser,
  staging: ScreensPass,
  files: EvidenceFiles,
  production: ProductionPass
): Promise<Outcome> {
  const key = (path: string, viewport: string) => `${path}\n${viewport}`;
  const shown = new Map(staging.visits.map((v) => [key(v.path, v.viewport.name), v]));
  // Only what staging showed: a screen it never reached has nothing to be compared with.
  const paths = staging.list.paths.filter((path) =>
    VIEWPORTS.some((viewport) => shown.has(key(path, viewport.name)))
  );
  const slugs = screenSlugs(staging.list.paths);
  const rows: ChangeRow[] = [];
  let uncompared = staging.unvisited;
  const pictures = await pictureComparer(browser);
  try {
    const pass = await openScreens(browser, production.target, paths, production.cap, {
      settleMs: production.settleMs,
      capture: true,
      until: Date.now() + production.budget,
      onVisit: async (visit) => {
        const before = shown.get(key(visit.path, visit.viewport.name));
        if (!before) return;
        rows.push(
          await compareScreen({
            staging: before,
            production: visit,
            slug: slugs.get(visit.path) ?? "root",
            files,
            compare: pictures.compare
          })
        );
      }
    });
    uncompared += pass.unvisited;
  } finally {
    await pictures.close();
  }
  const order = VIEWPORTS.map((viewport) => viewport.name);
  rows.sort(
    (a, b) =>
      paths.indexOf(a.path) - paths.indexOf(b.path) ||
      order.indexOf(a.viewport as (typeof order)[number]) -
        order.indexOf(b.viewport as (typeof order)[number])
  );
  return scoreChanges(production.name, rows, files.warnings(), uncompared);
}
