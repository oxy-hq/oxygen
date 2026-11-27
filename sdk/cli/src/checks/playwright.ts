/**
 * Playwright, found on demand for `oxyc checks run --browser`.
 *
 * NOT a dependency of `@oxy-hq/cli`: a browser driver is tens of megabytes
 * before the browser itself, and every other verb would pay for it. The check
 * resolves whichever Playwright package the caller already has — an app's own
 * dev dependency, the repo's `web-app`, a global install — the way the
 * create-oxy-app templates' `screenshot.mjs` does, and says how to add one when
 * none resolves.
 *
 * The types below are the slice of Playwright's API the check calls, declared
 * structurally so this package compiles without it.
 */

import { createRequire } from "node:module";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

import { CliError, ExitCode } from "../util/errors.js";

export interface PwRequest {
  url(): string;
  method(): string;
  headers(): Record<string, string>;
  postData(): string | null;
  /** Why the request failed, once it has; `null` otherwise. */
  failure(): { errorText: string } | null;
}

export interface PwResponse {
  url(): string;
  status(): number;
  headers(): Record<string, string>;
}

/** An answer fetched outside the page (`route.fetch`), handed back to it whole (`route.fulfill`). */
export interface PwApiResponse {
  status(): number;
  headers(): Record<string, string>;
}

export interface PwRoute {
  request(): PwRequest;
  continue(opts?: { headers?: Record<string, string> }): Promise<void>;
  /** Fail the request in the browser; it is never sent. */
  abort(errorCode?: string): Promise<void>;
  /**
   * Send the request from outside the page with these headers and return its
   * answer. With `maxRedirects: 0` a redirect is returned, not followed.
   */
  fetch(opts: {
    headers: Record<string, string>;
    maxRedirects: number;
    timeout?: number;
  }): Promise<PwApiResponse>;
  /** Answer the request in the browser with `response`, status and headers and body. */
  fulfill(opts: { response: PwApiResponse }): Promise<void>;
}

/** One WebSocket a page asked for, before anything was sent for it. */
export interface PwWebSocketRoute {
  url(): string;
  /** End the page's socket. Nothing was connected, and nothing is. */
  close(opts?: { code?: number; reason?: string }): Promise<void>;
}

export interface PwLocator {
  first(): PwLocator;
  waitFor(opts: { state: "visible"; timeout: number }): Promise<void>;
}

export interface PwPage {
  on(event: "pageerror", fn: (error: Error) => void): void;
  on(event: "response", fn: (response: PwResponse) => void): void;
  on(event: "requestfailed", fn: (request: PwRequest) => void): void;
  waitForRequest(
    pred: (request: PwRequest) => boolean,
    opts: { timeout: number }
  ): Promise<PwRequest>;
  goto(
    url: string,
    opts: { timeout: number; waitUntil?: "commit" | "load" }
  ): Promise<PwResponse | null>;
  url(): string;
  locator(selector: string): PwLocator;
  waitForLoadState(state: "networkidle", opts: { timeout: number }): Promise<void>;
  /**
   * Run `fn` in the page, with `arg` when it takes one. The function is sent as
   * its source, so it may use nothing from outside itself.
   */
  evaluate<T>(fn: () => T | Promise<T>): Promise<T>;
  evaluate<T, A>(fn: (arg: A) => T | Promise<T>, arg: A): Promise<T>;
  /**
   * Evaluate an expression in the page. It is given no argument: an
   * expression that evaluates to a function is NOT called.
   */
  evaluate<T>(expression: string): Promise<T>;
  /** The page's picture, as PNG bytes. */
  screenshot(opts: PwScreenshotOptions): Promise<Uint8Array>;
  close(): Promise<void>;
}

/** The slice of Playwright's screenshot options the checks set. */
export interface PwScreenshotOptions {
  type: "png";
  /** The whole scrollable page, not only the viewport. */
  fullPage: boolean;
  /** The part of the page kept; one that reaches past the page is cut to it. */
  clip?: { x: number; y: number; width: number; height: number };
  /** Stop CSS animations and transitions, so two pictures of one page are the same picture. */
  animations: "disabled";
  caret: "hide";
  /** One picture pixel per CSS pixel, whatever the device's scale factor. */
  scale: "css";
  timeout?: number;
}

export interface PwContext {
  route(pattern: string, handler: (route: PwRoute) => Promise<void>): Promise<void>;
  /**
   * Run `fn` with `arg` in every frame of every page of the context, before
   * any of the page's own code. Sent as its source: it may use nothing from
   * outside itself.
   */
  addInitScript<A>(fn: (arg: A) => void, arg: A): Promise<void>;
  /**
   * Hand every `WebSocket` a page of the context constructs to `handler`
   * instead of connecting it. Playwright 1.48 and later.
   */
  routeWebSocket(
    pattern: RegExp,
    handler: (socket: PwWebSocketRoute) => void | Promise<void>
  ): Promise<void>;
  newPage(): Promise<PwPage>;
  close(): Promise<void>;
}

/** The slice of Playwright's context options the checks set. */
export interface PwContextOptions {
  serviceWorkers: "block";
  viewport?: { width: number; height: number };
  deviceScaleFactor?: number;
  isMobile?: boolean;
}

export interface PwBrowser {
  newContext(opts: PwContextOptions): Promise<PwContext>;
  close(): Promise<void>;
}

/** The slice of Playwright's launch options the checks set. */
export interface PwLaunchOptions {
  headless: boolean;
  /** Keep Chromium's own sandbox on. Playwright's default is off. */
  chromiumSandbox?: boolean;
  /** More of Chromium's own command-line flags. */
  args?: string[];
  /** The browser's whole environment, in place of this process's. */
  env?: Record<string, string>;
}

export interface PwChromium {
  launch(opts: PwLaunchOptions): Promise<PwBrowser>;
}

/** How a check wants Chromium started. */
export interface LaunchChoices {
  /**
   * Start Chromium with its sandbox on, and fail when it cannot have one. For
   * a page whose code is not the caller's own: there is no falling back to a
   * browser without it.
   */
  sandbox?: boolean;
  /** Chromium flags of the check's own (`browser-confine.ts`). */
  args?: string[];
  /** The browser's whole environment (`browser-env.ts`); this process's when absent. */
  env?: Record<string, string>;
}

/** Tried in this order from each base; the first that exports `chromium` wins. */
export const PLAYWRIGHT_PACKAGES = ["playwright", "playwright-core", "@playwright/test"] as const;

const INSTALL_REMEDY =
  "pnpm add -D playwright && pnpm exec playwright install chromium — in the app's directory or any above it";

/**
 * `chromium` from the first Playwright package that resolves from `cwd`, then
 * from this CLI's own install. A module that resolves but exports no
 * `chromium` is skipped rather than trusted.
 */
export async function loadChromium(cwd: string = process.cwd()): Promise<PwChromium> {
  const bases = [join(cwd, "package.json"), import.meta.url];
  for (const base of bases) {
    const resolver = createRequire(base);
    for (const name of PLAYWRIGHT_PACKAGES) {
      let resolved: string;
      try {
        resolved = resolver.resolve(name);
      } catch {
        continue;
      }
      const mod = (await import(pathToFileURL(resolved).href)) as {
        chromium?: PwChromium;
        default?: { chromium?: PwChromium };
      };
      const chromium = mod.chromium ?? mod.default?.chromium;
      if (typeof chromium?.launch === "function") return chromium;
    }
  }
  throw new CliError("the browser check needs Playwright, and none resolves from here", {
    code: ExitCode.USAGE,
    hint: `looked for ${PLAYWRIGHT_PACKAGES.join(", ")} from ${cwd} and from oxyc's own install`,
    remedy: INSTALL_REMEDY
  });
}

/**
 * Launch headless Chromium, turning a missing browser binary into the install
 * remedy, and a browser that cannot have its sandbox into an error that says
 * so. It is never retried without the sandbox.
 */
export async function launchChromium(
  chromium: PwChromium,
  choices: LaunchChoices = {}
): Promise<PwBrowser> {
  const options: PwLaunchOptions = { headless: true };
  if (choices.sandbox) options.chromiumSandbox = true;
  if (choices.args) options.args = choices.args;
  if (choices.env) options.env = choices.env;
  try {
    return await chromium.launch(options);
  } catch (cause) {
    const message = (cause as Error).message ?? String(cause);
    if (/Executable doesn't exist|playwright install/i.test(message)) {
      throw new CliError("Playwright resolves, but its Chromium is not installed", {
        code: ExitCode.USAGE,
        remedy: "pnpm exec playwright install chromium"
      });
    }
    if (choices.sandbox) {
      throw new CliError(
        `Chromium could not start with its sandbox on (${sandboxReason(message)}); the page is not opened without one`,
        {
          code: ExitCode.FAILURE,
          hint: "on Linux the sandbox needs unprivileged user namespaces (Ubuntu 23.10+: sysctl kernel.apparmor_restrict_unprivileged_userns=0, or an AppArmor profile for the browser), and a user that is not root"
        }
      );
    }
    throw new CliError(`could not start Chromium: ${message.split("\n")[0]}`, {
      code: ExitCode.FAILURE
    });
  }
}

/** Why a sandboxed launch failed, in a few words: Chromium's own reason when it gave one. */
function sandboxReason(message: string): string {
  if (/No usable sandbox/i.test(message)) return "no usable sandbox on this machine";
  if (/as root without --no-sandbox/i.test(message)) return "it does not sandbox a root user";
  return (message.split("\n")[0] ?? "").trim() || "the browser did not say why";
}
