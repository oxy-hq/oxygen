// @vitest-environment jsdom

/**
 * Execution tests for the two browser scripts the server injects into every
 * custom app — `sw.js` and `runtime.js`, which live in
 * `crates/api-custom-apps/src/client/` and reach the browser via
 * `include_str!`.
 *
 * ## Why they are tested from here
 *
 * They are browser code, and this repo has exactly one JavaScript test runner.
 * The Rust side can assert *structure* — that a handler is registered, that
 * `strip_comments` did not eat a brace — and it did, and those assertions all
 * passed while both scripts carried logic bugs that quietly undid the feature
 * they exist for: the worker deleted the precache it had just built, and the
 * runtime reported every session's engagement twice. Neither is visible without
 * running the code, so the code is run here.
 *
 * The scripts are read off disk rather than imported: they are not modules, they
 * are self-installing scripts that expect a `ServiceWorkerGlobalScope` or a
 * `window`. Each test evaluates one in a sandbox with just enough of that global
 * to reach the branch under test.
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const CLIENT_DIR = join(__dirname, "../../../crates/api-custom-apps/src/client");
const readScript = (name: string) => readFileSync(join(CLIENT_DIR, name), "utf8");

// ── sw.js harness ───────────────────────────────────────────────────────────

const SCOPE = "/customer-apps/acme/sales/";
const ORIGIN = "https://app.example.com";

/** Minimal Cache API over a Map, enough for open/keys/delete/put/match. */
function fakeCaches(initial: Record<string, string[] | Record<string, unknown>> = {}) {
  const store = new Map<string, Map<string, unknown>>();
  for (const [name, seed] of Object.entries(initial)) {
    store.set(
      name,
      new Map(
        Array.isArray(seed)
          ? seed.map((u) => [u, { ok: true }] as [string, unknown])
          : Object.entries(seed)
      )
    );
  }
  return {
    store,
    async open(name: string) {
      if (!store.has(name)) store.set(name, new Map());
      const entries = store.get(name) as Map<string, unknown>;
      const keyOf = (key: unknown) =>
        typeof key === "string" ? key : String((key as Request).url ?? key);
      return {
        async put(key: unknown, value: unknown) {
          entries.set(keyOf(key), value);
        },
        async match(key: unknown) {
          return entries.get(keyOf(key));
        },
        async keys() {
          return [...entries.keys()];
        },
        async delete(key: unknown) {
          return entries.delete(keyOf(key));
        }
      };
    },
    async keys() {
      return [...store.keys()];
    },
    async has(name: string) {
      return store.has(name);
    },
    async delete(name: string) {
      return store.delete(name);
    },
    async match() {
      return undefined;
    }
  };
}

/**
 * Evaluate `sw.js` against a fresh global — the point of the exercise, since a
 * worker's global scope is torn down whenever it goes idle and every event may
 * run on a brand-new one.
 */
function bootWorker(caches: ReturnType<typeof fakeCaches>, fetchImpl: typeof fetch) {
  const listeners: Record<string, (e: unknown) => void> = {};
  const navPreloadEnable = vi.fn(async () => undefined);
  const self = {
    // A real `ServiceWorkerGlobalScope.location` is a `WorkerLocation` and has
    // `origin`. Omitting it here made the fetch handler bail at its
    // same-origin guard (`undefined !== "https://…"`), so the navigation test
    // below passed without ever entering the code it names.
    location: {
      href: `${ORIGIN}${SCOPE}__oxy/sw.js?base=${encodeURIComponent(SCOPE)}`,
      origin: ORIGIN
    },
    addEventListener: (type: string, fn: (e: unknown) => void) => {
      listeners[type] = fn;
    },
    clients: { claim: async () => undefined },
    skipWaiting: () => undefined,
    // The navigationPreload registration surface. `enable` is a spy so a test
    // can prove the worker opts into the browser feature — without which
    // `event.preloadResponse` is always undefined and the fast path is dead.
    registration: {
      scope: `${ORIGIN}${SCOPE}`,
      navigationPreload: { enable: navPreloadEnable }
    }
  };
  // eslint-disable-next-line @typescript-eslint/no-implied-eval
  const install = new Function("self", "caches", "fetch", readScript("sw.js"));
  install(self, caches, fetchImpl);
  return { listeners, navPreloadEnable };
}

/** Fire one lifecycle event and await everything it passed to `waitUntil`. */
async function fire(listener: (e: unknown) => void, extra: Record<string, unknown> = {}) {
  const pending: Promise<unknown>[] = [];
  listener({ waitUntil: (p: Promise<unknown>) => pending.push(p), ...extra });
  await Promise.all(pending);
}

const PRECACHE = (build: string) => `oxy-precache::${SCOPE}::${build}`;
const META = `oxy-meta::${SCOPE}`;
const LIVE_BUILD_URL = `${ORIGIN}${SCOPE}__oxy/live-build`;

/** Read back whatever the worker recorded, for assertions. */
async function readMarker(caches: ReturnType<typeof fakeCaches>) {
  const entry = caches.store.get(META)?.get(LIVE_BUILD_URL) as { text(): Promise<string> };
  return entry ? (await entry.text()).trim() : null;
}

/** The durable marker `precacheBuild` writes, as the worker will read it back. */
const marker = (build: string) => ({
  [LIVE_BUILD_URL]: {
    async text() {
      return build;
    }
  }
});

describe("sw.js — precache generation survives a worker restart", () => {
  /**
   * The regression: `currentBuild` is module-level, so it is `null` on every
   * cold start. `dropStalePrecaches` computed `keep = null`, which matched every
   * generation — including the live one — so the first load after an activation
   * ran with an empty precache. An updated worker waits for the last tab to
   * close, which is exactly long enough to guarantee the terminated-global case.
   */
  it("activate keeps the live precache when the global was torn down", async () => {
    const caches = fakeCaches({
      [PRECACHE("build-live")]: [`${SCOPE}assets/app.js`],
      [META]: marker("build-live")
    });
    // No manifest fetch: this simulates activate running on a restarted global,
    // where nothing has re-populated `currentBuild` in memory.
    const { listeners } = bootWorker(
      caches,
      vi.fn(async () => ({ ok: false })) as unknown as typeof fetch
    );

    await fire(listeners.activate);

    expect([...caches.store.keys()]).toContain(PRECACHE("build-live"));
    expect(caches.store.get(PRECACHE("build-live"))?.size).toBe(1);
  });

  /**
   * Superseded generations must still be swept — and the *right* one has to
   * survive.
   *
   * `build-old` is seeded first on purpose. Two generations is the normal state
   * for the whole waiting period (install precaches the new build; the old one
   * lives until activate), and `CacheStorage.keys()` yields creation order, so
   * anything that infers the live build from "the first name" reliably picks the
   * superseded one. An earlier fix did, and an earlier version of this test
   * asserted only `toHaveLength(1)` — which passed while the wrong cache
   * survived. Assert the name.
   */
  it("activate drops the superseded generation, not the new one", async () => {
    const caches = fakeCaches({
      [PRECACHE("build-old")]: [`${SCOPE}assets/old.js`],
      [PRECACHE("build-live")]: [`${SCOPE}assets/app.js`],
      [META]: marker("build-live")
    });
    const { listeners } = bootWorker(
      caches,
      vi.fn(async () => ({ ok: false })) as unknown as typeof fetch
    );

    await fire(listeners.activate);

    const remaining = [...caches.store.keys()].filter((n) => n.startsWith("oxy-precache::"));
    expect(remaining).toEqual([PRECACHE("build-live")]);
  });

  /**
   * A client that precached before the marker existed: two generations and
   * nothing authoritative to choose between them. Guessing is a coin flip on the
   * visitor's next load, so the sweep must decline entirely.
   */
  it("activate sweeps nothing when the live generation is unknowable", async () => {
    const caches = fakeCaches({
      [PRECACHE("build-old")]: [`${SCOPE}assets/old.js`],
      [PRECACHE("build-live")]: [`${SCOPE}assets/app.js`]
    });
    const { listeners } = bootWorker(
      caches,
      vi.fn(async () => ({ ok: false })) as unknown as typeof fetch
    );

    await fire(listeners.activate);

    const remaining = [...caches.store.keys()].filter((n) => n.startsWith("oxy-precache::"));
    expect(remaining).toHaveLength(2);
  });

  /** One generation is unambiguous whatever wrote it — marker or not. */
  it("activate trusts a single generation with no marker", async () => {
    const caches = fakeCaches({
      [PRECACHE("build-only")]: [`${SCOPE}assets/app.js`]
    });
    const { listeners } = bootWorker(
      caches,
      vi.fn(async () => ({ ok: false })) as unknown as typeof fetch
    );

    await fire(listeners.activate);

    expect([...caches.store.keys()]).toContain(PRECACHE("build-only"));
  });

  /**
   * `precacheBuild` swallows each asset fetch failure individually (one 404 must
   * not empty the whole precache), so "manifest fetched, every entry failed"
   * produces an empty cache. Marking that live would let a later fresh-global
   * activate trust it and sweep a *populated* generation in its favour —
   * strictly worse than the pre-marker behaviour, which declined.
   */
  it("does not mark an empty precache live when every entry failed", async () => {
    const caches = fakeCaches({
      [PRECACHE("build-old")]: [`${SCOPE}assets/old.js`]
    });
    const fetchImpl = vi.fn(async (input: unknown) => {
      if (String(input).includes("asset-manifest.json")) {
        return {
          ok: true,
          async json() {
            return {
              schemaVersion: 1,
              buildId: "build-new",
              entries: [{ path: "assets/new.js", kind: "module" }],
              assets: []
            };
          }
        };
      }
      // Every entry asset fails — the case the guard exists for.
      throw new Error("network down");
    });
    await fire(bootWorker(caches, fetchImpl as unknown as typeof fetch).listeners.install);
    expect(await caches.has(META)).toBe(false);

    // Activate on a FRESH global — the whole point. Within one live worker
    // `precacheBuild` has already set `currentBuild` in memory, and trusting
    // that is correct: a worker that just precached a build knows which
    // generation is its own, empty or not. The hazard is only a *later* global
    // that has to reconstruct the answer, and the marker is its only input.
    const { listeners: restarted } = bootWorker(caches, fetchImpl as unknown as typeof fetch);
    await fire(restarted.activate);

    const remaining = [...caches.store.keys()].filter((n) => n.startsWith("oxy-precache::"));
    expect(remaining).toContain(PRECACHE("build-old"));
    expect(remaining).toHaveLength(2);
  });

  /**
   * The distinction the guard has to draw: a bundle whose entry HTML inlines
   * everything has nothing to precache, and an empty cache is the correct and
   * complete result — that one must still be marked, or such an app can never
   * sweep an old generation. (The seeded starter app is exactly this shape.)
   */
  it("marks a build with no entries at all, because empty is the right answer", async () => {
    const caches = fakeCaches({});
    const fetchImpl = vi.fn(async (input: unknown) => {
      if (String(input).includes("asset-manifest.json")) {
        return {
          ok: true,
          async json() {
            return { schemaVersion: 1, buildId: "build-inline", entries: [], assets: [] };
          }
        };
      }
      throw new Error("nothing else should be fetched");
    });
    const { listeners } = bootWorker(caches, fetchImpl as unknown as typeof fetch);

    await fire(listeners.install);

    expect(await caches.has(META)).toBe(true);
    expect(await readMarker(caches)).toBe("build-inline");
  });

  /**
   * The second half of the same bug: with a cold `currentBuild`, every
   * navigation saw `served !== currentBuild` and re-ran `syncBuild` — a
   * `no-store` manifest fetch plus a re-fetch of every entry asset, in the
   * background, on most navigations.
   */
  it("a navigation for the build already held does not re-fetch the manifest", async () => {
    const caches = fakeCaches({
      [PRECACHE("build-live")]: [`${SCOPE}assets/app.js`],
      [META]: marker("build-live")
    });
    const fetchImpl = vi.fn(async (input: unknown) => {
      const url = String(input);
      if (url.includes("asset-manifest.json")) {
        return {
          ok: true,
          async json() {
            return { schemaVersion: 1, buildId: "build-live", entries: [], assets: [] };
          }
        };
      }
      return {
        ok: true,
        type: "basic",
        redirected: false,
        headers: new Headers({ "content-type": "text/html", "x-oxy-build": "build-live" }),
        clone() {
          return this;
        }
      };
    });
    const { listeners } = bootWorker(caches, fetchImpl as unknown as typeof fetch);

    const request = new Request(`${ORIGIN}${SCOPE}`, { method: "GET" });
    Object.defineProperty(request, "mode", { value: "navigate" });

    const pending: Promise<unknown>[] = [];
    let answered: Promise<unknown> | undefined;
    listeners.fetch({
      request,
      respondWith: (p: Promise<unknown>) => {
        answered = p;
      },
      waitUntil: (p: Promise<unknown>) => pending.push(p)
    });
    await answered;
    await Promise.all(pending);

    // The handler ran at all — without this the assertion below is satisfied by
    // a fetch listener that returned early and fetched nothing.
    expect(answered).toBeDefined();
    expect(fetchImpl.mock.calls.length).toBeGreaterThan(0);

    const manifestFetches = fetchImpl.mock.calls.filter((c) =>
      String(c[0]).includes("asset-manifest.json")
    );
    expect(manifestFetches).toHaveLength(0);
  });
});

describe("sw.js — navigationPreload overlaps worker boot with the fetch", () => {
  /**
   * The worker must opt into navigation preload, or `event.preloadResponse` is
   * always undefined and the whole point (the browser starting the navigation
   * fetch while the worker boots) never happens.
   */
  it("activate enables navigation preload where the browser supports it", async () => {
    const caches = fakeCaches({ [PRECACHE("b")]: [`${SCOPE}assets/app.js`], [META]: marker("b") });
    const { listeners, navPreloadEnable } = bootWorker(
      caches,
      vi.fn(async () => ({ ok: false })) as unknown as typeof fetch
    );
    await fire(listeners.activate);
    expect(navPreloadEnable).toHaveBeenCalledTimes(1);
  });

  /**
   * When the browser hands us a preloaded navigation response, the worker must
   * use it and NOT issue its own `fetch` — otherwise the request is made twice
   * and the preload bought nothing.
   */
  it("a navigation uses the preloaded response instead of fetching again", async () => {
    const caches = fakeCaches({
      [PRECACHE("build-live")]: [`${SCOPE}assets/app.js`],
      [META]: marker("build-live")
    });
    const fetchImpl = vi.fn(async () => {
      throw new Error("the worker fetched despite a preloaded response");
    });
    const { listeners } = bootWorker(caches, fetchImpl as unknown as typeof fetch);

    const request = new Request(`${ORIGIN}${SCOPE}`, { method: "GET" });
    Object.defineProperty(request, "mode", { value: "navigate" });

    // The browser's parallel navigation fetch, already resolved to the shell,
    // stamped with the build we already hold so no background sync fires.
    const preloaded = {
      ok: true,
      type: "basic",
      redirected: false,
      headers: new Headers({ "content-type": "text/html", "x-oxy-build": "build-live" }),
      clone() {
        return this;
      }
    };

    let answered: Promise<unknown> | undefined;
    const pending: Promise<unknown>[] = [];
    listeners.fetch({
      request,
      preloadResponse: Promise.resolve(preloaded),
      respondWith: (p: Promise<unknown>) => {
        answered = p;
      },
      waitUntil: (p: Promise<unknown>) => pending.push(p)
    });

    const served = await answered;
    await Promise.all(pending);

    expect(served).toBe(preloaded);
    expect(fetchImpl).not.toHaveBeenCalled();
  });
});

// ── runtime.js harness ──────────────────────────────────────────────────────

type Fetch = (url: string, init: RequestInit) => Promise<unknown>;

/** Evaluate `runtime.js` against a jsdom window with the globals it reads. */
function bootRuntime(
  app: Record<string, unknown> = {},
  fetchImpl: Fetch = async () => ({ ok: true })
) {
  const sent: { n: string; p: Record<string, unknown> }[] = [];
  (window as unknown as Record<string, unknown>).__OXY_APP__ = {
    basePath: SCOPE,
    serviceWorker: false,
    analytics: true,
    ...app
  };
  Object.defineProperty(navigator, "sendBeacon", {
    configurable: true,
    value: (_url: string, blob: Blob) => {
      // jsdom Blob has no sync reader; the runtime always stringifies first.
      const text = (blob as unknown as { __text?: string }).__text ?? "";
      if (text) sent.push(...JSON.parse(text).events);
      return true;
    }
  });
  // Capture the serialized body without depending on Blob internals.
  const RealBlob = window.Blob;
  vi.stubGlobal(
    "Blob",
    class extends RealBlob {
      __text: string;
      constructor(parts: string[], options?: BlobPropertyBag) {
        super(parts, options);
        this.__text = parts.join("");
      }
    }
  );
  vi.stubGlobal("fetch", vi.fn(fetchImpl));

  // eslint-disable-next-line @typescript-eslint/no-implied-eval
  new Function(readScript("runtime.js"))();
  return sent;
}

describe("runtime.js — engagement is reported once per screen", () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  /**
   * The regression: an ordinary desktop navigation away fires
   * `visibilitychange → hidden` AND then `pagehide`, and `finalize()` is wired
   * to both. `emitVitals` is guarded by `vitalsSent` and `emitErrors` empties
   * its map, but `emitEngagement` was neither — so every session's engagement
   * total was roughly doubled.
   */
  it("hidden followed by pagehide emits one oxy-engagement, not two", async () => {
    const sent = bootRuntime();

    // Engagement is only recorded past a 1s floor (sub-second is router churn).
    await new Promise((r) => setTimeout(r, 1100));

    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => "hidden"
    });
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new Event("pagehide"));

    const engagements = sent.filter((e) => e.n === "oxy-engagement");
    expect(engagements).toHaveLength(1);
    expect(engagements[0].p.path).toBe("/");
  });
});

describe("runtime.js — vitals carry the screen they landed on and the build", () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });
  afterEach(() => {
    window.history.replaceState({}, "", "/");
  });

  /** A navigation entry, so the runtime has numbers to send: jsdom records none. */
  const withNavigationTiming = () =>
    vi
      .spyOn(performance, "getEntriesByType")
      .mockImplementation((type: string) =>
        type === "navigation"
          ? ([
              { responseStart: 120, domContentLoadedEventEnd: 480 }
            ] as unknown as PerformanceEntryList)
          : []
      );

  const hide = () => {
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => "hidden"
    });
    document.dispatchEvent(new Event("visibilitychange"));
  };

  it("sends the landing screen, not the one the operator moved to, and the build", () => {
    const spy = withNavigationTiming();
    window.history.replaceState({}, "", `${SCOPE}orders?id=42#top`);
    const sent = bootRuntime({ buildId: "6e5e84a2-17f0-4879-8367-19211bd6a51a" });
    window.history.pushState({}, "", `${SCOPE}x`);
    hide();
    const vitals = sent.filter((e) => e.n === "oxy-web-vitals");
    expect(vitals).toHaveLength(1);
    expect(vitals[0].p.path).toBe("/orders");
    expect(vitals[0].p.build).toBe("6e5e84a2-17f0-4879-8367-19211bd6a51a");
    expect(vitals[0].p.ttfb).toBe(120);
    spy.mockRestore();
  });

  it("sends nothing with analytics off", () => {
    const spy = withNavigationTiming();
    const sent = bootRuntime({ analytics: false, buildId: "b" });
    hide();
    expect(sent.filter((e) => e.n === "oxy-web-vitals")).toHaveLength(0);
    spy.mockRestore();
  });
});

// ── same-tab navigation ─────────────────────────────────────────────────────

type Row = { n: string; p: Record<string, unknown> };

/** A beacon whose `fetch` answers only when the test says so, so the order of
 *  batches against the open's response can be asserted. */
function heldBeacon() {
  const batches: Row[][] = [];
  const answers: (() => void)[] = [];
  const fetchImpl: Fetch = (_url, init) => {
    batches.push(JSON.parse(init.body as string).events);
    return new Promise((resolve) => answers.push(() => resolve({ ok: true })));
  };
  const answerAll = async () => {
    for (const answer of answers.splice(0)) answer();
    // The runtime's `.then` runs a microtask after the answer.
    await Promise.resolve();
    await Promise.resolve();
  };
  return { batches, fetchImpl, answerAll };
}

/** The navigation entry the browser reports for this document. */
const navigationEntry = (entry: Record<string, unknown>) =>
  vi
    .spyOn(performance, "getEntriesByType")
    .mockImplementation((type: string) =>
      type === "navigation" ? ([entry] as unknown as PerformanceEntryList) : []
    );

/** The app says it has mounted (`window.__oxyAppReady`): jsdom lays nothing
 *  out, so the runtime's own check never fires here. */
const appMounted = () => (window as unknown as { __oxyAppReady?: () => void }).__oxyAppReady?.();

const setPrerendering = (value: boolean) =>
  Object.defineProperty(document, "prerendering", { configurable: true, get: () => value });

describe("runtime.js — a page shown from a prefetch reports its own open", () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });
  afterEach(() => {
    vi.restoreAllMocks();
    Reflect.deleteProperty(document, "prerendering");
  });

  it("a page the server served reports nothing", () => {
    navigationEntry({ deliveryType: "", activationStart: 0 });
    const beacon = heldBeacon();
    bootRuntime({}, beacon.fetchImpl);
    expect(beacon.batches).toHaveLength(0);
  });

  it("a prefetched page posts oxy-view first, and holds the next batch for its cookie", async () => {
    navigationEntry({ deliveryType: "navigational-prefetch", activationStart: 0 });
    Object.defineProperty(document, "referrer", {
      configurable: true,
      get: () => "https://app.example.com/"
    });
    const beacon = heldBeacon();
    bootRuntime({ buildId: "b" }, beacon.fetchImpl);
    expect(beacon.batches).toEqual([
      [expect.objectContaining({ n: "oxy-view", p: { referrer: "https://app.example.com/" } })]
    ]);

    // The app mounts before the open's response is back: its ready signal
    // must not go out without the session cookie that response sets.
    appMounted();
    expect(beacon.batches).toHaveLength(1);

    await beacon.answerAll();
    expect(beacon.batches).toHaveLength(2);
    expect(beacon.batches[1].map((e) => e.n)).toEqual(["oxy-app-ready"]);
    Reflect.deleteProperty(document, "referrer");
  });

  it("a prerendered page sends nothing until shown, then the open leads its queue", () => {
    setPrerendering(true);
    navigationEntry({ deliveryType: "", activationStart: 250 });
    const beacon = heldBeacon();
    bootRuntime({ buildId: "b" }, beacon.fetchImpl);
    appMounted();
    expect(beacon.batches).toHaveLength(0);

    setPrerendering(false);
    document.dispatchEvent(new Event("prerenderingchange"));
    expect(beacon.batches).toHaveLength(1);
    expect(beacon.batches[0].map((e) => e.n)).toEqual(["oxy-view", "oxy-app-ready"]);
  });
});

describe("runtime.js — a push before the person has touched the page", () => {
  // Each boot wraps whatever `history` holds, so the earlier tests' runtimes
  // would decide too. Every boot here starts from the browser's own methods.
  const browserPush = history.pushState;
  const browserReplace = history.replaceState;
  const activation = { hasBeenActive: false };

  beforeEach(() => {
    vi.unstubAllGlobals();
    history.pushState = browserPush;
    history.replaceState = browserReplace;
    history.replaceState(null, "", SCOPE);
    activation.hasBeenActive = false;
    Object.defineProperty(navigator, "userActivation", { configurable: true, value: activation });
  });
  afterEach(() => {
    history.pushState = browserPush;
    history.replaceState = browserReplace;
    Reflect.deleteProperty(navigator, "userActivation");
    Reflect.deleteProperty(navigator, "userAgentData");
    document.head.querySelector('meta[name="oxy-history"]')?.remove();
  });

  /** Did a push to `path` add a history entry? */
  const addsAnEntry = (path: string) => {
    const before = history.length;
    history.pushState(null, "", `${SCOPE}${path}`);
    expect(window.location.pathname).toBe(`${SCOPE}${path}`);
    return history.length === before + 1;
  };

  it("is made a replace, and the person's own push is not", () => {
    bootRuntime();
    expect(addsAnEntry("dashboard")).toBe(false);
    activation.hasBeenActive = true;
    expect(addsAnEntry("orders")).toBe(true);
  });

  /// Chrome skips such an entry on Back only until the page is activated, and
  /// by the time anyone presses Back they have clicked something; kept, the
  /// boot redirect's bare URL would bounce Back forward again there too.
  it("is made a replace in Chromium too", () => {
    Object.defineProperty(navigator, "userAgentData", {
      configurable: true,
      value: { brands: [{ brand: "Chromium", version: "141" }] }
    });
    bootRuntime();
    expect(addsAnEntry("dashboard")).toBe(false);
  });

  it("is left alone in an app that asks for the browser's own history", () => {
    const meta = document.createElement("meta");
    meta.name = "oxy-history";
    meta.content = "native";
    document.head.appendChild(meta);
    bootRuntime();
    expect(addsAnEntry("welcome")).toBe(true);
  });
});
