/**
 * A stand-in for Playwright's Chromium, for the promote checks' tests: it
 * plays a scripted page through the same seam the real one is driven by
 * (`checks/playwright.ts`), and records what the check did with each request.
 *
 * No browser is launched. What is real is the check's own code: its route
 * handler judges every scripted request, its listeners hear the responses,
 * and each function it hands to `evaluate` runs here against the scripted
 * page (`window.__OXY_APP__`, its links, its text: `fake-page.ts`). A
 * picture is a few scripted pixels, and two of them are compared by the
 * check's own in-page code. An expression is evaluated as the real one
 * evaluates it: with no argument, and never called.
 *
 * A context holds any number of pages, opened one after the other; the
 * script is asked what each does by the context's index and the URL it is
 * sent to.
 *
 * How one request plays out, and how a redirect is followed, is
 * `fake-requests.ts`: REDIRECTS BEHAVE AS CHROMIUM'S DO UNDER PLAYWRIGHT.
 */

import type {
  PwBrowser,
  PwChromium,
  PwContext,
  PwContextOptions,
  PwLaunchOptions,
  PwPage,
  PwScreenshotOptions
} from "../checks/playwright.js";
import { fakePng, inFakePage, leftByExpression, type PageContent } from "./fake-page.js";
import {
  answer,
  type Listeners,
  openScripted,
  type Routed,
  type ScriptedRequest,
  type Sent,
  type Socket,
  sendScripted,
  type Wire
} from "./fake-requests.js";

export type { ScriptedRequest, Sent, Socket } from "./fake-requests.js";

/** What one page does when it is opened, and what it holds once it is up. */
export interface PageScript extends PageContent {
  /** The document's status (default 200); a string is a server that does not answer, and why. */
  document?: number | string;
  /** The document's URL answers a redirect to this one. */
  documentRedirectTo?: string;
  requests?: ScriptedRequest[];
  /** The WebSockets the page constructs, by URL, after its requests. */
  sockets?: string[];
  /** Uncaught errors the page throws. */
  errors?: Error[];
  /** Where the page ends up by its own doing (a client-side redirect). */
  landed?: string;
  /** The page never answers `evaluate`: it is stuck in a loop. */
  hangs?: boolean;
  /** The page's picture (`fakePng`); two white pixels square when absent, `null` when it cannot be taken. */
  shot?: Uint8Array | null;
}

/** A script the check asked a context to run before each page's own code. */
export interface InitScript {
  context: number;
  /** The function handed over, and what it is given: a test runs it against a page of its own. */
  run: (arg: unknown) => void;
  arg: unknown;
}

/** One picture the check took. */
export interface Shot {
  context: number;
  /** Where the page was when it was pictured. */
  url: string;
  options: PwScreenshotOptions;
}

/** Waits `ms`, or not at all. */
const pause = (ms: number | undefined): Promise<void> =>
  ms ? new Promise((resolve) => setTimeout(resolve, ms)) : Promise.resolve();

export class FakeChromium implements PwChromium {
  /** The options each launch was asked for, in order. */
  readonly launchOptions: PwLaunchOptions[] = [];
  /** The options each context was created with, in order. */
  readonly contexts: PwContextOptions[] = [];
  readonly sent: Sent[] = [];
  /** Every script a context was told to run before a page's own, in order. */
  readonly initScripts: InitScript[] = [];
  /** Every WebSocket a page asked for, and what became of it. */
  readonly sockets: Socket[] = [];
  /** Every picture taken, in order. */
  readonly shots: Shot[] = [];
  /** How many pairs of pictures were compared. */
  compared = 0;
  closedPages = 0;
  closedContexts = 0;
  browserClosed = false;
  /** Set to make every launch fail with this message. */
  launchError?: string;
  /** Set to stand in for a Playwright before 1.48: a context cannot be handed a page's WebSockets. */
  withoutSocketRoutes = false;

  /** `script` is asked once per page opened: by its context's index, and the URL it is sent to. */
  constructor(private readonly script: (context: number, url: string) => PageScript) {}

  get launches(): number {
    return this.launchOptions.length;
  }

  async launch(opts: PwLaunchOptions): Promise<PwBrowser> {
    this.launchOptions.push(opts);
    if (this.launchError !== undefined) throw new Error(this.launchError);
    return {
      newContext: async (options) => {
        const index = this.contexts.push(options) - 1;
        return this.context(index);
      },
      close: async () => {
        this.browserClosed = true;
      }
    };
  }

  private context(index: number): PwContext {
    const routed: Routed = {};
    const context = this.routable(index, routed);
    if (!this.withoutSocketRoutes) return context;
    return { ...context, routeWebSocket: undefined } as unknown as PwContext;
  }

  private routable(index: number, routed: Routed): PwContext {
    return {
      route: async (_pattern, routeHandler) => {
        routed.handler = routeHandler;
      },
      addInitScript: async (run, arg) => {
        this.initScripts.push({ context: index, run: run as InitScript["run"], arg });
      },
      routeWebSocket: async (_pattern, socketHandler) => {
        routed.socket = socketHandler;
      },
      newPage: async () => this.page(index, routed),
      close: async () => {
        this.closedContexts += 1;
      }
    };
  }

  /** One page of a context: its own listeners, and the context's one route handler. */
  private page(index: number, routed: Routed): PwPage {
    // Until it is sent somewhere it is a blank page: it holds nothing.
    let script: PageScript = {};
    /** What the expressions evaluated in this page left on its global object. */
    const installed: Record<string, unknown> = {};
    const listeners: Listeners = { pageerror: [], response: [], requestfailed: [] };
    /** Every request of this page goes through the context's handler, and is recorded here. */
    const wire: Wire = { context: index, routed, listeners, sent: this.sent };

    let landed = "about:blank";
    const page: PwPage = {
      on: (event: keyof Listeners, fn: never) => {
        (listeners[event] as unknown[]).push(fn);
      },
      goto: async (url) => {
        script = this.script(index, url);
        // With no status scripted, a document answers 200, or 302 when it redirects.
        const document = script.document;
        const ended = await sendScripted(
          wire,
          {
            url,
            status: typeof document === "number" ? document : undefined,
            failure: typeof document === "string" ? document : undefined,
            redirectTo: script.documentRedirectTo
          },
          true
        );
        if (ended.failure !== undefined) {
          throw new Error(`page.goto: ${ended.failure} at ${url}\nCall log:\n  - navigating`);
        }
        landed = script.landed ?? ended.url;
        // The page's own requests and errors come after the document answered.
        const throwOnPage = (error: Error) => {
          for (const fn of listeners.pageerror) fn(error);
        };
        void (async () => {
          for (const scripted of script.requests ?? []) {
            await pause(scripted.afterMs);
            await sendScripted(wire, scripted, false);
          }
          for (const url of script.sockets ?? []) {
            this.sockets.push(await openScripted(routed, index, url));
          }
          for (const error of script.errors ?? []) throwOnPage(error);
        })();
        return answer(ended.url, ended.status);
      },
      url: () => landed,
      evaluate: async <T>(run: unknown, arg?: unknown): Promise<T> => {
        if (script.hangs) return new Promise<T>(() => undefined);
        // An expression gets no argument, and a function it evaluates to is not called: what
        // it leaves on the page is there for the functions that follow.
        if (typeof run === "string") {
          Object.assign(installed, leftByExpression(run));
          return undefined as T;
        }
        if (typeof arg === "object" && arg !== null && "a" in arg && "b" in arg) this.compared += 1;
        return inFakePage(script, () => (run as (arg: unknown) => T | Promise<T>)(arg), installed);
      },
      screenshot: async (options) => {
        this.shots.push({ context: index, url: landed, options });
        if (script.shot === null) throw new Error("page.screenshot: the page crashed");
        return script.shot ?? fakePng(2, 2, [255, 255, 255, 255]);
      },
      close: async () => {
        this.closedPages += 1;
      },
      waitForRequest: () => Promise.reject(new Error("not scripted")),
      waitForLoadState: async () => undefined,
      locator: () => {
        throw new Error("not scripted");
      }
    };
    return page;
  }
}
