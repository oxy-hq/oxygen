/**
 * The proxy's guardrails.
 *
 * These are the whole reason the command exists: everything else is a
 * forwarder. Each predicate is a decision carried over verbatim from the Rust
 * `oxy proxy` this replaced, and getting one wrong means a laptop writing to a
 * customer's production data — a failure nobody notices until it has already
 * happened.
 */

import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import { createServer, request, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { ExitCode } from "../util/errors.js";

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");

import {
  buildRequestHeaders,
  ensureTraceparent,
  functionRoute,
  isAuthPath,
  isEventsPath,
  isProductionTarget,
  isWritePath,
  rewriteSetCookie,
  traceIdOf
} from "./proxy.js";
import {
  admitsLocalRequest,
  forwardsWritesByDefault,
  isUnderApp,
  normalizeRequestPath,
  parseProxyApp
} from "./proxy-app.js";

/**
 * A port the OS says is free, rather than one derived from another port.
 *
 * The previous `45_000 + upstreamPort % 1000` was a guess: anything else on the
 * machine holding that port made the proxy fail to bind, and the readiness loop
 * below then spent its full budget before failing with a connection error that
 * named nothing. Binding zero and reading the number back leaves only the race
 * between close and re-bind, which is orders of magnitude narrower.
 */
async function freePort(): Promise<number> {
  const probe = createServer();
  await new Promise<void>((r) => probe.listen(0, "127.0.0.1", r));
  const { port } = probe.address() as AddressInfo;
  await new Promise<void>((r) => probe.close(() => r()));
  return port;
}

describe("isWritePath — held by default", () => {
  it("holds every mutating method", () => {
    for (const method of ["POST", "PUT", "PATCH", "DELETE"]) {
      expect(isWritePath(method, "/api/w/apps"), method).toBe(true);
    }
  });

  it("never holds a read", () => {
    expect(isWritePath("GET", "/api/w/apps")).toBe(false);
    expect(isWritePath("HEAD", "/api/w/apps")).toBe(false);
  });

  /**
   * The two POST-but-READ endpoints. They carry their filter in the body, so
   * they are POSTs that change nothing — holding them would break the one
   * thing a custom app in `pnpm dev` actually does.
   */
  it("lets the data-plane POSTs through", () => {
    expect(isWritePath("POST", "/api/projects/abc/query")).toBe(false);
    expect(isWritePath("POST", "/api/projects/abc/semantic-query")).toBe(false);
  });

  /** …but only those two, and only as a whole final segment. */
  it("still holds something that merely ends in a similar word", () => {
    expect(isWritePath("POST", "/api/w/apps/requery")).toBe(true);
    expect(isWritePath("POST", "/api/w/fn/run")).toBe(true);
  });

  it("is not fooled by a query string", () => {
    expect(isWritePath("POST", "/api/projects/abc/query?debug=1")).toBe(false);
    expect(isWritePath("POST", "/api/w/apps?x=1")).toBe(true);
  });

  it("leaves events and auth to their own handling", () => {
    expect(isWritePath("POST", "/api/customer-apps/abc/events")).toBe(false);
    expect(isWritePath("POST", "/api/auth/callback")).toBe(false);
  });
});

describe("isEventsPath — dropped by default", () => {
  it("matches the tracking endpoint", () => {
    expect(isEventsPath("/api/customer-apps/abc/events")).toBe(true);
    expect(isEventsPath("/api/customer-apps/abc/events?x=1")).toBe(true);
  });

  it("does not match a different customer-apps route", () => {
    expect(isEventsPath("/api/customer-apps/abc/manifest")).toBe(false);
    expect(isEventsPath("/api/w/events")).toBe(false);
  });
});

describe("isAuthPath — must reach the backend unauthenticated", () => {
  it("covers the auth tree and the user probe", () => {
    expect(isAuthPath("/api/auth/callback")).toBe(true);
    expect(isAuthPath("/api/auth/dev-login?email=x")).toBe(true);
    expect(isAuthPath("/api/user")).toBe(true);
  });

  /** `/api/users` is a different thing entirely. */
  it("matches /api/user exactly, not as a prefix", () => {
    expect(isAuthPath("/api/users")).toBe(false);
    expect(isAuthPath("/api/user/settings")).toBe(false);
  });
});

describe("buildRequestHeaders — the token is a fallback, never an override", () => {
  it("injects the dev bearer when the request carries no auth", () => {
    const h = buildRequestHeaders({}, "dev-token", "/api/w/apps");
    expect(h.authorization).toBe("Bearer dev-token");
  });

  /**
   * A real browser session must win. Overriding it would silently act as a
   * different user than the one signed in, which is the worst possible way to
   * be wrong about authorization.
   */
  it("leaves a browser cookie alone", () => {
    const h = buildRequestHeaders({ cookie: "session=abc" }, "dev-token", "/api/w/apps");
    expect(h.authorization).toBeUndefined();
    expect(h.cookie).toBe("session=abc");
  });

  it("leaves an explicit Authorization alone", () => {
    const h = buildRequestHeaders({ authorization: "Bearer theirs" }, "dev-token", "/api/w/apps");
    expect(h.authorization).toBe("Bearer theirs");
  });

  /** Injecting on an auth endpoint would break sign-in. */
  it("never injects on an auth endpoint", () => {
    expect(
      buildRequestHeaders({}, "dev-token", "/api/auth/callback").authorization
    ).toBeUndefined();
    expect(buildRequestHeaders({}, "dev-token", "/api/user").authorization).toBeUndefined();
  });

  /**
   * The backend derives its base URL from these, and an OAuth `redirect_uri`
   * that does not match what the provider issued the code for is a 401.
   */
  it("forwards Origin and Referer, which sign-in depends on", () => {
    const h = buildRequestHeaders(
      { origin: "http://localhost:5173", referer: "http://localhost:5173/app" },
      undefined,
      "/api/auth/callback"
    );
    expect(h.origin).toBe("http://localhost:5173");
    expect(h.referer).toBe("http://localhost:5173/app");
  });
});

describe("functionRoute — only an Oxy Function call is annotated", () => {
  it("names the function of a /customer-apps/<org>/<slug>/fn/<name> call", () => {
    expect(functionRoute("/customer-apps/acme/sales/fn/refresh")).toBe("refresh");
    expect(functionRoute("/customer-apps/o/s/fn/sync/extra")).toBe("sync");
    expect(functionRoute("/customer-apps/o/s/fn/refresh?debug=1")).toBe("refresh");
  });

  /** A bundle asset that merely contains `/fn/` is not a call. */
  it("leaves assets and other routes alone", () => {
    expect(functionRoute("/customer-apps/acme/sales/assets/app.js")).toBeUndefined();
    expect(functionRoute("/customer-apps/acme/sales/assets/fn/x.js")).toBeUndefined();
    expect(functionRoute("/customer-apps/acme/sales/fn")).toBeUndefined();
    expect(functionRoute("/api/fn/not-an-app")).toBeUndefined();
  });
});

describe("traceparent — one trace per function call", () => {
  /**
   * The SDK's header must survive the outbound allowlist, or the id the page
   * stamped on its error names a trace that exists nowhere.
   */
  it("forwards the SDK's traceparent and tracestate untouched", () => {
    const traceparent = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
    const headers = buildRequestHeaders(
      { traceparent, tracestate: "oxy=1" },
      undefined,
      "/customer-apps/o/s/fn/refresh"
    );
    expect(ensureTraceparent(headers)).toBe("0af7651916cd43dd8448eb211c80319c");
    expect(headers.traceparent).toBe(traceparent);
    expect(headers.tracestate).toBe("oxy=1");
  });

  it("mints a sampled one when the call arrived without", () => {
    const headers: Record<string, string> = {};
    const minted = ensureTraceparent(headers);
    expect(minted).toMatch(/^[0-9a-f]{32}$/);
    expect(traceIdOf(headers.traceparent ?? "")).toBe(minted);
    expect(headers.traceparent).toMatch(/-01$/);
  });

  /** A rejected header takes its `tracestate` with it (W3C). */
  it("replaces a malformed one rather than forwarding it, and drops its tracestate", () => {
    for (const bad of [
      "garbage",
      "00-00000000000000000000000000000000-b7ad6b7169203331-01",
      "00-0af7651916cd43dd8448eb211c80319c-0000000000000000-01",
      "zz-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
      "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331"
    ]) {
      const headers: Record<string, string> = { traceparent: bad, tracestate: "vendor=stale" };
      const minted = ensureTraceparent(headers);
      expect(bad.includes(minted), bad).toBe(false);
      expect(traceIdOf(headers.traceparent ?? ""), bad).toBe(minted);
      expect(headers.tracestate, bad).toBeUndefined();
    }
  });
});

describe("rewriteSetCookie — storable on localhost", () => {
  it("strips Domain and Secure, which stop a browser storing it on http://localhost", () => {
    const out = rewriteSetCookie("session=abc; Domain=.oxygen-hq.com; Secure; HttpOnly; Path=/");
    expect(out).not.toMatch(/domain=/i);
    expect(out).not.toMatch(/\bsecure\b/i);
    expect(out).toContain("session=abc");
    expect(out).toContain("HttpOnly");
    expect(out).toContain("Path=/");
  });

  /** Browsers only honour `SameSite=None` together with `Secure`, which just went. */
  it("relaxes SameSite=None, which cannot survive without Secure", () => {
    expect(rewriteSetCookie("s=1; SameSite=None; Secure")).toContain("SameSite=Lax");
  });

  it("leaves an already-storable cookie alone", () => {
    expect(rewriteSetCookie("s=1; Path=/; HttpOnly")).toBe("s=1; Path=/; HttpOnly");
  });
});

describe("the forwarder, end to end", () => {
  let upstream: Server;
  let base: string;
  let proxy: ChildProcess;
  let proxyUrl: string;
  let proxyStderr = "";
  let streamClosed = false;

  beforeAll(async () => {
    upstream = createServer((req, res) => {
      const path = (req.url ?? "/").split("?")[0];
      if (path === "/customer-apps/o/s/fn/refresh") {
        res.writeHead(200, { "content-type": "application/json", "x-oxy-request-id": "req-123" });
        res.end(JSON.stringify({ traceparent: req.headers.traceparent ?? null }));
        return;
      }
      if (path === "/api/broken-stream") {
        // Headers and a first chunk, then the connection dies — the status is
        // already on the wire when the forwarder finds out.
        res.writeHead(200, { "content-type": "text/plain" });
        res.write("partial");
        setTimeout(() => res.socket?.destroy(), 20);
        return;
      }
      if (path === "/api/stream") {
        // An SSE stream that never ends on its own. `close` fires only if the
        // connection goes away, which is the thing under test.
        res.writeHead(200, { "content-type": "text/event-stream" });
        const tick = setInterval(() => res.write("data: tick\n\n"), 25);
        res.on("close", () => {
          clearInterval(tick);
          streamClosed = true;
        });
        return;
      }
      if (path === "/api/gzipped") {
        // Compressed on the wire. undici asks for this and decodes it, so the
        // forwarder must not pass `content-encoding` on to the browser.
        const body = gzipSync(Buffer.from(JSON.stringify({ big: "x".repeat(2000) })));
        res.writeHead(200, { "content-type": "application/json", "content-encoding": "gzip" });
        res.end(body);
        return;
      }
      if (path === "/api/cookie") {
        res.writeHead(200, {
          "content-type": "application/json",
          "set-cookie": ["a=1; Domain=.oxygen-hq.com; Secure; Path=/", "b=2; Path=/"]
        });
        res.end("{}");
        return;
      }
      res.writeHead(200, { "content-type": "application/json" });
      res.end(
        JSON.stringify({
          method: req.method,
          path: req.url,
          auth: req.headers.authorization ?? null
        })
      );
    });
    await new Promise<void>((r) => upstream.listen(0, "127.0.0.1", r));
    base = `http://127.0.0.1:${(upstream.address() as AddressInfo).port}`;

    const port = await freePort();
    proxyUrl = `http://127.0.0.1:${port}`;
    proxy = spawn(process.execPath, [BIN, "proxy", "--target", base, "--port", String(port)], {
      env: { ...process.env, OXY_TOKEN: "dev-token", NO_COLOR: "1" },
      stdio: ["ignore", "ignore", "pipe"]
    });
    proxy.stderr?.on("data", (d) => {
      proxyStderr += d;
    });

    // Wait for the listener rather than sleeping a fixed amount.
    let ready = false;
    for (let i = 0; i < 100; i++) {
      try {
        await fetch(`${proxyUrl}/api/ping`);
        ready = true;
        break;
      } catch {
        await new Promise((r) => setTimeout(r, 100));
      }
    }
    // FAIL HERE, NAMING WHY. Without this every case below fails on its own
    // fetch with a bare connection error, and the actual reason — the proxy
    // exited, or could not bind — is in a stream nobody reads.
    if (!ready) {
      throw new Error(
        `the proxy never came up on ${proxyUrl}` +
          (proxyStderr ? `\n--- its stderr ---\n${proxyStderr}` : " (it printed nothing)")
      );
    }
  }, 30_000);

  afterAll(() => {
    proxy?.kill();
    upstream.close();
  });

  /**
   * The case the command exists for. node's fetch decompresses transparently,
   * so forwarding `content-encoding: gzip` on the decoded bytes is
   * `ERR_CONTENT_DECODING_FAILED` in the browser.
   */
  it("does not label a decompressed body as still compressed", async () => {
    const res = await fetch(`${proxyUrl}/api/gzipped`);
    expect(res.headers.get("content-encoding")).toBeNull();
    // …and the body really is readable, which is the point.
    expect(((await res.json()) as { big: string }).big).toHaveLength(2000);
  });

  /**
   * `@oxy-hq/sdk` matches `^403:` as its catch-all and renders "Access denied
   * — check the oxy server logs", which would discard the explanation this
   * response carries. 409 has no branch there.
   */
  it("answers a held write 409, not 403, so the reason survives the SDK", async () => {
    const res = await fetch(`${proxyUrl}/api/w/apps`, { method: "POST", body: "{}" });
    expect(res.status).toBe(409);
    expect(((await res.json()) as { message: string }).message).toMatch(/--allow-writes/);
  });

  it("forwards a read, injecting the dev bearer", async () => {
    const body = (await (await fetch(`${proxyUrl}/api/ping`)).json()) as { auth: string };
    expect(body.auth).toBe("Bearer dev-token");
  });

  it("lets the POST-but-read data-plane endpoints through", async () => {
    const res = await fetch(`${proxyUrl}/api/projects/x/query`, { method: "POST", body: "{}" });
    expect(res.status).toBe(200);
  });

  it("drops a tracking event with a 204 rather than an error", async () => {
    const res = await fetch(`${proxyUrl}/api/customer-apps/x/events`, {
      method: "POST",
      body: "{}"
    });
    expect(res.status).toBe(204);
  });

  /** `Headers.forEach` comma-joins Set-Cookie; a login setting two would lose one. */
  it("keeps multiple Set-Cookie headers separate, and rewrites each", async () => {
    const res = await fetch(`${proxyUrl}/api/cookie`);
    const cookies = res.headers.getSetCookie();
    expect(cookies).toHaveLength(2);
    expect(cookies.join(" ")).not.toMatch(/domain=/i);
    expect(cookies.join(" ")).not.toMatch(/\bsecure\b/i);
  });

  /**
   * The line a developer copies into HyperDX after watching a function fail:
   * the trace the proxy sent upstream, and the request id the server answered
   * with, so neither has to be dug out by hand.
   */
  it("gives a function call a trace and prints its ids", async () => {
    const res = await fetch(`${proxyUrl}/customer-apps/o/s/fn/refresh`);
    const sent = ((await res.json()) as { traceparent: string | null }).traceparent;
    const traceId = traceIdOf(sent ?? "");
    expect(traceId).toMatch(/^[0-9a-f]{32}$/);
    for (let i = 0; i < 20 && !proxyStderr.includes(`trace_id=${traceId}`); i++) {
      await new Promise((r) => setTimeout(r, 50));
    }
    expect(proxyStderr).toContain(`↳ 200 fn refresh  request_id=req-123  trace_id=${traceId}`);
  });

  /**
   * Last-but-one, and on purpose: before the fix this took the proxy process
   * down, which would fail every case after it for a reason none of them name.
   */
  it("survives an upstream that dies after the status is sent", async () => {
    await fetch(`${proxyUrl}/api/broken-stream`, { signal: AbortSignal.timeout(5_000) })
      .then((r) => r.text())
      .catch(() => undefined);
    expect(proxy.exitCode, proxyStderr).toBeNull();
    const res = await fetch(`${proxyUrl}/api/ping`, { signal: AbortSignal.timeout(5_000) });
    expect(res.status).toBe(200);
  });

  /** An SSE stream nobody reads must not stay open against the cloud. */
  it("closes the upstream request when the browser goes away", async () => {
    const browser = new AbortController();
    const res = await fetch(`${proxyUrl}/api/stream`, { signal: browser.signal });
    await res.body?.getReader().read();
    browser.abort();
    for (let i = 0; i < 30 && !streamClosed; i++) {
      await new Promise((r) => setTimeout(r, 100));
    }
    expect(streamClosed).toBe(true);
  });
});

describe("isProductionTarget — refused without --yes", () => {
  it("catches the product host and every org subdomain under it", () => {
    for (const target of [
      "https://app.oxygen-hq.com",
      "https://acme.oxygen-hq.com",
      "https://poke-house.oxygen-hq.com",
      "https://acme--app.customer-apps.oxygen-hq.com"
    ]) {
      expect(isProductionTarget(target), target).toBe(true);
    }
  });

  /**
   * THE CASE A SUFFIX TEST LOSES. `"oxygen-hq.com".endsWith(".oxygen-hq.com")`
   * is false, so a guard written as the suffix alone waves the apex through —
   * which the regex this predicate replaced did catch. Pinned separately from
   * the subdomains above because it is the one host the obvious rewrite drops.
   */
  it("catches the apex, which a bare suffix test does not", () => {
    expect(isProductionTarget("https://oxygen-hq.com")).toBe(true);
    expect(isProductionTarget("https://oxygen-hq.com/some/path")).toBe(true);
    expect(isProductionTarget("https://OXYGEN-HQ.COM")).toBe(true);
  });

  it("leaves dev, staging, loopback and a self-hosted host alone", () => {
    for (const target of [
      "https://aip.dev.oxy.tech",
      "https://aip.staging.oxy.tech",
      "https://poke-house.staging.oxy.tech",
      "http://localhost:3000",
      "http://127.0.0.1:5173",
      "https://oxy.acme.internal:8443"
    ]) {
      expect(isProductionTarget(target), target).toBe(false);
    }
  });

  /**
   * A host that merely ENDS in the same letters is not under the zone —
   * `notoxygen-hq.com` would match a naive `endsWith("oxygen-hq.com")`.
   */
  it("is not fooled by a lookalike host", () => {
    expect(isProductionTarget("https://notoxygen-hq.com")).toBe(false);
    expect(isProductionTarget("https://oxygen-hq.com.evil.test")).toBe(false);
  });

  /** "I could not tell" is not a reason to allow an accident. */
  it("treats an unparseable target as production", () => {
    expect(isProductionTarget("not a url")).toBe(true);
  });
});

describe("the production refusal, through the binary", () => {
  /**
   * A bare `oxyc proxy` defaults to production, so it must refuse.
   *
   * The port is arbitrary and never bound — the refusal happens before the
   * listener is opened, which is the property this pins.
   */
  it("refuses without --yes and names the target", () => {
    const r = spawnSync(process.execPath, [BIN, "proxy", "--port", "45999"], {
      encoding: "utf8",
      env: { ...process.env, OXY_TOKEN: "t", NO_COLOR: "1" },
      timeout: 15_000
    });
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(r.stderr).toMatch(/production target/);
    expect(r.stderr).toMatch(/--yes/);
  });
});

const SANDBOX_TOKEN = "oxy_sbx_0123456789abcdefghijABCDEFGHIJ012345";
const DEV_APP = { org: "acme", app: "store", env: "dev-a1" };

describe("parseProxyApp — usage errors before anything is bound", () => {
  const usage = (flags: { app?: string; appEnv?: string }, sandbox = false) => {
    try {
      parseProxyApp(flags, sandbox);
    } catch (e) {
      return e as { code: number; message: string; hint?: string };
    }
    return undefined;
  };

  it("names no app when neither flag is given", () => {
    expect(parseProxyApp({}, false)).toBeUndefined();
  });

  it("takes an org/app and a sandbox or staging", () => {
    expect(parseProxyApp({ app: "acme/store", appEnv: "dev-a1" }, false)).toEqual(DEV_APP);
    expect(parseProxyApp({ app: "acme/store", appEnv: "staging" }, false)).toEqual({
      org: "acme",
      app: "store",
      env: "staging"
    });
    expect(parseProxyApp({ app: "acme/store" }, false)).toEqual({ org: "acme", app: "store" });
  });

  it("refuses --app-env without --app", () => {
    const e = usage({ appEnv: "dev-a1" });
    expect(e?.code).toBe(ExitCode.USAGE);
    expect(e?.message).toMatch(/--app-env needs --app/);
  });

  it("refuses --app-env production, which reads already do", () => {
    const e = usage({ app: "acme/store", appEnv: "production" });
    expect(e?.code).toBe(ExitCode.USAGE);
    expect(e?.hint).toMatch(/omit --app-env/);
  });

  it("refuses a malformed environment and a malformed app", () => {
    expect(usage({ app: "acme/store", appEnv: "dev-NOPE!" })?.code).toBe(ExitCode.USAGE);
    for (const app of ["acme", "acme/", "/store", "a/b/c", "acme/st ore", "acme/st%2fore"]) {
      expect(usage({ app }, false)?.code, app).toBe(ExitCode.USAGE);
    }
  });

  it("holds a sandbox agent token to a sandbox of the named app", () => {
    for (const flags of [{}, { app: "acme/store" }, { app: "acme/store", appEnv: "staging" }]) {
      const e = usage(flags, true);
      expect(e?.code, JSON.stringify(flags)).toBe(ExitCode.USAGE);
    }
    expect(usage({}, true)?.message).toMatch(/--app-env dev-<handle>/);
    expect(parseProxyApp({ app: "acme/store", appEnv: "dev-a1" }, true)).toEqual(DEV_APP);
  });
});

describe("isUnderApp — the app's own requests, whole-segment", () => {
  it("matches the base, with and without the slash, a query, and any case", () => {
    for (const path of [
      "/customer-apps/acme/store",
      "/customer-apps/acme/store/",
      "/customer-apps/acme/store?x=1",
      "/customer-apps/acme/store/assets/a.js?v=2",
      "/customer-apps/acme/store/fn/refresh",
      "/customer-apps/acme/store/__oxy/beacon",
      "/customer-apps/Acme/STORE/"
    ]) {
      expect(isUnderApp(path, DEV_APP), path).toBe(true);
    }
  });

  it("leaves everything else alone", () => {
    for (const path of [
      "/",
      "/api/user",
      "/api/customer-apps/acme/store/events",
      "/customer-apps/acme/store-2/",
      "/customer-apps/acme/stor",
      "/customer-apps/acme",
      "/customer-apps/other/store/",
      "/x/customer-apps/acme/store/"
    ]) {
      expect(isUnderApp(path, DEV_APP), path).toBe(false);
    }
  });
});

describe("buildRequestHeaders — the app's environment", () => {
  const incoming = {
    cookie: "session=browser",
    accept: "text/html",
    origin: "http://127.0.0.1:3000",
    referer: "http://127.0.0.1:3000/customer-apps/acme/store/"
  };

  it("adds X-Oxy-App-Env and drops the cookie under the app base, and the token takes over", () => {
    const h = buildRequestHeaders(incoming, "tok", "/customer-apps/acme/store/", DEV_APP);
    expect(h["x-oxy-app-env"]).toBe("dev-a1");
    expect(h.cookie).toBeUndefined();
    // The loopback page's origin would fail the server's origin gate at a cloud host.
    expect(h.origin).toBeUndefined();
    expect(h.referer).toBeUndefined();
    expect(h.authorization).toBe("Bearer tok");
    expect(h.accept).toBe("text/html");
  });

  it("does the same for a function call and the beacon", () => {
    for (const path of [
      "/customer-apps/acme/store/fn/refresh",
      "/customer-apps/acme/store/__oxy/beacon"
    ]) {
      const h = buildRequestHeaders(incoming, "tok", path, DEV_APP);
      expect(h["x-oxy-app-env"], path).toBe("dev-a1");
      expect(h.cookie, path).toBeUndefined();
    }
  });

  it("leaves an explicit Authorization where it is", () => {
    const h = buildRequestHeaders(
      { ...incoming, authorization: "Bearer theirs" },
      "tok",
      "/customer-apps/acme/store/",
      DEV_APP
    );
    expect(h.authorization).toBe("Bearer theirs");
    expect(h["x-oxy-app-env"]).toBe("dev-a1");
  });

  it("touches no other path: no header, the cookie stays, no token over it", () => {
    for (const path of ["/api/user", "/api/projects/p/query", "/customer-apps/acme/other/"]) {
      const h = buildRequestHeaders(incoming, "tok", path, DEV_APP);
      expect(h["x-oxy-app-env"], path).toBeUndefined();
      expect(h.cookie, path).toBe("session=browser");
      expect(h.authorization, path).toBeUndefined();
    }
  });

  it("adds nothing without an environment, or without an app", () => {
    const base = "/customer-apps/acme/store/";
    const noEnv = buildRequestHeaders(incoming, "tok", base, { org: "acme", app: "store" });
    expect(noEnv["x-oxy-app-env"]).toBeUndefined();
    expect(noEnv.cookie).toBe("session=browser");
    expect(buildRequestHeaders(incoming, "tok", base)["x-oxy-app-env"]).toBeUndefined();
  });
});

describe("normalizeRequestPath — decide on the path the upstream will name", () => {
  it("collapses dot segments, encoded ones too, and keeps the query", () => {
    expect(normalizeRequestPath("/customer-apps/acme/store/../../../api/w/apps")).toBe(
      "/api/w/apps"
    );
    expect(normalizeRequestPath("/customer-apps/acme/store/%2e%2e/%2E%2E/%2e%2e/api/x?y=1")).toBe(
      "/api/x?y=1"
    );
    expect(normalizeRequestPath("/customer-apps/acme/store/./fn/save")).toBe(
      "/customer-apps/acme/store/fn/save"
    );
  });

  it("leaves an ordinary path alone, and a leading // stays a path", () => {
    expect(normalizeRequestPath("/customer-apps/acme/store/")).toBe("/customer-apps/acme/store/");
    expect(normalizeRequestPath("//evil.example/x")).toBe("//evil.example/x");
  });

  it("no longer reads an escaping path as the app's own", () => {
    const path = normalizeRequestPath("/customer-apps/acme/store/../../../api/w/apps");
    expect(isUnderApp(path, DEV_APP)).toBe(false);
    expect(forwardsWritesByDefault(DEV_APP, path)).toBe(false);
  });
});

describe("admitsLocalRequest — the token rides only for a page on this machine", () => {
  const local = { host: "127.0.0.1:3000" };

  it("admits a loopback page, and a client that sends no origin at all", () => {
    expect(admitsLocalRequest({ ...local, origin: "http://127.0.0.1:3000" })).toBe(true);
    expect(admitsLocalRequest({ host: "localhost:5173", origin: "http://localhost:5173" })).toBe(
      true
    );
    expect(admitsLocalRequest({ host: "[::1]:3000", referer: "http://[::1]:3000/x" })).toBe(true);
    expect(admitsLocalRequest(local)).toBe(true);
    expect(admitsLocalRequest({ ...local, "sec-fetch-site": "same-origin" })).toBe(true);
    expect(admitsLocalRequest({ ...local, "sec-fetch-site": "none" })).toBe(true);
  });

  it("refuses another site's page, whatever it sends", () => {
    expect(admitsLocalRequest({ ...local, origin: "https://evil.example" })).toBe(false);
    expect(admitsLocalRequest({ ...local, origin: "null" })).toBe(false);
    // No Origin, no Referer (a no-referrer page): the browser still names the site.
    expect(admitsLocalRequest({ ...local, "sec-fetch-site": "cross-site" })).toBe(false);
    expect(
      admitsLocalRequest({
        ...local,
        origin: "http://127.0.0.1:3000",
        "sec-fetch-site": "cross-site"
      })
    ).toBe(false);
    expect(admitsLocalRequest({ ...local, referer: "https://evil.example/page" })).toBe(false);
    // A loopback Referer does not rescue a foreign Origin.
    expect(
      admitsLocalRequest({
        ...local,
        origin: "https://evil.example",
        referer: "http://127.0.0.1:3000/"
      })
    ).toBe(false);
  });

  it("refuses a request that arrived under another name: DNS rebinding", () => {
    expect(admitsLocalRequest({ host: "evil.example:3000" })).toBe(false);
    expect(
      admitsLocalRequest({ host: "evil.example:3000", origin: "http://evil.example:3000" })
    ).toBe(false);
    expect(admitsLocalRequest({})).toBe(false);
  });
});

describe("forwardsWritesByDefault — a sandbox's own requests only", () => {
  const under = "/customer-apps/acme/store/fn/save";

  it("forwards in a dev sandbox, under the app base", () => {
    expect(forwardsWritesByDefault(DEV_APP, under)).toBe(true);
  });

  it("holds staging, production and no app", () => {
    expect(forwardsWritesByDefault({ ...DEV_APP, env: "staging" }, under)).toBe(false);
    expect(forwardsWritesByDefault({ org: "acme", app: "store" }, under)).toBe(false);
    expect(forwardsWritesByDefault(undefined, under)).toBe(false);
  });

  it("holds a write outside the base, which carries no environment", () => {
    expect(forwardsWritesByDefault(DEV_APP, "/api/customer-apps/x/environments")).toBe(false);
  });
});

/** One proxy process against `target`: resolves once it listens, with what it has printed. */
async function startProxy(
  target: string,
  args: string[],
  env: Record<string, string>
): Promise<{ url: string; child: ChildProcess; stderr: () => string }> {
  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;
  let stderr = "";
  const child = spawn(
    process.execPath,
    [BIN, "proxy", "--target", target, "--port", String(port), ...args],
    { env: { ...process.env, NO_COLOR: "1", ...env }, stdio: ["ignore", "ignore", "pipe"] }
  );
  child.stderr?.on("data", (d) => {
    stderr += d;
  });
  for (let i = 0; i < 100; i++) {
    try {
      await fetch(`${url}/api/ping`);
      return { url, child, stderr: () => stderr };
    } catch {
      await new Promise((r) => setTimeout(r, 100));
    }
  }
  child.kill();
  throw new Error(`the proxy never came up on ${url}\n--- its stderr ---\n${stderr}`);
}

describe("the app-scoped proxy, through the binary", () => {
  let upstream: Server;
  let base: string;
  const proxies: ChildProcess[] = [];
  /** What the upstream saw, in order. */
  const seen: { method: string; path: string; headers: Record<string, string | undefined> }[] = [];

  beforeAll(async () => {
    upstream = createServer((req, res) => {
      seen.push({
        method: req.method ?? "GET",
        path: req.url ?? "/",
        headers: {
          cookie: req.headers.cookie,
          authorization: req.headers.authorization,
          "x-oxy-app-env": req.headers["x-oxy-app-env"] as string | undefined
        }
      });
      res.writeHead(200, { "content-type": "application/json" });
      res.end("{}");
    });
    await new Promise<void>((r) => upstream.listen(0, "127.0.0.1", r));
    base = `http://127.0.0.1:${(upstream.address() as AddressInfo).port}`;
  });

  afterAll(() => {
    for (const p of proxies) p.kill();
    upstream.close();
  });

  const lastSeen = () => seen[seen.length - 1];

  it("sends the app's requests into the sandbox and says what to open", async () => {
    const p = await startProxy(base, ["--app", "acme/store", "--app-env", "dev-a1"], {
      OXY_TOKEN: "dev-token"
    });
    proxies.push(p.child);
    const cookie = { cookie: "session=browser" };

    await fetch(`${p.url}/customer-apps/acme/store/`, { headers: cookie });
    expect(lastSeen()?.headers).toEqual({
      cookie: undefined,
      authorization: "Bearer dev-token",
      "x-oxy-app-env": "dev-a1"
    });

    await fetch(`${p.url}/api/user`, { headers: cookie });
    expect(lastSeen()?.headers["x-oxy-app-env"]).toBeUndefined();
    expect(lastSeen()?.headers.cookie).toBe("session=browser");

    await fetch(`${p.url}/customer-apps/acme/other/`);
    expect(lastSeen()?.headers["x-oxy-app-env"]).toBeUndefined();

    expect(p.stderr()).toContain(
      `open ${p.url}/customer-apps/acme/store/ — dev-a1 of acme/store on ${base}`
    );
  }, 30_000);

  it("forwards a sandbox's own writes by default and still holds the rest", async () => {
    const p = await startProxy(base, ["--app", "acme/store", "--app-env", "dev-a1"], {
      OXY_TOKEN: "dev-token"
    });
    proxies.push(p.child);
    const before = seen.length;

    const own = await fetch(`${p.url}/customer-apps/acme/store/fn/save`, { method: "POST" });
    expect(own.status).toBe(200);
    expect(lastSeen()).toMatchObject({ method: "POST", path: "/customer-apps/acme/store/fn/save" });

    const elsewhere = await fetch(`${p.url}/api/w/apps`, { method: "POST" });
    expect(elsewhere.status).toBe(409);
    expect(seen.length).toBe(before + 1);
  }, 30_000);

  it("holds a write whose dot segments escape the app base, as the upstream would see it", async () => {
    const p = await startProxy(base, ["--app", "acme/store", "--app-env", "dev-a1"], {
      OXY_TOKEN: "dev-token"
    });
    proxies.push(p.child);
    const before = seen.length;
    // fetch() would collapse the dots before sending, so speak raw HTTP.
    const status = await new Promise<number>((done, fail) => {
      const req = request(
        `${p.url}/customer-apps/acme/store/../../../api/w/apps`.replace(/\/customer-apps.*/, ""),
        { method: "POST", path: "/customer-apps/acme/store/../../../api/w/apps" },
        (res) => {
          res.resume();
          done(res.statusCode ?? 0);
        }
      );
      req.on("error", fail);
      req.end();
    });
    expect(status).toBe(409);
    expect(seen.length).toBe(before);
  }, 30_000);

  it("refuses another site's page under the app base before the token is added", async () => {
    const p = await startProxy(base, ["--app", "acme/store", "--app-env", "dev-a1"], {
      OXY_TOKEN: "dev-token"
    });
    proxies.push(p.child);
    const before = seen.length;

    const forged = await fetch(`${p.url}/customer-apps/acme/store/fn/save`, {
      method: "POST",
      headers: { origin: "https://evil.example" }
    });
    expect(forged.status).toBe(403);
    expect(await forged.json()).toMatchObject({ error: "origin_not_local" });
    const page = await fetch(`${p.url}/customer-apps/acme/store/`, {
      headers: { referer: "https://evil.example/" }
    });
    expect(page.status).toBe(403);
    expect(seen.length).toBe(before);

    const local = await fetch(`${p.url}/customer-apps/acme/store/fn/save`, {
      method: "POST",
      headers: { origin: p.url }
    });
    expect(local.status).toBe(200);
  }, 30_000);

  it("holds staging's writes, and --allow-writes forwards them", async () => {
    const held = await startProxy(base, ["--app", "acme/store", "--app-env", "staging"], {
      OXY_TOKEN: "dev-token"
    });
    proxies.push(held.child);
    const before = seen.length;
    const res = await fetch(`${held.url}/customer-apps/acme/store/fn/save`, { method: "POST" });
    expect(res.status).toBe(409);
    expect(seen.length).toBe(before);

    const open = await startProxy(
      base,
      ["--app", "acme/store", "--app-env", "staging", "--allow-writes"],
      { OXY_TOKEN: "dev-token" }
    );
    proxies.push(open.child);
    await fetch(`${open.url}/customer-apps/acme/store/fn/save`, { method: "POST" });
    expect(lastSeen()).toMatchObject({ method: "POST" });
    expect(lastSeen()?.headers["x-oxy-app-env"]).toBe("staging");
  }, 30_000);

  it("keeps dropping events, in a sandbox too", async () => {
    const p = await startProxy(base, ["--app", "acme/store", "--app-env", "dev-a1"], {
      OXY_TOKEN: "dev-token"
    });
    proxies.push(p.child);
    const before = seen.length;
    const res = await fetch(`${p.url}/api/customer-apps/abc/events`, { method: "POST" });
    expect(res.status).toBe(204);
    expect(seen.length).toBe(before);
  }, 30_000);

  /** Refused before the listener opens; `--yes` is irrelevant to the local target. */
  const refused = (args: string[], token = "t") =>
    spawnSync(process.execPath, [BIN, "proxy", "--target", base, "--port", "45998", ...args], {
      encoding: "utf8",
      env: { ...process.env, OXY_TOKEN: token, NO_COLOR: "1" },
      timeout: 15_000
    });

  it("exits 2 on the usage errors", () => {
    for (const args of [
      ["--app-env", "dev-a1"],
      ["--app", "acme/store", "--app-env", "production"],
      ["--app", "acme/store", "--app-env", "dev-NOPE!"],
      ["--app", "acme"]
    ]) {
      const r = refused(args);
      expect(r.status, args.join(" ")).toBe(ExitCode.USAGE);
    }
  });

  it("exits 2 for a sandbox agent token that names no sandbox, and says how to", () => {
    for (const args of [[], ["--app", "acme/store"]]) {
      const r = refused(args, SANDBOX_TOKEN);
      expect(r.status, args.join(" ")).toBe(ExitCode.USAGE);
      expect(r.stderr).toMatch(/--app <org>\/<app> --app-env dev-<handle>/);
    }
  });

  const againstProduction = (args: string[], timeout = 15_000) =>
    spawnSync(process.execPath, [BIN, "proxy", "--port", "45997", "--app", "acme/store", ...args], {
      encoding: "utf8",
      env: { ...process.env, OXY_TOKEN: "t", NO_COLOR: "1" },
      timeout
    });

  it("still refuses a production target without --yes for staging, or a sandbox with --allow-writes", () => {
    for (const args of [
      ["--app-env", "staging"],
      ["--app-env", "dev-a1", "--allow-writes"]
    ]) {
      const r = againstProduction(args);
      expect(r.status, args.join(" ")).toBe(ExitCode.REFUSED);
      expect(r.stderr).toMatch(/production target/);
    }
  });

  it("opens into a sandbox on a production target without --yes: its writes stay in the sandbox", () => {
    // It listens until killed, so the timeout ends it; the banner proves it got past the refusal.
    const r = againstProduction(["--app-env", "dev-a1"], 4_000);
    expect(r.stderr).not.toMatch(/production target/);
    expect(r.stderr).toMatch(/listening on http:\/\/127\.0\.0\.1:45997/);
  });
});
