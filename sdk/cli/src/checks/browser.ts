/**
 * The browser check `oxyc checks run --browser` adds to an app's checks: load
 * the app in headless Chromium and prove it mounted — in a NAMED environment.
 *
 * Same proof as the release checks' `web-app/tests/custom-app-checks/app-loads.spec.ts`:
 * a custom-app host answers 200 with the app shell for every path, so a status
 * code proves nothing; the platform runtime's `oxy-app-ready` beacon (sent once
 * the app has laid out) does. What this adds is the environment.
 *
 * HOW THE ENVIRONMENT REACHES THE APP'S REQUESTS. The page loads on the path
 * URL (`{target}/customer-apps/{org}/{app}/`), and a context-wide route handler
 * adds the caller's credential and `X-Oxy-App-Env` to every request under that
 * base, on that origin: the document, its assets, the beacon and the app's own
 * `/fn` calls. The server resolves the environment of each from that header
 * (`custom_app_env_request::request_environment`; the path URL has no host
 * label to outrank it), and honours it only beside an explicit credential,
 * which is why the credential rides along. NOT `extraHTTPHeaders`: that would
 * hand the credential to every origin the page touches, a font CDN included.
 * NOT the sandbox host: it authenticates by the session cookie, which a staff
 * token cannot become, and a sandbox whose name is too long has no host at
 * all. Service workers are blocked so no request escapes the route handler.
 *
 * NOTHING OUTSIDE THE APP'S BASE GETS THE CREDENTIAL, same origin or not. The
 * page runs on the deployment's own origin, beside the whole API, and its code
 * is the build under test — often one an agent or an app operator published.
 * A credential on every same-origin request would let that code call
 * `/api/…` (a promote, a token mint) as the person running the check.
 * So the app's `/api/…` calls go without it, as an anonymous visitor's would:
 * an app that will not mount without its data fails the check rather than
 * borrowing the operator's authority.
 *
 * THE CHECK VERIFIES ITSELF. The served document carries the environment and
 * the build in `window.__OXY_APP__`; a page reporting any other environment —
 * or, for a sandbox or staging, any build but the one that environment serves
 * — fails, so a header lost on the way can never pass against production.
 */

import { functionRoute } from "../commands/proxy.js";
import { launchChromium, loadChromium, type PwChromium, type PwPage } from "./playwright.js";

export interface BrowserTarget {
  /** The page to load: the app's base on the path URL. */
  url: string;
  /** The one origin that receives `headers`: the deployment's, which serves the path URL. */
  origin: string;
  /** The app's base path on that origin (`/customer-apps/{org}/{app}/`): only requests under it receive `headers`. */
  basePath: string;
  /** The credential, and `X-Oxy-App-Env` outside production. */
  headers: Record<string, string>;
  /** The environment the page must report; `production` when none was named. */
  environment: string;
  /** The build that environment serves (`Environment.build_uuid`), when known. */
  expectedBuild?: string;
  expectSelector?: string;
  /** Each wait: the ready beacon, the selector, the navigation. */
  waitMs: number;
}

/** A `/fn` call the page made, as its response arrived. */
export interface FunctionCall {
  function: string;
  status: number;
  /** `x-oxy-invocation-id`: the row it wrote. Absent for a result-cache hit. */
  invocationId?: string;
}

export interface BrowserCheckResult {
  name: "browser";
  kind: "browser";
  url: string;
  status: "done" | "failed" | "timed_out";
  passed: boolean;
  error?: string;
  durationMs: number;
  /** What the served document reported (`window.__OXY_APP__`). */
  environment?: string;
  buildId?: string;
  functionCalls: FunctionCall[];
}

/** What one page load showed, before it is scored. */
export interface Observed {
  /** Why the document itself failed: a navigation error, an error status, a redirect off the app. */
  navigationError?: string;
  ready: boolean;
  selectorError?: string;
  pageErrors: string[];
  app?: { environment?: string; buildId?: string };
  functionCalls: FunctionCall[];
  /**
   * The page's same-origin requests outside the app's base that were refused
   * (401/403) — sent without the credential on purpose, so a failure they
   * cause can say so ({@link deniedCallOf}). Optional: absent is none.
   */
  denied?: string[];
}

/**
 * The headers a request gets: `target.headers` under the app's base path on
 * the app's origin, nothing elsewhere — not the rest of the API on that origin,
 * nor another host. The path is read after URL parsing, so a `..` segment that
 * climbs out of the base is judged where it lands.
 */
export function headersFor(
  requestUrl: string,
  target: BrowserTarget
): Record<string, string> | undefined {
  let url: URL;
  try {
    url = new URL(requestUrl);
  } catch {
    return undefined;
  }
  if (url.origin !== target.origin) return undefined;
  return url.pathname.startsWith(target.basePath) ? target.headers : undefined;
}

/**
 * `"<path> answered <status>"` for a same-origin response outside the app's
 * base that refused the page (401/403): a request the check sent without the
 * credential on purpose. `undefined` for everything else.
 */
export function deniedCallOf(
  responseUrl: string,
  status: number,
  target: Pick<BrowserTarget, "origin" | "basePath">
): string | undefined {
  if (status !== 401 && status !== 403) return undefined;
  let url: URL;
  try {
    url = new URL(responseUrl);
  } catch {
    return undefined;
  }
  if (url.origin !== target.origin || url.pathname.startsWith(target.basePath)) return undefined;
  return `${url.pathname} answered ${status}`;
}

/** The function a response answered, when it is this deployment's `/fn` call. */
export function functionCallOf(
  responseUrl: string,
  status: number,
  headers: Record<string, string>,
  target: BrowserTarget
): FunctionCall | undefined {
  let url: URL;
  try {
    url = new URL(responseUrl);
  } catch {
    return undefined;
  }
  if (url.origin !== target.origin) return undefined;
  const name = functionRoute(url.pathname);
  if (!name) return undefined;
  const invocationId = headers["x-oxy-invocation-id"];
  return { function: name, status, ...(invocationId ? { invocationId } : {}) };
}

/** The verdict, from what the page showed. The first failure found is the one named. */
export function scoreBrowserCheck(
  target: BrowserTarget,
  observed: Observed,
  durationMs: number
): BrowserCheckResult {
  const base = {
    name: "browser" as const,
    kind: "browser" as const,
    url: target.url,
    durationMs,
    environment: observed.app?.environment,
    buildId: observed.app?.buildId,
    functionCalls: observed.functionCalls
  };
  // A failure the withheld credential may have caused names it, rather than
  // leaving the app to take the blame for a data call the check refused to sign.
  // Each distinct call once: a query the app retries or polls is one cause.
  const denied = [...new Set(observed.denied ?? [])];
  const cause =
    denied.length > 0
      ? `; the page's ${denied.slice(0, 3).join(", ")}${denied.length > 3 ? ` (+${denied.length - 3} more)` : ""} — the browser check sends no credential outside the app's base path (${target.basePath}), so the app's own /api calls run as an anonymous visitor's`
      : "";
  const fail = (status: "failed" | "timed_out", error: string): BrowserCheckResult => ({
    ...base,
    status,
    passed: false,
    error: error + cause
  });
  if (observed.navigationError) return fail("failed", observed.navigationError);
  const served = observed.app?.environment;
  if (served !== undefined && served !== target.environment) {
    return fail(
      "failed",
      `the page was served by ${served}, not ${target.environment}: the environment header did not reach the document`
    );
  }
  const build = observed.app?.buildId;
  if (target.expectedBuild && build !== target.expectedBuild) {
    return fail(
      "failed",
      `the page was served build ${build || "(none)"}, but ${target.environment} serves ${target.expectedBuild}`
    );
  }
  if (!observed.ready) {
    return fail(
      "timed_out",
      `the app never sent oxy-app-ready within ${target.waitMs / 1000}s — a blank page, a crash on boot, or a sign-in page`
    );
  }
  if (observed.selectorError) return fail("failed", observed.selectorError);
  if (observed.pageErrors.length > 0) {
    return fail("failed", `uncaught page error: ${observed.pageErrors[0]}`);
  }
  if (!observed.app) return fail("failed", "the page carries no window.__OXY_APP__");
  return { ...base, status: "done", passed: true };
}

/** Run the check: launch, load, observe, score. The browser is always closed. */
export async function runBrowserCheck(
  target: BrowserTarget,
  load: () => Promise<PwChromium> = () => loadChromium()
): Promise<BrowserCheckResult> {
  const browser = await launchChromium(await load());
  const started = Date.now();
  try {
    const context = await browser.newContext({ serviceWorkers: "block" });
    await context.route("**/*", (route) => {
      const request = route.request();
      const extra = headersFor(request.url(), target);
      return extra
        ? route.continue({ headers: { ...request.headers(), ...extra } })
        : route.continue();
    });
    const observed = await observe(await context.newPage(), target);
    return scoreBrowserCheck(target, observed, Date.now() - started);
  } finally {
    await browser.close();
  }
}

/** Load the page and record what it did; nothing here decides pass or fail. */
async function observe(page: PwPage, target: BrowserTarget): Promise<Observed> {
  const denied: string[] = [];
  const observed: Observed = { ready: false, pageErrors: [], functionCalls: [], denied };
  page.on("pageerror", (e) => observed.pageErrors.push(e.message));
  page.on("response", (r) => {
    const call = functionCallOf(r.url(), r.status(), r.headers(), target);
    if (call) observed.functionCalls.push(call);
    const refused = deniedCallOf(r.url(), r.status(), target);
    if (refused) denied.push(refused);
  });
  // `markReady` flushes with fetch, not sendBeacon, so the batch is readable
  // as postData; it goes to `<app base>__oxy/beacon`.
  const ready = page
    .waitForRequest(
      (r) => r.url().includes("/__oxy/beacon") && (r.postData() ?? "").includes("oxy-app-ready"),
      { timeout: target.waitMs }
    )
    .then(
      () => true,
      () => false
    );
  observed.navigationError = await navigate(page, target);
  // `ready` cannot reject (both arms are handled), so leaving it unawaited is safe.
  if (observed.navigationError) return observed;
  observed.ready = await ready;
  if (observed.ready && target.expectSelector) {
    observed.selectorError = await page
      .locator(target.expectSelector)
      .first()
      .waitFor({ state: "visible", timeout: target.waitMs })
      .then(
        () => undefined,
        () => `${target.expectSelector} was not visible within ${target.waitMs / 1000}s`
      );
  }
  // The calls the app makes as it mounts: let them answer, bounded.
  await page.waitForLoadState("networkidle", { timeout: 5_000 }).catch(() => undefined);
  observed.app = await page
    .evaluate(() => {
      const app = (globalThis as { __OXY_APP__?: { environment?: string; buildId?: string } })
        .__OXY_APP__;
      return app ? { environment: app.environment, buildId: app.buildId } : undefined;
    })
    .catch(() => undefined);
  return observed;
}

/** `undefined` when the document loaded as the app; otherwise why not. */
async function navigate(page: PwPage, target: BrowserTarget): Promise<string | undefined> {
  let response: Awaited<ReturnType<PwPage["goto"]>>;
  try {
    response = await page.goto(target.url, { timeout: target.waitMs });
  } catch (cause) {
    return `could not load ${target.url}: ${((cause as Error).message ?? "").split("\n")[0]}`;
  }
  const status = response?.status() ?? 0;
  if (status >= 400) return `the page answered ${status}`;
  // The login redirect leaves the app's base; an app's own client-side route does not.
  const landed = new URL(page.url());
  if (!landed.pathname.startsWith(new URL(target.url).pathname)) {
    return `redirected to ${landed.pathname} — the credential was not accepted (a sign-in page)`;
  }
  return undefined;
}
