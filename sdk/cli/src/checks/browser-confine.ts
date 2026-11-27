/**
 * What keeps the promote checks' browser on ONE origin, the app's own, beyond
 * the requests the check is shown.
 *
 * `loads-target.ts` `treat` judges every request Playwright hands the check.
 * That is not every way a page reaches the network: Chromium follows a
 * redirect, opens a socket or a peer connection, and acts on a hint in the
 * markup without asking. A page that read the app's data as the checker must
 * have no such way out either, so the browser itself is started unable to
 * find any other host ({@link confinementArgs}), and each context loses what
 * the flags cannot take away ({@link confine}).
 *
 * WHAT EACH PATH MEETS. Each was seen in a real Chromium, unless it says
 * otherwise (`browser-confine.real.test.ts`: Playwright 1.65.1 driving the
 * headless shell of Chromium 153, and Chrome 158):
 *
 * - A redirect the browser follows, on any hop after the one the check
 *   judged (`guarded-route.ts`); a WebSocket from a worker; `WebTransport`; a
 *   speculation-rules prefetch: none is shown to the check. Each needs an
 *   address for a host, and the browser's resolver answers only for the
 *   app's own host and port (`--host-resolver-rules`). An IP literal is
 *   "resolved" by the same resolver, so `http://169.254.169.254/` and
 *   `ws://127.0.0.1:9000/` fail the same way, and so does the app's own host
 *   on another port.
 * - `preconnect` and `dns-prefetch` send no request to judge either. NOT
 *   SEEN: neither opened a connection in those runs, with the rule or
 *   without, and a lookup cannot be watched on a loopback. A name they would
 *   look up is asked of the same resolver.
 * - A `WebSocket` a page or a frame constructs is handed to the check
 *   (`routeWebSocket`, Playwright 1.48 and later), which ends it before
 *   anything is sent, on the app's own origin too, and records it.
 *   `WebSocketStream`, the other constructor for the same socket, is not
 *   handed over by Playwright, so the browser is started without it
 *   (`--disable-blink-features`).
 * - `fetch`, `sendBeacon`, `<a ping>`, an image, a frame, a popup,
 *   `prefetch`/`preload`, a worker's `fetch`: all shown to the check, which
 *   refuses them first. The resolver is behind each of them too.
 * - A peer connection (WebRTC) asks the resolver for nothing when its servers
 *   are IP literals, and its STUN is UDP, which no rule above sees. Three
 *   things, because none is enough alone: the constructors are taken out of
 *   every frame before the page's code runs ({@link withoutPeerConnections});
 *   UDP that is not proxied is off (`--force-webrtc-ip-handling-policy`,
 *   honoured by the headless shell Playwright starts, NOT by full Chrome in
 *   its new headless mode); and TURN over TCP goes through the resolver.
 * - A system or environment proxy would resolve names itself, past the rule:
 *   `--no-proxy-server`. NOT SEEN: no run here had a proxy to ignore.
 * - A service worker: the context blocks them (`serviceWorkers: "block"`),
 *   and none installed by any way a page has to ask for one.
 *
 * NOT CLOSED: a worker's own `WebSocket` to the APP'S origin. Playwright
 * hands over no worker's socket. It reaches the platform as an anonymous
 * visitor's would, and nothing else: to any other host it finds no address.
 */

import type { PwContext } from "./playwright.js";

/** A host as the resolver's rules spell one: a name, an IPv4 address, or an IPv6 one in brackets. */
const HOST = /^(?:[a-z0-9.-]+|\[[0-9a-f:.]+\])$/;

/**
 * The Chromium flags that leave the browser able to reach `origin` and no
 * other host or port. Throws on an origin that is not `http(s)://host[:port]`.
 */
export function confinementArgs(origin: string): string[] {
  const url = new URL(origin);
  const secure = url.protocol === "https:";
  if (!secure && url.protocol !== "http:") {
    throw new Error(`the browser cannot be held to ${origin}: not an http or https origin`);
  }
  const host = url.hostname.toLowerCase();
  if (!HOST.test(host)) {
    throw new Error(`the browser cannot be held to ${origin}: its host is not one a rule can name`);
  }
  const port = url.port || (secure ? "443" : "80");
  return [
    // The first rule that matches a host wins: the app's own host and port resolve
    // as they are, and every other name, address and port is not found.
    `--host-resolver-rules=MAP ${host}:${port} ${host}, MAP * ~NOTFOUND`,
    "--no-proxy-server",
    "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
    "--disable-blink-features=WebSocketStream"
  ];
}

/** The constructors a page opens a peer connection with; nothing else of WebRTC makes one. */
export const PEER_CONSTRUCTORS = ["RTCPeerConnection", "webkitRTCPeerConnection"];

/**
 * Take the peer-connection constructors out of this frame, for good. Run in
 * the page before any of its code, in every frame; sent as its source, so it
 * uses nothing from outside itself.
 */
export function withoutPeerConnections(names: string[]): void {
  for (const name of names) {
    try {
      Object.defineProperty(globalThis, name, {
        value: undefined,
        writable: false,
        configurable: false
      });
    } catch {
      // Already taken out and locked: there is nothing left to take.
    }
  }
}

/**
 * A context this Playwright cannot confine. The runner's failure, not the
 * page's: no page is opened, and the check has no verdict on the build.
 */
export class Unconfined extends Error {}

/** What a refused socket's page is told: 1008, a policy refused it. */
const SOCKET_REFUSED = { code: 1008, reason: "not opened: the page is being checked" };

/**
 * Take from every page of `context` what the launch flags cannot: its peer
 * connections, and every WebSocket it constructs, each of which is ended
 * before anything is sent and named to `onSocket`. Throws when this
 * Playwright cannot do it: a page is not opened in a context that is not
 * confined.
 */
export async function confine(context: PwContext, onSocket: (url: string) => void): Promise<void> {
  if (typeof context.addInitScript !== "function") {
    throw new Unconfined(
      "this Playwright cannot run a script before a page's own: the page is not opened"
    );
  }
  if (typeof context.routeWebSocket !== "function") {
    throw new Unconfined(
      "this Playwright cannot refuse a page's WebSockets (it needs 1.48 or later): the page is not opened"
    );
  }
  await context.addInitScript(withoutPeerConnections, PEER_CONSTRUCTORS);
  await context.routeWebSocket(/.*/, async (socket) => {
    onSocket(socket.url());
    // A socket whose page has gone has no one left to tell.
    await socket.close(SOCKET_REFUSED).catch(() => undefined);
  });
}
