/**
 * Every screen of a build, opened at desk and phone width: what `smoke`,
 * `changes` and `production_smoke` look at.
 *
 * Each screen goes through the SAME guarded path `loads` uses
 * (`loads-browser.ts`): the browser is the check's one sandboxed browser, each
 * viewport has one fresh context, and every request of every page in it is
 * judged by `guarded-route.ts`. A screen is opened by its URL. Nothing is
 * clicked, and no request of a page is sent that `loads` would not send.
 *
 * The two viewports are opened side by side, each walking the screens in
 * order, one page at a time. A pass stops opening screens when its time is
 * up and says how many it did not reach.
 *
 * A PICTURE AND A PAGE'S TEXT ARE A CUSTOMER'S DATA. They are taken only when
 * the check that compares them asked ({@link PassOptions.capture}), held in
 * memory until they are uploaded to the platform, and never written to disk
 * or printed.
 */

import { type GuardedContext, guardedContext } from "./loads-browser.js";
import type { Observed } from "./loads-score.js";
import { type LoadsTarget, VIEWPORTS, type Viewport } from "./loads-target.js";
import { fontsLoaded, MAX_LINKS, readLinks, readVisibleText } from "./page-reads.js";
import type { PwBrowser, PwPage } from "./playwright.js";
import {
  declaredScreens,
  discoveredScreens,
  type ScreenList,
  screenUrl,
  type Unlisted,
  unlistedScreens
} from "./screens.js";

/** A picture is the full page, cut at this height. */
export const MAX_SHOT_HEIGHT = 4_000;
/**
 * And at this width: twice the desk's. A page is as wide as its viewport
 * unless it overflows it, or is a phone page with no viewport of its own.
 */
export const MAX_SHOT_WIDTH = 2_880;
/** The most of a page's visible text kept. */
export const MAX_TEXT_CHARS = 4_000;
/** How long a page's calls get to answer once it is ready, before the next screen. */
export const SCREEN_SETTLE_MS = 1_500;
/** How long a page gets to go quiet and load its fonts before its picture. */
const STABLE_WAIT_MS = 5_000;
const SHOT_TIMEOUT_MS = 15_000;

/** One screen at one viewport, as it was opened. */
export interface ScreenVisit {
  path: string;
  viewport: Viewport;
  observed: Observed;
  /** The picture (PNG), when one was asked for and could be taken. Dropped once it is uploaded. */
  shot?: Uint8Array;
  /** The page's visible text, when a picture was asked for. */
  text?: string;
}

export interface PassOptions {
  /** How long a page's calls get to answer once it is ready. */
  settleMs: number;
  /** Take each screen's picture and text once it is stable. */
  capture: boolean;
  /** When the pass stops opening screens (ms since the epoch). */
  until: number;
  /** Called with each screen as soon as it has been opened, before the next. */
  onVisit?: (visit: ScreenVisit) => Promise<void>;
}

/** What one pass over the screens left. */
export interface ScreensPass {
  list: ScreenList;
  visits: ScreenVisit[];
  /** Screens, counted once per viewport, that were not opened before the time was up. */
  unvisited: number;
  /** Screens the opened ones link to (read at desk width) that the list does not hold. */
  unlisted: Unlisted;
}

/** One viewport's side of a pass: its guarded context over the target. */
interface Side {
  context: GuardedContext;
  target: LoadsTarget;
  viewport: Viewport;
}

/**
 * Open every screen of `target` at both viewports. The screens are `declared`
 * (the claim's `screens`) when the build declares them; otherwise the root is
 * opened first, at desk width, and its links are the list. Every screen's
 * links are read at desk width, so the pass can say which screens of the app
 * it saw linked and did not open.
 */
export async function openScreens(
  browser: PwBrowser,
  target: LoadsTarget,
  declared: unknown,
  cap: number,
  opts: PassOptions
): Promise<ScreensPass> {
  const [desk, phone] = VIEWPORTS;
  const contexts: GuardedContext[] = [];
  try {
    const deskContext = await guardedContext(browser, target, desk);
    contexts.push(deskContext);
    const phoneContext = await guardedContext(browser, target, phone);
    contexts.push(phoneContext);
    const deskSide: Side = { context: deskContext, target, viewport: desk };
    const phoneSide: Side = { context: phoneContext, target, viewport: phone };

    const first: ScreenVisit[] = [];
    const links: string[] = [];
    let list = declaredScreens(declared, target, cap);
    if (!list) {
      const root = await openScreen(deskSide, "", opts, true);
      first.push(root.visit);
      links.push(...root.links);
      await opts.onVisit?.(root.visit);
      list = discoveredScreens(root.links, target, cap);
    }
    const [atDesk, atPhone] = await Promise.all([
      // A discovered list starts with the root, which is already open at desk width.
      openEach(deskSide, list.paths.slice(first.length), opts, true),
      openEach(phoneSide, list.paths, opts, false)
    ]);
    links.push(...atDesk.links);
    // A screen the build declared past the cap was left out, not left undeclared: it is
    // counted in `list.omitted`, and telling its author to declare it would be wrong.
    const declaredAll = declaredScreens(declared, target, Number.POSITIVE_INFINITY)?.paths ?? [];
    return {
      list,
      visits: [...first, ...atDesk.visits, ...atPhone.visits],
      unvisited: atDesk.unvisited + atPhone.unvisited,
      unlisted: unlistedScreens(links, target, [...list.paths, ...declaredAll])
    };
  } finally {
    await Promise.all(contexts.map((context) => context.close()));
  }
}

/** Open `paths` in order on one side, until the time is up; with `links`, read each one's. */
async function openEach(
  side: Side,
  paths: readonly string[],
  opts: PassOptions,
  links: boolean
): Promise<{ visits: ScreenVisit[]; unvisited: number; links: string[] }> {
  const visits: ScreenVisit[] = [];
  const found: string[] = [];
  for (const [index, path] of paths.entries()) {
    if (Date.now() >= opts.until) {
      return { visits, unvisited: paths.length - index, links: found };
    }
    const opened = await openScreen(side, path, opts, links);
    visits.push(opened.visit);
    found.push(...opened.links);
    await opts.onVisit?.(opened.visit);
  }
  return { visits, unvisited: 0, links: found };
}

/** Open one screen, read what was asked of it, and close its page. */
async function openScreen(
  side: Side,
  path: string,
  opts: PassOptions,
  links: boolean
): Promise<{ visit: ScreenVisit; links: string[] }> {
  const { context, target, viewport } = side;
  const { observed, page } = await context.open(screenUrl(target, path), opts.settleMs);
  const visit: ScreenVisit = { path, viewport, observed };
  let found: string[] = [];
  try {
    if (page && links) found = await page.evaluate(readLinks, MAX_LINKS).catch(() => []);
    if (page && opts.capture) await capture(page, visit);
  } finally {
    await page?.close().catch(() => undefined);
  }
  return { visit, links: found };
}

/**
 * The page's picture and text, once it is stable: its requests have gone
 * quiet, its fonts have loaded, and its animations are off. A page that
 * cannot be pictured leaves none, and the comparison says so.
 */
async function capture(page: PwPage, visit: ScreenVisit): Promise<void> {
  await page.waitForLoadState("networkidle", { timeout: STABLE_WAIT_MS }).catch(() => undefined);
  await within(page.evaluate(fontsLoaded), STABLE_WAIT_MS);
  try {
    visit.shot = await page.screenshot({
      type: "png",
      fullPage: true,
      // A clip that reaches past the page is cut to the page: this only ever makes it smaller.
      clip: { x: 0, y: 0, width: MAX_SHOT_WIDTH, height: MAX_SHOT_HEIGHT },
      animations: "disabled",
      caret: "hide",
      scale: "css",
      timeout: SHOT_TIMEOUT_MS
    });
    visit.text = await page.evaluate(readVisibleText, MAX_TEXT_CHARS);
  } catch {
    // No picture: the row that compares this screen records that there is none.
  }
}

/** Wait for `promise`, whatever it settles to, for at most `ms`. */
async function within(promise: Promise<unknown>, ms: number): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<void>((resolve) => {
    timer = setTimeout(resolve, ms);
  });
  try {
    await Promise.race([promise.then(() => undefined).catch(() => undefined), late]);
  } finally {
    clearTimeout(timer);
  }
}
