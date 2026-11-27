/**
 * The promote checks' browser: headless Chromium opens a build's page and
 * records what it shows. Nothing here decides pass or fail (`loads-score.ts`),
 * and nothing is read from the page here but the two fields of
 * `window.__OXY_APP__`.
 *
 * `loads` opens the app's root at each viewport ({@link observeAll}). The
 * checks that open every screen (`screens-browser.ts`) go through the same
 * two pieces, so one set of rules holds for every page a check opens:
 *
 * - {@link withGuardedBrowser}: ONE browser per check, started with its
 *   sandbox on, an environment that holds no credential and no way to find a
 *   host but the app's own, bound by a deadline, and always closed;
 * - {@link guardedContext}: one fresh context per viewport, every request of
 *   it judged by `guarded-route.ts`.
 *
 * A page is navigated, listened to, and asked one expression once it is up.
 * Nothing is clicked.
 *
 * - HEADERS: a context-wide route handler judges every request
 *   (`loads-target.ts` `treat`): signed, passed as it was, or refused and
 *   never sent. Not `extraHTTPHeaders`, which would hand the credential to
 *   every origin the page touches.
 * - READY: the request to `<base>__oxy/beacon` whose body carries
 *   `oxy-app-ready`, seen in that same handler.
 * - ERRORS: uncaught page errors, responses at 400 or above, and requests
 *   that failed for a reason other than the page cancelling them or the check
 *   refusing them.
 * - SERVICE WORKERS are blocked, so no request escapes the route handler.
 * - SOCKETS: every WebSocket a page constructs is ended before anything is
 *   sent, and recorded like a refused request (`browser-confine.ts`).
 *
 * THE PAGE IS A TENANT'S CODE, AND THE CREDENTIAL CAN READ EVERY ORG'S
 * STAGING. Four things keep the two apart:
 *
 * - THE SANDBOX IS ON. Playwright starts Chromium without its sandbox unless
 *   told; this tells it, and a browser that cannot have one is an error, never
 *   a browser started without it.
 * - THE BROWSER'S ENVIRONMENT HOLDS NO CREDENTIAL (`browser-env.ts`): the
 *   variable the checker's token is in is not handed to the process that runs
 *   the page.
 * - A REDIRECT NEVER CARRIES THE CREDENTIAL, and its next hop is judged
 *   before the browser is handed it (`guarded-route.ts`).
 * - THE PAGE REACHES THE APP'S ORIGIN AND NOTHING ELSE: every request to
 *   another origin is refused (`loads-target.ts`), and the browser cannot find
 *   another host by a path the check is not shown (`browser-confine.ts`).
 */

import { deniedCallOf } from "./browser.js";
import { confine, confinementArgs, Unconfined } from "./browser-confine.js";
import { browserEnv } from "./browser-env.js";
import { answerRoute, recordRefusal } from "./guarded-route.js";
import { newObserved, type Observed, pushFailed, pushPageError } from "./loads-score.js";
import { type LoadsTarget, treatSocket, type Viewport } from "./loads-target.js";
import { startDateAt } from "./page-clock.js";
import {
  launchChromium,
  loadChromium,
  type PwBrowser,
  type PwChromium,
  type PwContext,
  type PwPage
} from "./playwright.js";

/** How long the calls the app makes as it mounts get to answer, once ready. */
const SETTLE_MS = 3_000;
/** The whole check's bound: both viewports' waits, the settle and the launch. */
const CHECK_DEADLINE_MS = 150_000;

export interface LoadsBrowserOptions {
  /** Where Chromium comes from; the tests hand in a stand-in. */
  load?: () => Promise<PwChromium>;
  settleMs?: number;
  deadlineMs?: number;
  /** The environment the browser's is picked from; this process's when absent. */
  env?: NodeJS.ProcessEnv;
}

/** One viewport and what its load showed. */
export type Load = readonly [Viewport, Observed];

/**
 * Launch Chromium, sandboxed, with no credential in its environment and able
 * to reach `origin` and no other host, and run `work` with it. The browser is
 * always closed, and work that outlasts the deadline is an error.
 */
export async function withGuardedBrowser<T>(
  origin: string,
  opts: LoadsBrowserOptions,
  work: (browser: PwBrowser) => Promise<T>
): Promise<T> {
  const deadlineMs = opts.deadlineMs ?? CHECK_DEADLINE_MS;
  const browser = await launchChromium(await (opts.load ?? loadChromium)(), {
    sandbox: true,
    args: confinementArgs(origin),
    env: browserEnv(opts.env ?? process.env)
  });
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () =>
        reject(
          new Error(`the browser check did not finish within ${Math.floor(deadlineMs / 1000)}s`)
        ),
      deadlineMs
    );
  });
  const run = work(browser);
  // A page stuck in a loop never answers `evaluate`; closing the browser below ends it.
  run.catch(() => undefined);
  try {
    return await Promise.race([run, late]);
  } finally {
    clearTimeout(timer);
    await browser.close().catch(() => undefined);
  }
}

/** Launch Chromium and load `target` at each viewport, in order. The browser is always closed. */
export async function observeAll(
  target: LoadsTarget,
  viewports: readonly Viewport[],
  opts: LoadsBrowserOptions = {}
): Promise<Load[]> {
  return withGuardedBrowser(target.origin, opts, async (browser) => {
    const seen: Load[] = [];
    for (const viewport of viewports) {
      const observed = await observeViewport(browser, target, viewport, opts.settleMs ?? SETTLE_MS);
      seen.push([viewport, observed]);
    }
    return seen;
  });
}

/** One page a check opened: what it showed, and the page while it is still open. */
export interface Visit {
  observed: Observed;
  /** The page, for a reader that needs more of it; absent when it could not be created. */
  page?: PwPage;
}

/** One viewport's browser context: every request of every page in it is judged by the check. */
export interface GuardedContext {
  /**
   * Open `url` in a new page and record what it shows. Never throws: a page
   * that cannot be opened is the visit's navigation error. One page at a time;
   * close the one before opening the next.
   */
  open(url: string, settleMs: number): Promise<Visit>;
  close(): Promise<void>;
}

/** What the route handler records into: the page being opened now. */
interface Current {
  observed: Observed;
  onReady: () => void;
}

/** A fresh context at `viewport`, with the check's route handler on every request. */
export async function guardedContext(
  browser: PwBrowser,
  target: LoadsTarget,
  viewport: Viewport
): Promise<GuardedContext> {
  const context = await browser.newContext({
    serviceWorkers: "block",
    viewport: { width: viewport.width, height: viewport.height },
    deviceScaleFactor: viewport.mobile ? 3 : 1,
    isMobile: viewport.mobile
  });
  const state: { current?: Current } = {};
  const onSocket = (url: string) => {
    if (state.current) recordRefusal(state.current.observed, treatSocket(target, url));
  };
  // A context that could not be confined opens no page.
  await confine(context, onSocket).catch(async (cause: unknown) => {
    await context.close().catch(() => undefined);
    throw cause;
  });
  // Every page of a run starts its clock at one instant, so a screen that draws
  // the time draws the same one on staging and on production.
  if (target.clockAt !== undefined) await context.addInitScript(startDateAt, target.clockAt);
  // A handler that throws would leave its request hanging: one whose page has
  // gone (the context closed under it) has nothing left to answer.
  await context.route("**/*", (route) => {
    const current = state.current;
    // Before the first page is opened nothing is owed an answer, and nothing is sent.
    if (!current) return route.abort("blockedbyclient").catch(() => undefined);
    return answerRoute(route, target, current.observed, current.onReady).catch(() => undefined);
  });
  return {
    open: async (url, settleMs) => {
      const observed = newObserved();
      const visit: Visit = { observed };
      try {
        await load({ context, target, url, visit, settleMs, state });
      } catch (cause) {
        observed.navigationError ??= `the page could not be opened: ${firstLine(cause)}`;
      }
      return visit;
    },
    close: () => context.close().catch(() => undefined)
  };
}

/**
 * One viewport's load. A failure to set the page up is that viewport's
 * navigation error; a context that cannot be confined is not: it is thrown, so
 * the check is an error and never a verdict on the build.
 */
async function observeViewport(
  browser: PwBrowser,
  target: LoadsTarget,
  viewport: Viewport,
  settleMs: number
): Promise<Observed> {
  let context: GuardedContext | undefined;
  try {
    context = await guardedContext(browser, target, viewport);
    return (await context.open(target.url, settleMs)).observed;
  } catch (cause) {
    if (cause instanceof Unconfined) throw cause;
    const observed = newObserved();
    observed.navigationError = `the page could not be opened: ${firstLine(cause)}`;
    return observed;
  } finally {
    await context?.close();
  }
}

/** One page load in a guarded context. */
interface PageLoad {
  context: PwContext;
  target: LoadsTarget;
  url: string;
  /** Filled as the page loads: what it showed, and the page itself. */
  visit: Visit;
  settleMs: number;
  /** Where the context's route handler looks up the page being opened now. */
  state: { current?: Current };
}

async function load({ context, target, url, visit, settleMs, state }: PageLoad): Promise<void> {
  const observed = visit.observed;
  let started = Date.now();
  let sawReady: () => void = () => undefined;
  const ready = new Promise<void>((resolve) => {
    sawReady = resolve;
  });
  const onReady = () => {
    if (observed.readyMs !== null) return;
    observed.readyMs = Date.now() - started;
    sawReady();
  };
  state.current = { observed, onReady };
  const page = await context.newPage();
  visit.page = page;
  listen(page, target, observed);

  started = Date.now();
  observed.navigationError = await navigate(page, target, url, observed);
  if (observed.navigationError) return;
  const left = Math.max(0, target.waitMs - (Date.now() - started));
  if (await within(ready, left)) await sleep(settleMs);
  observed.navigationError = landedOffTheApp(page, target);
  observed.app = await readApp(page);
}

/** Page errors, failed requests and refused calls, as they happen. */
function listen(page: PwPage, target: LoadsTarget, observed: Observed): void {
  page.on("pageerror", (error) => pushPageError(observed, pageErrorText(error)));
  page.on("response", (response) => {
    const status = response.status();
    if (status >= 400) pushFailed(observed, { url: response.url(), status, error: null });
    const denied = deniedCallOf(response.url(), status, target);
    if (denied) observed.denied.push(denied);
  });
  page.on("requestfailed", (request) => {
    const error = request.failure()?.errorText ?? "failed";
    // A cancel is the page's own; a block is a request the check refused,
    // recorded as such when it was refused.
    if (/ERR_ABORTED|ERR_BLOCKED_BY_CLIENT/.test(error)) return;
    pushFailed(observed, { url: request.url(), status: null, error });
  });
}

/** `undefined` when the document answered; otherwise why it did not. */
async function navigate(
  page: PwPage,
  target: LoadsTarget,
  url: string,
  observed: Observed
): Promise<string | undefined> {
  let response: Awaited<ReturnType<PwPage["goto"]>>;
  try {
    // `commit`: the document answered. Whether the app then mounts is the beacon's to say.
    response = await page.goto(url, { timeout: target.waitMs, waitUntil: "commit" });
  } catch (cause) {
    // Nothing of the page has run yet, so a request refused by now is the document's own
    // redirect: it was sent to another origin, which is where a sign-in lives.
    const away = observed.offOrigin[0];
    if (away !== undefined) {
      return `redirected off the app's origin — the checker's credential was not accepted (a sign-in page); not sent: ${away}`;
    }
    return `could not load ${url}: ${firstLine(cause)}`;
  }
  const status = response?.status() ?? 0;
  return status >= 400 ? `the page answered ${status}` : undefined;
}

/** A redirect that left the app's base is a sign-in page, not the app's own client-side route. */
function landedOffTheApp(page: PwPage, target: LoadsTarget): string | undefined {
  let landed: URL;
  try {
    landed = new URL(page.url());
  } catch {
    return undefined;
  }
  const here = landed.origin === target.origin;
  if (here && landed.pathname.startsWith(target.basePath)) return undefined;
  const where = here ? landed.pathname : `${landed.origin}${landed.pathname}`;
  return `redirected to ${where} — the checker's credential was not accepted (a sign-in page)`;
}

/** `window.__OXY_APP__`'s environment and build, as the served document set them. */
async function readApp(page: PwPage): Promise<Observed["app"]> {
  const app = await page
    .evaluate(() => {
      const injected = (
        globalThis as { __OXY_APP__?: { environment?: unknown; buildId?: unknown } }
      ).__OXY_APP__;
      if (!injected || typeof injected !== "object") return null;
      const text = (value: unknown) => (typeof value === "string" ? value : null);
      return { environment: text(injected.environment), buildId: text(injected.buildId) };
    })
    .catch(() => null);
  return app ?? undefined;
}

/** `TypeError: x is undefined`: the error's name and the first line of its message. */
function pageErrorText(error: Error): string {
  const message = firstLine(error);
  const name = error.name?.trim();
  return name && !message.startsWith(`${name}:`) ? `${name}: ${message}` : message;
}

function firstLine(cause: unknown): string {
  const text = cause instanceof Error ? cause.message : String(cause);
  return (text.split("\n")[0] ?? "").trim();
}

/** Whether `promise` settled within `ms`. */
async function within(promise: Promise<void>, ms: number): Promise<boolean> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<boolean>((resolve) => {
    timer = setTimeout(() => resolve(false), ms);
  });
  try {
    return await Promise.race([promise.then(() => true), late]);
  } finally {
    clearTimeout(timer);
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
