/**
 * The promote checks' page, in a REAL Chromium: a hostile app reads its own
 * data as the checker and then tries every way out a page has
 * (`testing/hostile-app.ts`). What the scripted stand-in cannot show is shown
 * here: what the browser itself does with a redirect it follows, a socket, a
 * peer connection, a hint in the markup.
 *
 * OPT-IN, and local only. It needs a Chromium on this machine and Playwright
 * resolvable from `web-app`, so it runs only when told where the browser is:
 *
 *   OXY_CHECKS_REAL_CHROMIUM=<path to chrome-headless-shell or Chrome> \
 *     pnpm exec vitest run src/checks/browser-confine.real.test.ts
 *
 * Both servers listen on the loopback interface; nothing is asked of any
 * other host, and the point of the test is that nothing could be.
 */

import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { HOSTILE_WS, type HostileWorld, startHostileWorld } from "../testing/hostile-app.js";
import { confine, confinementArgs } from "./browser-confine.js";
import { browserEnv } from "./browser-env.js";
import { observeAll } from "./loads-browser.js";
import type { Observed } from "./loads-score.js";
import { loadsTarget, VIEWPORTS } from "./loads-target.js";
import { launchChromium, loadChromium, type PwChromium } from "./playwright.js";

const EXECUTABLE = process.env.OXY_CHECKS_REAL_CHROMIUM;
const WEB_APP = resolve(fileURLToPath(new URL("../../../../web-app", import.meta.url)));
const BUILD = "b1";

/**
 * The Playwright `web-app` installs, starting the browser this test was
 * pointed at. `publicPorts` are loopback ports the browser is told to count
 * as PUBLIC addresses. Chromium itself keeps a page from asking a loopback or
 * private address for anything (Local Network Access), which would hide what
 * the same page could send to a real host on the internet: with the override
 * the other server stands in for one. The app's own port is named too where
 * the check hands the browser its document, since such a page has no address
 * of its own and would be kept from its own loopback origin's sockets.
 */
async function realChromium(publicPorts: number[]): Promise<PwChromium> {
  const chromium = await loadChromium(WEB_APP);
  const asPublic = publicPorts.map((port) => `127.0.0.1:${port}=public`).join(",");
  return {
    launch: (options) =>
      (chromium.launch as (o: unknown) => ReturnType<PwChromium["launch"]>)({
        ...options,
        args: [...(options.args ?? []), `--ip-address-space-overrides=${asPublic}`],
        executablePath: EXECUTABLE
      })
  };
}

type Said = ReturnType<HostileWorld["report"]>;
type Seen = Observed | undefined;
const notPublic = (what: string) =>
  expect.stringMatching(new RegExp(`^${what} \\(not a public address\\)$`));
const OTHER = "http:\\/\\/127\\.0\\.0\\.1:\\d+";

/** The page ran to its end, and its own data read was answered: the check still reads. */
function expectStillReads(_world: HostileWorld, desk: Seen, said: Said): void {
  expect(said?.rows).toBe('{"rows":[["payroll",12900]]}');
  expect(desk?.readyMs).not.toBeNull();
  expect(desk?.app).toEqual({ environment: "staging", buildId: BUILD });
  expect(desk?.pageErrors).toEqual([]);
}

/** Not one request, upgrade, connection or datagram arrived anywhere but the app's origin. */
function expectNothingLeft(world: HostileWorld, desk: Seen, said: Said): void {
  expect(world.leaked()).toEqual([]);
  // Its own constructors for a peer connection are gone, in a fresh frame too.
  expect(said).toMatchObject({ peerConnection: "undefined", framePeerConnection: "undefined" });
  // What the check was shown, it refused and recorded; the page still came up.
  expect(desk?.offOrigin).toEqual(
    expect.arrayContaining([
      notPublic(`GET ${OTHER}\\/fetch\\?d=.*`),
      notPublic(`POST ${OTHER}\\/beacon`),
      notPublic(`POST ${OTHER}\\/ping`),
      notPublic(`GET ${OTHER}\\/popup`)
    ])
  );
}

/** A socket is never opened, on the app's own origin either, and no service worker installs. */
function expectNoSocketOrWorker(world: HostileWorld, desk: Seen, said: Said): void {
  expect(said).toMatchObject({
    socket: "closed 1008",
    ownSocket: "closed 1008",
    socketStream: "undefined"
  });
  expect(world.app.hits).not.toContain("UPGRADE /own-socket");
  expect(desk?.refused).toEqual(["WS /own-socket"]);
  // KNOWN, AND NOT CLOSED: Playwright hands the check no worker's socket, so a worker's
  // to the app's OWN origin is opened. It is the platform that receives it, and no one else.
  expect(world.app.hits).toContain("UPGRADE /worker-own-socket");
  // No service worker installs, by any of the ways a page has to ask for one.
  expect(said).toMatchObject({
    serviceWorker: ["nothing", "nothing", "nothing"],
    controlled: false
  });
  expect(world.app.hits.filter((hit) => hit.includes("sw.js"))).toEqual([]);
}

/** A redirect of a request the check passes is read by the check, and its next hop judged. */
function expectRedirectsJudged(desk: Seen): void {
  expect(desk?.offOrigin).toEqual(
    expect.arrayContaining([
      notPublic(`GET ${OTHER}\\/redirected`),
      notPublic("WS ws:\\/\\/127\\.0\\.0\\.1:\\d+\\/socket")
    ])
  );
  // What the browser sent by itself (the second hop of a redirect; in the headless shell
  // a speculation prefetch too) found no host to go to, and is a failed request.
  const lost = desk?.failedRequests ?? [];
  expect(lost.map((request) => request.url)).toContainEqual(
    expect.stringMatching(new RegExp(`^${OTHER}\\/second-hop$`))
  );
  expect(new Set(lost.map((request) => request.error))).toEqual(
    new Set(["net::ERR_NAME_NOT_RESOLVED"])
  );
}

describe.skipIf(!EXECUTABLE)("a checked page in a real Chromium", { timeout: 90_000 }, () => {
  let world: HostileWorld;

  beforeEach(async () => {
    world = await startHostileWorld(BUILD);
  });

  afterEach(async () => {
    await world.close();
  });

  const target = () =>
    loadsTarget({
      pageUrl: world.pageUrl,
      deployment: world.origin,
      credential: { "x-api-key": "oxysvc_x" },
      appId: "00000000-0000-0000-0000-0000000000a1",
      workspaceId: HOSTILE_WS,
      environment: "staging",
      expectedBuild: BUILD,
      waitMs: 45_000
    });

  it("reads the app's data as the checker, and sends none of it anywhere else", async () => {
    const [[, desk] = []] = await observeAll(target(), VIEWPORTS.slice(0, 1), {
      load: () => realChromium([world.app.port, world.other.port, world.tcp.port]),
      settleMs: 1_000,
      env: process.env
    });
    const said = world.report();
    expectStillReads(world, desk, said);
    expectNothingLeft(world, desk, said);
    expectNoSocketOrWorker(world, desk, said);
    expectRedirectsJudged(desk);
  });

  /**
   * The browser by itself: no request is judged, and each one is let through with the
   * credential on it. `confined` takes from the context what `confine` takes.
   */
  async function letEverythingThrough(confined: boolean): Promise<void> {
    const ports = [world.app.port, world.other.port, world.tcp.port];
    const browser = await launchChromium(await realChromium(ports), {
      sandbox: true,
      args: confinementArgs(world.origin),
      env: browserEnv(process.env)
    });
    try {
      const context = await browser.newContext({ serviceWorkers: "block" });
      if (confined) await confine(context, () => undefined);
      await context.route("**/*", (route) =>
        route
          .continue({ headers: { ...route.request().headers(), "x-api-key": "oxysvc_x" } })
          .catch(() => undefined)
      );
      const page = await context.newPage();
      const ready = page.waitForRequest((request) => request.url().endsWith("__oxy/beacon"), {
        timeout: 60_000
      });
      await page.goto(world.pageUrl, { timeout: 30_000, waitUntil: "load" });
      await ready;
    } finally {
      await browser.close();
    }
  }

  it("finds no other host even with every request let through, signed", async () => {
    await letEverythingThrough(true);
    expect(world.report()?.rows).toBe('{"rows":[["payroll",12900]]}');
    // Both redirects were followed by the browser, as far as the app's own origin goes.
    expect(world.app.hits).toEqual(
      expect.arrayContaining(["GET /api/redirect-away", "GET /api/second-hop"])
    );
    expect(world.leaked()).toEqual([]);
  });

  it("holds a peer connection a page was left with to the resolver, and its UDP to the flag", async () => {
    // Without `confine`: the page has its peer connections, and uses them.
    await letEverythingThrough(false);
    expect(world.report()).toMatchObject({ peerConnection: "function" });
    // TURN over TCP asks the resolver, like everything that is HTTP: nothing arrived.
    expect([...world.other.hits, ...world.tcp.hits]).toEqual([]);
    // STUN is UDP to an address, which only the flag stops; and the flag is honoured by the
    // headless shell, not by full Chrome. QUIC (1250 bytes a datagram) never arrives.
    const stun = world.udp.hits.filter((hit) => hit === "UDP 20 bytes");
    expect(world.udp.hits).toEqual(stun);
    if (/headless[-_]shell/.test(EXECUTABLE ?? "")) expect(stun).toEqual([]);
    else expect(stun.length).toBeGreaterThan(0);
  });
});
