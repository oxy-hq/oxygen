/**
 * Which of a page's requests carry the checker, which go as the page sent
 * them, and which are never sent: the promote checks' read-only rule.
 */

import { describe, expect, it } from "vitest";
import {
  isReadyBeacon,
  type LoadsTarget,
  loadsTarget,
  privateHost,
  treat,
  treatHop,
  treatSocket,
  VIEWPORTS
} from "./loads-target.js";

const APP = "00000000-0000-0000-0000-0000000000a1";
const WS = "00000000-0000-0000-0000-0000000000b1";
const BASE = "https://app.example.com/customer-apps/acme/store/";

function target(overrides: Partial<Parameters<typeof loadsTarget>[0]> = {}): LoadsTarget {
  return loadsTarget({
    pageUrl: BASE,
    deployment: "https://app.example.com/",
    credential: { "X-API-Key": "oxysvc_x" },
    appId: APP,
    workspaceId: WS,
    environment: "staging",
    expectedBuild: "b1",
    ...overrides
  });
}

/** The names of the headers a request is signed with, or `undefined` when it is not. */
function signs(t: LoadsTarget, method: string, url: string): string[] | undefined {
  const treatment = treat(t, method, url);
  return treatment.kind === "sign" ? Object.keys(treatment.headers).sort() : undefined;
}

describe("loadsTarget", () => {
  it("opens the page the platform named, as the checker, on staging", () => {
    const t = target();
    expect(t.url).toBe(BASE);
    expect(t.origin).toBe("https://app.example.com");
    expect(t.basePath).toBe("/customer-apps/acme/store/");
    expect(t.headers).toEqual({ "x-api-key": "oxysvc_x", "x-oxy-app-env": "staging" });
    expect(t.dataHeaders).toEqual({ ...t.headers, "x-oxy-app": APP });
    expect(t.waitMs).toBe(30_000);
    expect(VIEWPORTS.map((v) => `${v.name} ${v.width}x${v.height}`)).toEqual([
      "desk 1440x900",
      "phone 390x844"
    ]);
  });

  it("names no environment on production, and signs exactly what it signs on staging", () => {
    const staging = target();
    const production = target({ environment: "production" });
    // Production is what the URL serves unnamed; each signed read asks for an uncached copy.
    expect(production.headers).toEqual({ "x-api-key": "oxysvc_x", "cache-control": "no-cache" });
    expect(production.dataHeaders).toEqual({ ...production.headers, "x-oxy-app": APP });
    expect(Object.keys(production.headers)).not.toContain("x-oxy-app-env");
    expect(production.environment).toBe("production");
    // The same requests are signed, passed and refused: the reach is staging's, on production.
    const requests: [string, string][] = [
      ["GET", BASE],
      ["GET", `${BASE}stores`],
      ["GET", `${BASE}assets/index.js`],
      ["POST", `${BASE}__oxy/beacon`],
      ["POST", `https://app.example.com/api/projects/${WS}/query`],
      ["POST", `https://app.example.com/api/projects/${WS}/semantic-query`],
      ["POST", `${BASE}fn/save`],
      ["PUT", `${BASE}stores`],
      ["DELETE", "https://app.example.com/api/orgs/1"],
      ["GET", "https://app.example.com/api/orgs"],
      ["POST", "https://app.example.com/api/projects/other/query"],
      ["GET", "https://fonts.example.com/a.woff2"],
      ["POST", "https://collector.example.com/track"],
      ["GET", "http://169.254.169.254/latest/meta-data"]
    ];
    for (const [method, url] of requests) {
      expect(treat(production, method, url).kind, `${method} ${url}`).toBe(
        treat(staging, method, url).kind
      );
    }
    expect(treat(production, "POST", `${BASE}fn/save`).kind).toBe("refuse");
    // On production as on staging, nothing of another origin is asked for.
    for (const env of [staging, production]) {
      expect(treat(env, "GET", "https://fonts.example.com/a.woff2").kind).toBe("refuse");
      expect(treat(env, "GET", "https://attacker.example/?d=rows").kind).toBe("refuse");
    }
  });

  it("refuses a page on another origin than the deployment the run targets", () => {
    expect(() =>
      target({ pageUrl: "https://elsewhere.example.net/customer-apps/acme/store/" })
    ).toThrow(
      /on https:\/\/elsewhere\.example\.net, but this run targets https:\/\/app\.example\.com/
    );
  });

  it("refuses a page with no base path to hold the credential to", () => {
    expect(() => target({ pageUrl: "https://app.example.com/" })).toThrow(/not an app's base path/);
    // Without the trailing slash, `…/store` would also be the base of `…/storefront/`.
    expect(() => target({ pageUrl: "https://app.example.com/customer-apps/acme/store" })).toThrow(
      /not an app's base path/
    );
    expect(() => target({ pageUrl: "nonsense" })).toThrow(/not a URL/);
  });
});

describe("treat", () => {
  it("signs reads under the app's base and its beacon", () => {
    const t = target();
    expect(signs(t, "GET", BASE)).toEqual(["x-api-key", "x-oxy-app-env"]);
    expect(signs(t, "HEAD", `${BASE}assets/index.js`)).toBeDefined();
    expect(signs(t, "POST", `${BASE}__oxy/beacon`)).toBeDefined();
  });

  it("passes, unsigned, a read on the app's origin that is not under the base or climbs out of it", () => {
    const t = target();
    for (const url of [
      "https://app.example.com/api/orgs",
      "https://app.example.com/customer-apps/acme/store/../../api/x",
      "https://app.example.com/customer-apps/acme/store/%2e%2e/%2e%2e/api/x",
      "https://app.example.com/customer-apps/acme/storefront/"
    ]) {
      expect(treat(t, "GET", url), url).toEqual({ kind: "pass" });
    }
    expect(treat(t, "OPTIONS", BASE)).toEqual({ kind: "pass" });
  });

  it("refuses every request to another origin, read or not, and says where it was going", () => {
    const t = target();
    const requests: [string, string][] = [
      // What a page that read the app's data would do with it.
      ["GET", "https://attacker.example/?d=rows"],
      ["HEAD", "https://attacker.example/pixel.gif"],
      ["OPTIONS", "https://attacker.example/collect"],
      ["POST", "https://sentry.example.com/api/1/envelope/"],
      // A font, a script: nothing of another origin is fetched, whatever it is for.
      ["GET", "https://fonts.example.com/customer-apps/acme/store/a.woff2"],
      ["GET", "https://cdn.example.com/lib.js"],
      // The app's own host on another port or scheme is another origin.
      ["GET", "https://app.example.com:8443/customer-apps/acme/store/"],
      ["GET", "http://app.example.com/customer-apps/acme/store/"],
      ["GET", "ftp://app.example.com/a"]
    ];
    for (const [method, url] of requests) {
      expect(treat(t, method, url), `${method} ${url}`).toEqual({
        kind: "refuse",
        why: "off-origin",
        what: `${method} ${url} (another origin)`
      });
    }
  });

  it("leaves to the browser what never leaves it", () => {
    const t = target();
    for (const url of [
      "data:image/png;base64,AAAA",
      "blob:https://app.example.com/0b0f6d5c-6a52-4a2e-9d0b-3d7c2a1e5f10",
      "blob:https://attacker.example/0b0f6d5c-6a52-4a2e-9d0b-3d7c2a1e5f10",
      "about:blank"
    ]) {
      expect(treat(t, "GET", url), url).toEqual({ kind: "local" });
    }
  });

  it("signs the app's own data reads, and names the app on them", () => {
    const t = target();
    const query = `https://app.example.com/api/projects/${WS}/query`;
    expect(signs(t, "POST", query)).toEqual(["x-api-key", "x-oxy-app", "x-oxy-app-env"]);
    expect(signs(t, "POST", `https://app.example.com/api/projects/${WS}/semantic-query`)).toEqual([
      "x-api-key",
      "x-oxy-app",
      "x-oxy-app-env"
    ]);
    // Another workspace's data, or another data route, is not the app's read.
    const other = "https://app.example.com/api/projects/00000000-0000-0000-0000-0000000000c1/query";
    expect(treat(t, "POST", other).kind).toBe("refuse");
    expect(treat(t, "POST", `https://app.example.com/api/projects/${WS}/agents/a/asks`)).toEqual({
      kind: "refuse",
      why: "write",
      what: `POST /api/projects/${WS}/agents/a/asks`
    });
  });

  // Unsigned, the bootstrap answered 401 on every screen of an app with the SDK's
  // shell: a warning per screen, and pictures without the app's chrome.
  it("signs the shell's bootstrap of the app's workspace, named the same way", () => {
    const t = target();
    const shell = `https://app.example.com/api/projects/${WS}/shell-context?app=${APP}`;
    expect(signs(t, "GET", shell)).toEqual(["x-api-key", "x-oxy-app", "x-oxy-app-env"]);
    // Only a read of it, and only the app's own workspace's.
    expect(treat(t, "PUT", `https://app.example.com/api/projects/${WS}/shell-context`).kind).toBe(
      "refuse"
    );
    const other =
      "https://app.example.com/api/projects/00000000-0000-0000-0000-0000000000c1/shell-context";
    expect(treat(t, "GET", other)).toEqual({ kind: "pass" });
  });

  it("signs no data read when the platform did not say which workspace is the app's", () => {
    const t = target({ workspaceId: undefined });
    expect(t.dataPaths).toEqual([]);
    expect(t.shellPath).toBeNull();
    expect(treat(t, "POST", `https://app.example.com/api/projects/${WS}/query`).kind).toBe(
      "refuse"
    );
  });

  it("refuses every write on the app's origin, and names it", () => {
    const t = target();
    expect(treat(t, "POST", `${BASE}fn/save`)).toEqual({
      kind: "refuse",
      why: "write",
      what: "POST /customer-apps/acme/store/fn/save"
    });
    expect(treat(t, "DELETE", BASE)).toMatchObject({ kind: "refuse", why: "write" });
    expect(treat(t, "PUT", "https://app.example.com/api/me/appearance")).toMatchObject({
      kind: "refuse",
      why: "write"
    });
  });

  it("refuses what it cannot place on the app's origin: something that is no URL", () => {
    const t = target();
    for (const method of ["GET", "POST"]) {
      expect(treat(t, method, "not a url")).toEqual({
        kind: "refuse",
        why: "off-origin",
        what: `${method} not a url (not a URL)`
      });
    }
  });

  it("refuses a request to a private address off the app's origin, read or not", () => {
    const t = target();
    for (const url of [
      "http://169.254.169.254/latest/meta-data/",
      "http://127.0.0.1:9000/",
      "http://10.0.0.5/",
      "http://[::1]/",
      "http://[fd00::1]/",
      "http://[::ffff:192.168.1.1]/",
      "http://oxy-serve:3000/api/orgs",
      "http://postgres.default.svc.cluster.local:5432/",
      "http://metadata.google.internal/",
      "http://printer.local/"
    ]) {
      // Its own refusal, with its own reason: it does not lean on the origin rule.
      expect(treat(t, "GET", url), url).toEqual({
        kind: "refuse",
        why: "off-origin",
        what: `GET ${url} (not a public address)`
      });
    }
  });

  it("still signs the app's own origin when the deployment is a private address", () => {
    const local = target({
      pageUrl: "http://127.0.0.1:4010/customer-apps/acme/store/",
      deployment: "http://127.0.0.1:4010"
    });
    expect(signs(local, "GET", "http://127.0.0.1:4010/customer-apps/acme/store/")).toBeDefined();
    expect(treat(local, "GET", "http://127.0.0.1:4010/api/orgs")).toEqual({ kind: "pass" });
  });
});

describe("treatHop", () => {
  it("lets a read's next hop go where the read itself could, and nowhere else", () => {
    const t = target();
    expect(treatHop(t, "GET", "https://app.example.com/login?next=x")).toEqual({ kind: "pass" });
    expect(treatHop(t, "HEAD", `${BASE}assets/index.js`).kind).toBe("sign");
    expect(treatHop(t, "GET", "http://169.254.169.254/latest/meta-data/")).toEqual({
      kind: "refuse",
      why: "off-origin",
      what: "GET http://169.254.169.254/latest/meta-data/ (not a public address)"
    });
    expect(treatHop(t, "GET", "http://127.0.0.1:9000/admin")).toMatchObject({
      kind: "refuse",
      why: "off-origin"
    });
    expect(treatHop(t, "GET", "https://attacker.example/?d=rows")).toEqual({
      kind: "refuse",
      why: "off-origin",
      what: "GET https://attacker.example/?d=rows (another origin)"
    });
  });

  it("refuses a hop that would repeat a write, even to where the write itself was allowed", () => {
    const t = target();
    // The browser follows a hop without asking, and the hop after it: a write is not sent on.
    const query = `https://app.example.com/api/projects/${WS}/query`;
    expect(treat(t, "POST", query).kind).toBe("sign");
    expect(treatHop(t, "POST", query)).toEqual({
      kind: "refuse",
      why: "write",
      what: `POST /api/projects/${WS}/query`
    });
    expect(treatHop(t, "POST", `${BASE}__oxy/beacon`)).toEqual({
      kind: "refuse",
      why: "write",
      what: "POST /customer-apps/acme/store/__oxy/beacon"
    });
    expect(treatHop(t, "OPTIONS", BASE)).toMatchObject({ kind: "refuse", why: "write" });
    expect(treatHop(t, "POST", "https://attacker.example/collect")).toMatchObject({
      kind: "refuse",
      why: "off-origin"
    });
  });
});

describe("treatSocket", () => {
  it("opens no socket: off the app's origin for where it goes, on it because the checker reads", () => {
    const t = target();
    expect(treatSocket(t, "wss://attacker.example/live?d=rows")).toEqual({
      kind: "refuse",
      why: "off-origin",
      what: "WS wss://attacker.example/live?d=rows (another origin)"
    });
    expect(treatSocket(t, "ws://127.0.0.1:9222/devtools/browser")).toEqual({
      kind: "refuse",
      why: "off-origin",
      what: "WS ws://127.0.0.1:9222/devtools/browser (not a public address)"
    });
    // `wss` on the app's host is the app's origin; `ws` there, or another port, is not.
    expect(treatSocket(t, "wss://app.example.com/api/live?x=1")).toEqual({
      kind: "refuse",
      why: "write",
      what: "WS /api/live"
    });
    expect(treatSocket(t, "wss://app.example.com:443/api/live").why).toBe("write");
    expect(treatSocket(t, "ws://app.example.com/api/live").why).toBe("off-origin");
    expect(treatSocket(t, "wss://app.example.com:8443/api/live").why).toBe("off-origin");
    expect(treatSocket(t, "not a url")).toEqual({
      kind: "refuse",
      why: "off-origin",
      what: "WS not a url (not a URL)"
    });
  });

  it("knows a plain-HTTP deployment's own sockets too", () => {
    const local = target({
      pageUrl: "http://localhost:3000/customer-apps/acme/store/",
      deployment: "http://localhost:3000"
    });
    expect(treatSocket(local, "ws://localhost:3000/api/live")).toEqual({
      kind: "refuse",
      why: "write",
      what: "WS /api/live"
    });
    expect(treatSocket(local, "ws://localhost:9229/")).toMatchObject({ why: "off-origin" });
  });
});

describe("privateHost", () => {
  it("reads loopback, private, link-local and shared ranges as not public", () => {
    for (const host of ["100.64.1.1", "172.16.0.1", "192.168.0.9", "0.0.0.0", "[fe80::1]"]) {
      expect(privateHost(host), host).toBe(true);
    }
    for (const host of [
      "8.8.8.8",
      "100.128.0.1",
      "172.32.0.1",
      "[2606:4700::1111]",
      "example.com"
    ]) {
      expect(privateHost(host), host).toBe(false);
    }
  });
});

describe("isReadyBeacon", () => {
  it("is ready only with its event", () => {
    const url = `${BASE}__oxy/beacon`;
    expect(isReadyBeacon(url, '{"v":1,"events":[{"n":"oxy-app-ready"}]}')).toBe(true);
    expect(isReadyBeacon(url, '{"events":[{"n":"view"}]}')).toBe(false);
    expect(isReadyBeacon(url, null)).toBe(false);
    expect(isReadyBeacon(`${BASE}fn/x`, "oxy-app-ready")).toBe(false);
  });
});
