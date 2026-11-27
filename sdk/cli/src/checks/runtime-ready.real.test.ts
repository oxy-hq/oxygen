/**
 * The platform runtime's ready signal, in a REAL Chromium. The loads and screens
 * checks trust `oxy-app-ready` to mean "the app drew something", so it must not
 * fire for a page that drew nothing.
 *
 * Every template ships its mount node as an empty `#root` sized to the
 * viewport. The runtime called any element with height mounted, so that empty
 * box made a bundle that never mounted, or whose root a crash emptied, ready:
 * a blank screen that throws nothing passed both checks.
 *
 * OPT-IN, and local only, like `browser-confine.real.test.ts`:
 *
 *   OXY_CHECKS_REAL_CHROMIUM=<path to chrome-headless-shell or Chrome> \
 *     pnpm exec vitest run src/checks/runtime-ready.real.test.ts
 *
 * Nothing leaves the browser: the page and its beacon are answered by routes.
 */

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { loadChromium } from "./playwright.js";

const EXECUTABLE = process.env.OXY_CHECKS_REAL_CHROMIUM;
const REPO = resolve(fileURLToPath(new URL("../../../../", import.meta.url)));
const RUNTIME = readFileSync(resolve(REPO, "crates/api-custom-apps/src/client/runtime.js"), "utf8");
const ORIGIN = "https://app.test";
const BASE = "/customer-apps/o/a/";

/** A served page: the app's config, the runtime, and `body`, as the platform serves a bundle. */
function document(body: string): string {
  return `<!doctype html><html><head><meta charset="utf-8">
<style>html,body{height:100%;margin:0}#root{height:100%}</style>
<script>window.__OXY_APP__ = { basePath: "${BASE}", buildId: "b1", analytics: false };</script>
<script>${RUNTIME}</script></head><body>${body}</body></html>`;
}

// Minimal shapes of the Playwright objects this test drives.
interface Route {
  request(): { url(): string; postData(): string | null };
  fulfill(r: { status: number; contentType?: string; body?: string }): Promise<void>;
}
interface Page {
  route(url: string, handler: (route: Route) => Promise<void>): Promise<void>;
  goto(url: string): Promise<unknown>;
  waitForTimeout(ms: number): Promise<void>;
  close(): Promise<void>;
}
interface Browser {
  newPage(): Promise<Page>;
  close(): Promise<void>;
}

// A page's close waits for the beacon it sent on its way out: give each case room.
/** The names of the events in one beacon batch (`{ v, events: [{ n, … }] }`). */
function eventNames(postData: string | null): string[] {
  const batch = JSON.parse(postData ?? "{}") as { events?: { n?: string }[] };
  return (batch.events ?? []).flatMap((e) => (e?.n ? [e.n] : []));
}

/** The app's origin, answered here: its beacon recorded into `sent`, its page `body`, nothing else. */
async function answer(route: Route, body: string, sent: string[]): Promise<void> {
  const url = route.request().url();
  if (url.endsWith("/__oxy/beacon")) {
    sent.push(...eventNames(route.request().postData()));
    return route.fulfill({ status: 204 });
  }
  const isPage = url.startsWith(`${ORIGIN}${BASE}`) && !url.includes("__oxy/");
  return isPage
    ? route.fulfill({ status: 200, contentType: "text/html", body: document(body) })
    : route.fulfill({ status: 404 });
}

describe.skipIf(!EXECUTABLE)(
  "the runtime's ready signal, in a real Chromium",
  { timeout: 20_000 },
  () => {
    let browser: Browser;

    beforeAll(async () => {
      const chromium = await loadChromium(resolve(REPO, "web-app"));
      browser = (await (chromium.launch as (o: unknown) => Promise<unknown>)({
        executablePath: EXECUTABLE
      })) as Browser;
    });
    afterAll(async () => {
      await browser?.close();
    });

    /** Open `body` as the app's page; the beacon events it sent within `ms`. */
    async function opened(body: string, ms = 1600): Promise<string[]> {
      const page = await browser.newPage();
      const sent: string[] = [];
      await page.route(`${ORIGIN}/**`, (route) => answer(route, body, sent));
      await page.goto(`${ORIGIN}${BASE}`);
      await page.waitForTimeout(ms);
      await page.close();
      return sent;
    }

    it("says nothing for the empty mount node every template ships", async () => {
      expect(await opened('<div id="root"></div>')).not.toContain("oxy-app-ready");
    });

    it("says ready once the app has drawn into it", async () => {
      expect(await opened('<div id="root"><main><h1>Overview</h1></main></div>')).toContain(
        "oxy-app-ready"
      );
    });

    it("says ready for an app that mounts a moment later", async () => {
      const late =
        '<div id="root"></div><script>setTimeout(function(){document.getElementById("root").innerHTML="<p>Stores</p>"},400)</script>';
      expect(await opened(late)).toContain("oxy-app-ready");
    });

    it("counts an app drawn inside an open shadow root as drawn", async () => {
      const shadow =
        '<oxy-app style="display:block;height:100%"></oxy-app><script>document.querySelector("oxy-app").attachShadow({mode:"open"}).innerHTML="<p>Stores</p>"</script>';
      expect(await opened(shadow)).toContain("oxy-app-ready");
    });

    it("counts a picture or a canvas with no text as drawn", async () => {
      expect(
        await opened('<div id="root"><canvas width="300" height="150"></canvas></div>')
      ).toContain("oxy-app-ready");
    });
  }
);
