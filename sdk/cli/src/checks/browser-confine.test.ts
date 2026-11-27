/**
 * What holds the promote checks' browser to one origin beyond the requests
 * the check is shown: the flags Chromium is started with, and what each
 * context loses. Whether Chromium honours them is `browser-confine.real.test.ts`.
 */

import { Script } from "node:vm";
import { describe, expect, it } from "vitest";
import { FakeChromium } from "../testing/fake-playwright.js";
import {
  confine,
  confinementArgs,
  PEER_CONSTRUCTORS,
  withoutPeerConnections
} from "./browser-confine.js";
import { observeAll } from "./loads-browser.js";
import { loadsTarget, VIEWPORTS } from "./loads-target.js";
import { RUN_CLOCK, startDateAt } from "./page-clock.js";
import type { PwContext } from "./playwright.js";
import { claimTarget, DEFAULT_DEPS } from "./run-deps.js";

describe("confinementArgs", () => {
  it("lets the browser find the app's own host and port, and no other", () => {
    expect(confinementArgs("https://app.example.com")).toEqual([
      "--host-resolver-rules=MAP app.example.com:443 app.example.com, MAP * ~NOTFOUND",
      "--no-proxy-server",
      "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
      "--disable-blink-features=WebSocketStream"
    ]);
  });

  it("names the port an origin leaves out, and the one it gives", () => {
    const rule = (origin: string) => confinementArgs(origin)[0];
    expect(rule("http://app.example.com")).toBe(
      "--host-resolver-rules=MAP app.example.com:80 app.example.com, MAP * ~NOTFOUND"
    );
    expect(rule("http://localhost:3000")).toBe(
      "--host-resolver-rules=MAP localhost:3000 localhost, MAP * ~NOTFOUND"
    );
    expect(rule("http://127.0.0.1:4010/")).toBe(
      "--host-resolver-rules=MAP 127.0.0.1:4010 127.0.0.1, MAP * ~NOTFOUND"
    );
    expect(rule("https://[::1]:8443")).toBe(
      "--host-resolver-rules=MAP [::1]:8443 [::1], MAP * ~NOTFOUND"
    );
    // A name is matched as Chromium spells it: lower-case.
    expect(rule("https://App.Example.COM")).toBe(
      "--host-resolver-rules=MAP app.example.com:443 app.example.com, MAP * ~NOTFOUND"
    );
  });

  it("refuses an origin it cannot hold a browser to", () => {
    expect(() => confinementArgs("ftp://app.example.com")).toThrow(/not an http or https origin/);
    expect(() => confinementArgs("nonsense")).toThrow();
  });
});

describe("withoutPeerConnections", () => {
  /** Run the function as a page would get it: as its source, in a world of its own. */
  function inPage(page: Record<string, unknown>): Record<string, unknown> {
    const source = `(${withoutPeerConnections.toString()})(${JSON.stringify(PEER_CONSTRUCTORS)})`;
    new Script(source).runInNewContext(page);
    return page;
  }

  it("takes the peer-connection constructors out of a page, and they cannot be put back", () => {
    class Connection {}
    const page = inPage({
      RTCPeerConnection: Connection,
      webkitRTCPeerConnection: Connection,
      fetch: () => undefined
    });
    expect(page.RTCPeerConnection).toBeUndefined();
    expect(page.webkitRTCPeerConnection).toBeUndefined();
    // Nothing else of the page is touched.
    expect(typeof page.fetch).toBe("function");
    expect(() => {
      page.RTCPeerConnection = Connection;
    }).toThrow(TypeError);
    expect(() => Object.defineProperty(page, "RTCPeerConnection", { value: Connection })).toThrow(
      TypeError
    );
    expect(page.RTCPeerConnection).toBeUndefined();
  });

  it("does nothing to a page that has none, twice over", () => {
    const page = inPage(inPage({}));
    expect(page.RTCPeerConnection).toBeUndefined();
  });
});

const TARGET = {
  ...loadsTarget({
    pageUrl: "https://app.example.com/customer-apps/acme/store/",
    deployment: "https://app.example.com",
    credential: { "x-api-key": "oxysvc_x" },
    appId: "00000000-0000-0000-0000-0000000000a1",
    environment: "staging",
    expectedBuild: "b1"
  }),
  waitMs: 20
};

describe("confine", () => {
  it("is asked of every context a check opens a page in", async () => {
    const chromium = new FakeChromium(() => ({}));
    await observeAll(TARGET, VIEWPORTS, { load: async () => chromium, settleMs: 0, env: {} });
    expect(chromium.initScripts).toEqual([
      { context: 0, run: withoutPeerConnections, arg: PEER_CONSTRUCTORS },
      { context: 1, run: withoutPeerConnections, arg: PEER_CONSTRUCTORS }
    ]);
  });

  // A clock the page draws differed between staging's picture and production's,
  // taken a minute apart, so every screen "changed".
  it("gives every page of a run the run's one instant", async () => {
    const chromium = new FakeChromium(() => ({}));
    const at = Date.UTC(2026, 10, 25, 9, 14);
    await observeAll({ ...TARGET, clockAt: at }, VIEWPORTS, {
      load: async () => chromium,
      settleMs: 0,
      env: {}
    });
    expect(chromium.initScripts.filter((s) => s.run === startDateAt)).toEqual([
      { context: 0, run: startDateAt, arg: at },
      { context: 1, run: startDateAt, arg: at }
    ]);
  });

  it("names one instant for staging's pages and production's in a run", () => {
    const claim = {
      url: "https://app.example.com/customer-apps/acme/store/",
      app: { id: "00000000-0000-0000-0000-0000000000a1", workspace_id: null }
    } as unknown as Parameters<typeof claimTarget>[0];
    const creds = {
      target: "https://app.example.com",
      apiKey: "oxysvc_x"
    } as unknown as Parameters<typeof claimTarget>[1];
    const staging = claimTarget(claim, creds, DEFAULT_DEPS, "staging", "b2");
    const production = claimTarget(claim, creds, DEFAULT_DEPS, "production", "b1");
    expect(staging.clockAt).toBe(RUN_CLOCK);
    expect(production.clockAt).toBe(RUN_CLOCK);
  });

  it("opens no page with a Playwright that cannot confine one", async () => {
    const old = { route: async () => undefined } as unknown as PwContext;
    await expect(confine(old, () => undefined)).rejects.toThrow(
      "this Playwright cannot run a script before a page's own: the page is not opened"
    );
    // Before 1.48 a context has no way to be handed a page's WebSockets.
    const older = { addInitScript: async () => undefined } as unknown as PwContext;
    await expect(confine(older, () => undefined)).rejects.toThrow(
      "this Playwright cannot refuse a page's WebSockets (it needs 1.48 or later): the page is not opened"
    );
  });

  it("is an error of the check, not a verdict on the page, when a page cannot be confined", async () => {
    const chromium = new FakeChromium(() => ({}));
    chromium.withoutSocketRoutes = true;
    await expect(
      observeAll(TARGET, VIEWPORTS, { load: async () => chromium, settleMs: 0, env: {} })
    ).rejects.toThrow(
      "this Playwright cannot refuse a page's WebSockets (it needs 1.48 or later): the page is not opened"
    );
    // No page was opened in the context that could not be confined, and everything was closed.
    expect(chromium.sent).toEqual([]);
    expect(chromium.closedContexts).toBe(1);
    expect(chromium.browserClosed).toBe(true);
  });

  it("ends every WebSocket it is handed, and says which", async () => {
    const asked: string[] = [];
    const closed: unknown[] = [];
    let handler: Parameters<PwContext["routeWebSocket"]>[1] | undefined;
    const context = {
      addInitScript: async () => undefined,
      routeWebSocket: async (pattern: RegExp, given: typeof handler) => {
        // Every socket, whatever its URL.
        expect(pattern.test("wss://anything.example/x")).toBe(true);
        handler = given;
      }
    } as unknown as PwContext;
    await confine(context, (url) => asked.push(url));
    await handler?.({
      url: () => "wss://attacker.example/live",
      close: async (opts) => {
        closed.push(opts);
      }
    });
    expect(asked).toEqual(["wss://attacker.example/live"]);
    // 1008: a policy refused it.
    expect(closed).toEqual([{ code: 1008, reason: "not opened: the page is being checked" }]);
  });
});
