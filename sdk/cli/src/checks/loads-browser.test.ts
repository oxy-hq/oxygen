/**
 * The promote checks' page load, driven through the Playwright seam by a
 * scripted stand-in (`testing/fake-playwright.ts`): what each request went
 * out with, and what the load recorded. No browser.
 *
 * What a page may reach, and what is never sent for it:
 * `loads-browser-reach.test.ts`.
 */

import { describe, expect, it } from "vitest";
import { FakeChromium } from "../testing/fake-playwright.js";
import {
  APP,
  BASE,
  BEACON,
  deskSent,
  KEY,
  ORIGIN,
  observe,
  STAGING_APP,
  target,
  WS
} from "../testing/observed-page.js";
import { observeAll } from "./loads-browser.js";
import { VIEWPORTS } from "./loads-target.js";

describe("observeAll", () => {
  it("opens the page in a fresh context per viewport, desk then phone, and closes everything", async () => {
    const { chromium, loads } = await observe({ requests: [BEACON], app: STAGING_APP });
    expect(chromium.launches).toBe(1);
    expect(chromium.contexts).toEqual([
      {
        serviceWorkers: "block",
        viewport: { width: 1440, height: 900 },
        deviceScaleFactor: 1,
        isMobile: false
      },
      {
        serviceWorkers: "block",
        viewport: { width: 390, height: 844 },
        deviceScaleFactor: 3,
        isMobile: true
      }
    ]);
    expect(loads.map(([viewport]) => viewport.name)).toEqual(["desk", "phone"]);
    expect(chromium.closedContexts).toBe(2);
    expect(chromium.browserClosed).toBe(true);
  });

  it("records a page that became ready on staging's build", async () => {
    const { desk, phone } = await observe({ requests: [BEACON], app: STAGING_APP });
    for (const observed of [desk, phone]) {
      expect(observed?.navigationError).toBeUndefined();
      expect(observed?.readyMs).toBeGreaterThanOrEqual(0);
      expect(observed?.app).toEqual({ environment: "staging", buildId: "b1" });
      expect(observed?.pageErrors).toEqual([]);
      expect(observed?.failedRequests).toEqual([]);
      expect(observed?.refused).toEqual([]);
      expect(observed?.offOrigin).toEqual([]);
    }
  });

  it("signs the document, its assets, the beacon and the app's own data reads; nothing else", async () => {
    const { chromium } = await observe({
      requests: [
        { url: `${BASE}assets/index.js` },
        { url: "https://fonts.example.com/a.woff2" },
        { url: `${ORIGIN}/api/orgs`, status: 401 },
        { method: "POST", url: `${ORIGIN}/api/projects/${WS}/query` },
        BEACON
      ],
      app: STAGING_APP
    });
    const own = { "user-agent": "fake-chromium" };
    const signed = { ...own, ...KEY };
    // A signed request is sent from outside the page, once, with redirects
    // off, and its answer handed back; anything else goes as the page sent it.
    expect(deskSent(chromium)).toEqual([
      {
        context: 0,
        method: "GET",
        url: BASE,
        decision: "fulfilled",
        headers: signed,
        maxRedirects: 0
      },
      {
        context: 0,
        method: "GET",
        url: `${BASE}assets/index.js`,
        decision: "fulfilled",
        headers: signed,
        maxRedirects: 0
      },
      // Another origin: never sent, whatever it is.
      {
        context: 0,
        method: "GET",
        url: "https://fonts.example.com/a.woff2",
        decision: "aborted",
        headers: own
      },
      // Passed, not signed: the page's own headers, and its redirect not followed either.
      {
        context: 0,
        method: "GET",
        url: `${ORIGIN}/api/orgs`,
        decision: "fulfilled",
        headers: own,
        maxRedirects: 0
      },
      {
        context: 0,
        method: "POST",
        url: `${ORIGIN}/api/projects/${WS}/query`,
        decision: "fulfilled",
        headers: { ...signed, "x-oxy-app": APP },
        maxRedirects: 0
      },
      {
        context: 0,
        method: "POST",
        url: `${BASE}__oxy/beacon`,
        decision: "fulfilled",
        headers: signed,
        maxRedirects: 0
      }
    ]);
  });

  it("starts Chromium with its sandbox on, and with no credential in its environment", async () => {
    const { chromium } = await observe({ requests: [BEACON], app: STAGING_APP }, target(), {
      PATH: "/usr/bin:/bin",
      HOME: "/home/runner",
      LANG: "en_US.UTF-8",
      LC_ALL: "C.UTF-8",
      DISPLAY: ":99",
      PLAYWRIGHT_BROWSERS_PATH: "/home/runner/.cache/ms-playwright",
      // The checker's token, by its usual name and by one `--api-key-env` could name.
      OXY_API_KEY: "oxysvc_x",
      OXY_TOKEN: "oxy_pat_person",
      CHECKER_CREDENTIAL_ELSEWHERE: "oxysvc_y",
      GITHUB_TOKEN: "ghs_x",
      ACTIONS_ID_TOKEN_REQUEST_TOKEN: "jwt"
    });
    expect(chromium.launchOptions).toEqual([
      {
        headless: true,
        chromiumSandbox: true,
        // The browser itself can find the app's origin and nothing else (`browser-confine.ts`).
        args: [
          "--host-resolver-rules=MAP app.example.com:443 app.example.com, MAP * ~NOTFOUND",
          "--no-proxy-server",
          "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
          "--disable-blink-features=WebSocketStream"
        ],
        env: {
          PATH: "/usr/bin:/bin",
          HOME: "/home/runner",
          LANG: "en_US.UTF-8",
          LC_ALL: "C.UTF-8",
          DISPLAY: ":99",
          PLAYWRIGHT_BROWSERS_PATH: "/home/runner/.cache/ms-playwright"
        }
      }
    ]);
  });

  it("is an error naming the sandbox when Chromium cannot start with it, and never starts without", async () => {
    const chromium = new FakeChromium(() => ({ requests: [BEACON], app: STAGING_APP }));
    chromium.launchError = [
      "browserType.launch: Target page, context or browser has been closed",
      "Browser logs:",
      "[pid=1][err] FATAL:zygote_host_impl_linux.cc(128)] No usable sandbox! If you are running on Ubuntu 23.10+ … you can try using --no-sandbox."
    ].join("\n");
    await expect(
      observeAll(target(), VIEWPORTS, { load: async () => chromium, env: {} })
    ).rejects.toThrow(
      "Chromium could not start with its sandbox on (no usable sandbox on this machine); the page is not opened without one"
    );
    // One launch, with the sandbox asked for: there is no second try without it.
    expect(chromium.launchOptions.map((o) => o.chromiumSandbox)).toEqual([true]);
    expect(chromium.contexts).toEqual([]);
  });

  it("survives a request whose page went away before it could be answered", async () => {
    // Playwright does not catch what a route handler throws: left uncaught, a
    // request still in flight when a context closes would end the whole command.
    const { desk } = await observe({
      requests: [
        { url: `${BASE}assets/late.js`, gone: true },
        { url: "https://fonts.example.com/late.woff2", gone: true },
        { method: "POST", url: `${BASE}fn/late`, gone: true },
        BEACON
      ],
      app: STAGING_APP
    });
    expect(desk?.readyMs).not.toBeNull();
    expect(desk?.failedRequests).toEqual([]);
  });

  it("fails a signed request for the page when its server does not answer, and says why", async () => {
    const { desk } = await observe({
      requests: [
        {
          url: `${BASE}assets/a.js`,
          failure: "apiRequestContext.fetch: connect ECONNREFUSED 10.0.0.1:443"
        },
        { url: `${BASE}assets/b.js`, failure: "apiRequestContext.fetch: Timeout 30000ms exceeded" },
        { url: `${BASE}assets/c.js`, failure: "something else" },
        BEACON
      ],
      app: STAGING_APP
    });
    expect(desk?.failedRequests).toEqual([
      { url: `${BASE}assets/a.js`, status: null, error: "net::ERR_CONNECTION_REFUSED" },
      { url: `${BASE}assets/b.js`, status: null, error: "net::ERR_TIMED_OUT" },
      { url: `${BASE}assets/c.js`, status: null, error: "net::ERR_FAILED" }
    ]);
  });

  it("records error answers and failed requests, but not the page's own cancels", async () => {
    const { desk } = await observe({
      requests: [
        { url: `${BASE}assets/missing.js`, status: 404 },
        { url: `${ORIGIN}/api/sql/query`, status: 401 },
        // Sent from here like every request: what fails is the request the check made for it.
        { url: `${ORIGIN}/static/a.js`, failure: "apiRequestContext.fetch: connect ECONNREFUSED" },
        { url: `${ORIGIN}/static/b.js`, cancelled: true },
        BEACON
      ],
      app: STAGING_APP
    });
    expect(desk?.failedRequests).toEqual([
      { url: `${BASE}assets/missing.js`, status: 404, error: null },
      { url: `${ORIGIN}/api/sql/query`, status: 401, error: null },
      { url: `${ORIGIN}/static/a.js`, status: null, error: "net::ERR_CONNECTION_REFUSED" }
    ]);
    // The call that went without the credential, outside the app's base.
    expect(desk?.denied).toEqual(["/api/sql/query answered 401"]);
  });

  it("records an uncaught page error by its name and first line", async () => {
    const error = new TypeError("x is undefined\n    at main (index.js:1:1)");
    const { desk } = await observe({ requests: [BEACON], errors: [error], app: STAGING_APP });
    expect(desk?.pageErrors).toEqual(["TypeError: x is undefined"]);
  });

  it("waits for a beacon that comes late, and no longer than that", async () => {
    const started = Date.now();
    const { desk, phone } = await observe(
      { requests: [{ ...BEACON, afterMs: 60 }], app: STAGING_APP },
      target({ waitMs: 5_000 })
    );
    // Seen when it came, from the navigation; not at once, and not after the whole wait.
    expect(desk?.readyMs).toBeGreaterThanOrEqual(50);
    expect(phone?.readyMs).toBeGreaterThanOrEqual(50);
    expect(Date.now() - started).toBeLessThan(2_500);
  });

  it("gives up on the beacon after the wait, and still reads the page", async () => {
    const started = Date.now();
    const { desk, phone } = await observe({ app: STAGING_APP }, target({ waitMs: 120 }));
    // Each viewport waited its whole time before giving up.
    expect(Date.now() - started).toBeGreaterThanOrEqual(220);
    expect(desk?.readyMs).toBeNull();
    expect(phone?.readyMs).toBeNull();
    expect(desk?.navigationError).toBeUndefined();
    expect(desk?.app).toEqual({ environment: "staging", buildId: "b1" });
  });

  it("lets a ready page's own calls answer before it reads what failed", async () => {
    const chromium = new FakeChromium(() => ({
      requests: [BEACON, { url: `${BASE}assets/late.js`, status: 500, afterMs: 40 }],
      app: STAGING_APP
    }));
    const settled = await observeAll(target(), VIEWPORTS, {
      load: async () => chromium,
      settleMs: 150
    });
    expect(settled[0]?.[1].failedRequests).toEqual([
      { url: `${BASE}assets/late.js`, status: 500, error: null }
    ]);
  });

  it("reads a page with no window.__OXY_APP__, or one that is not the app's, as it is", async () => {
    expect((await observe({ requests: [BEACON] })).desk?.app).toBeUndefined();
    expect((await observe({ requests: [BEACON], app: { environment: 7 } })).desk?.app).toEqual({
      environment: null,
      buildId: null
    });
  });

  it("names a document that answered an error, or did not answer at all", async () => {
    const missing = await observe({ document: 404 });
    expect(missing.desk?.navigationError).toBe("the page answered 404");
    expect(missing.desk?.failedRequests).toEqual([{ url: BASE, status: 404, error: null }]);
    const down = await observe({ document: "apiRequestContext.fetch: connect ECONNREFUSED" });
    expect(down.desk?.navigationError).toBe(
      `could not load ${BASE}: page.goto: net::ERR_CONNECTION_REFUSED at ${BASE}`
    );
    expect(down.phone?.navigationError).toBeDefined();
  });

  it("names a redirect that left the app: a sign-in page", async () => {
    const { desk } = await observe({ landed: `${ORIGIN}/login?next=%2Fcustomer-apps` });
    expect(desk?.navigationError).toBe(
      "redirected to /login — the checker's credential was not accepted (a sign-in page)"
    );
  });

  it("stops a page that never answers, and closes the browser", async () => {
    const chromium = new FakeChromium(() => ({ requests: [BEACON], hangs: true }));
    await expect(
      observeAll(target(), VIEWPORTS, { load: async () => chromium, settleMs: 0, deadlineMs: 60 })
    ).rejects.toThrow("the browser check did not finish within 0s");
    expect(chromium.browserClosed).toBe(true);
  });
});
