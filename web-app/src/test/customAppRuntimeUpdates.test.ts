// @vitest-environment jsdom

/**
 * Execution tests for the update agent in `runtime.js` — the part of the
 * platform-injected client runtime that notices a new build in a long-open
 * custom-app tab and, in `auto` mode only, reloads onto it at a safe moment.
 *
 * Kept apart from `customAppClientScripts.test.ts` because these need fake
 * timers, a controllable `__oxy/build` answer and a fake `location.reload`,
 * and because each boot here must not leak its listeners into the next.
 *
 * The script is evaluated with `location` and `sessionStorage` passed in as
 * parameters: jsdom's `window.location` cannot be redefined, and the loop-guard
 * tests need a storage that throws.
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const RUNTIME = readFileSync(
  join(__dirname, "../../../crates/api-custom-apps/src/client/runtime.js"),
  "utf8"
);

const BASE = "/customer-apps/acme/store-ops/";
const BUILD_A = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const BUILD_B = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const APP_ID = "11111111-1111-4111-8111-111111111111";
/** The loop guard's sessionStorage key: one per app, since apps share an origin. */
const GUARD = `oxy-update:${APP_ID}`;
const MIN = 60_000;

/** `runtime.js` as the server ships it in `off` mode: the update agent cut out
 *  between its markers (`oxy-api-custom-apps` `client.rs::without_update_agent`). */
const RUNTIME_OFF = RUNTIME.replace(
  /^\s*\/\/ @oxy-updates:begin$[\s\S]*?^\s*\/\/ @oxy-updates:end$/m,
  ""
);

type Live = string | "404" | "redirect" | "junk" | "network";

interface Booted {
  reload: ReturnType<typeof vi.fn>;
  probes: () => RequestInit[];
  rows: () => { n: string; p: Record<string, unknown> }[];
  setLive: (live: Live) => void;
  /** The person uses the page (a click, a key): pushes from now on are theirs. */
  act: () => void;
  setVisibility: (state: "visible" | "hidden") => void;
}

interface BootOptions {
  mode?: string;
  /** The build this page loaded. */
  build?: string;
  live?: Live;
  storage?: Storage;
  source?: string;
  analytics?: boolean;
  /** Has the person used the page yet (`navigator.userActivation`)? Default
   *  yes: the pushes these tests make stand for navigations the person made.
   *  Before that, the runtime turns a push into a replace (a boot-time URL
   *  rewrite), which is never a safe moment to reload. */
  acted?: boolean;
}

type Listener = [EventTarget, string, EventListenerOrEventListenerObject, unknown];
let listeners: Listener[] = [];
const originalPush = history.pushState;
const originalReplace = history.replaceState;

function recordListeners(target: EventTarget) {
  const original = target.addEventListener.bind(target);
  vi.spyOn(target, "addEventListener").mockImplementation((type, fn, opts) => {
    if (fn) listeners.push([target, type, fn, opts]);
    original(type, fn, opts);
  });
}

function boot(options: BootOptions = {}): Booted {
  let live: Live = options.live ?? BUILD_A;
  let visibility: "visible" | "hidden" = "visible";
  const probes: RequestInit[] = [];
  const rows: { n: string; p: Record<string, unknown> }[] = [];

  (window as unknown as Record<string, unknown>).__OXY_APP__ = {
    appId: APP_ID,
    basePath: BASE,
    serviceWorker: false,
    analytics: options.analytics ?? false,
    buildId: options.build ?? BUILD_A,
    updates: options.mode ?? "auto"
  };
  Object.defineProperty(navigator, "sendBeacon", { configurable: true, value: undefined });
  const activation = { hasBeenActive: options.acted ?? true };
  Object.defineProperty(navigator, "userActivation", { configurable: true, value: activation });
  Object.defineProperty(document, "visibilityState", {
    configurable: true,
    get: () => visibility
  });

  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init: RequestInit = {}) => {
      if (url === `${BASE}__oxy/beacon`) {
        rows.push(...JSON.parse(init.body as string).events);
        return { ok: true };
      }
      if (url !== `${BASE}__oxy/build`) throw new Error(`unexpected fetch ${url}`);
      probes.push(init);
      if (live === "network") throw new TypeError("Failed to fetch");
      if (live === "redirect") return { type: "opaqueredirect", status: 0 };
      if (live === "404") return { type: "basic", status: 404, json: async () => ({}) };
      if (live === "junk")
        return { type: "basic", status: 200, json: async () => ({ buildId: "x" }) };
      return { type: "basic", status: 200, json: async () => ({ buildId: live }) };
    })
  );

  const reload = vi.fn();
  const fakeLocation = {
    get pathname() {
      return window.location.pathname;
    },
    reload
  };
  // eslint-disable-next-line @typescript-eslint/no-implied-eval
  new Function("location", "sessionStorage", options.source ?? RUNTIME)(
    fakeLocation,
    options.storage ?? window.sessionStorage
  );

  return {
    reload,
    probes: () => probes,
    rows: () => {
      // Drain the beacon queue so rows pushed since the last flush are visible.
      window.dispatchEvent(new Event("pagehide"));
      return rows;
    },
    setLive: (next) => {
      live = next;
    },
    act: () => {
      activation.hasBeenActive = true;
    },
    setVisibility: (state) => {
      visibility = state;
      document.dispatchEvent(new Event("visibilitychange"));
    }
  };
}

/** Let one poll fire and its promise chain settle. `Math.random` is pinned to
 *  0.5 in `beforeEach`, so the ±20% jitter lands every poll at exactly 5 min and
 *  the detection time — and so the 60 s floor — is deterministic. */
async function poll() {
  await vi.advanceTimersByTimeAsync(5 * MIN);
}

function updateRows(rows: { n: string; p: Record<string, unknown> }[]) {
  return rows.filter((r) => r.n === "oxy-update").map((r) => r.p);
}

/** Keep the page "in use" so the idle path cannot fire before the one under test. */
function focusAnInput() {
  document.body.innerHTML = '<input id="f" />';
  const input = document.getElementById("f") as HTMLInputElement;
  input.focus();
  return input;
}

/** Remove every listener added so far: the document they belonged to is gone. */
function unloadPage() {
  for (const [target, type, fn, opts] of listeners) {
    target.removeEventListener(type, fn, opts as EventListenerOptions);
  }
  listeners = [];
  history.pushState = originalPush;
  history.replaceState = originalReplace;
}

/** An app with unsaved work: its `beforeunload` asks "leave this page?", and the
 *  fake `location.reload` runs it the way the browser does. */
function appAsksBeforeLeaving(page: Booted) {
  window.addEventListener("beforeunload", (e) => e.preventDefault());
  page.reload.mockImplementation(() => {
    window.dispatchEvent(new Event("beforeunload", { cancelable: true }));
  });
}

beforeEach(() => {
  // Every listener a test or a boot adds is removed afterwards, so one boot's
  // agent (or a test's cancelling listener) cannot act in the next test.
  recordListeners(window);
  recordListeners(document);
  vi.useFakeTimers();
  vi.spyOn(Math, "random").mockReturnValue(0.5);
  vi.setSystemTime(new Date("2026-10-16T08:00:00Z"));
  window.history.replaceState(null, "", `${BASE}`);
  window.sessionStorage.clear();
});

afterEach(() => {
  unloadPage();
  document.body.innerHTML = "";
  delete (window as unknown as Record<string, unknown>).__oxyUpdate;
  Reflect.deleteProperty(navigator, "userActivation");
  vi.clearAllTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("runtime.js updates — detect and announce", () => {
  it("polls __oxy/build uncached and announces a new build once per target", async () => {
    const page = boot({ mode: "observe" });
    const announced: CustomEvent[] = [];
    window.addEventListener("oxy:update-available", (e) => announced.push(e as CustomEvent));

    await poll();
    expect(page.probes()).toHaveLength(1);
    expect(page.probes()[0]).toMatchObject({
      cache: "no-store",
      credentials: "same-origin",
      redirect: "manual"
    });
    expect(announced).toHaveLength(0);

    page.setLive(BUILD_B);
    await poll();
    await poll();
    expect(announced).toHaveLength(1);
    expect(announced[0].detail).toEqual({ from: BUILD_A, to: BUILD_B });
    expect(announced[0].cancelable).toBe(true);
    expect(updateRows(page.rows())).toEqual([
      expect.objectContaining({ phase: "detected", how: "poll", from: BUILD_A, to: BUILD_B })
    ]);
  });

  it("treats a 404, junk, or a network error as unknown, never as a new build", async () => {
    const page = boot({ mode: "observe" });
    const announced = vi.fn();
    window.addEventListener("oxy:update-available", announced);
    for (const live of ["404", "junk", "network"] as const) {
      page.setLive(live);
      await poll();
    }
    expect(page.probes().length).toBeGreaterThanOrEqual(3);
    expect(announced).not.toHaveBeenCalled();
  });

  it("reads a redirect as an expired session, not a build", async () => {
    const page = boot({ mode: "auto", live: "redirect" });
    const expired = vi.fn();
    const announced = vi.fn();
    window.addEventListener("oxy:session-expired", expired);
    window.addEventListener("oxy:update-available", announced);
    await poll();
    await poll();
    expect(expired).toHaveBeenCalledTimes(1);
    expect(announced).not.toHaveBeenCalled();
    expect(page.reload).not.toHaveBeenCalled();
  });

  it("probes on resume and does not poll while hidden", async () => {
    const page = boot({ mode: "observe" });
    page.setVisibility("hidden");
    await poll();
    expect(page.probes()).toHaveLength(0);
    page.setVisibility("visible");
    await vi.advanceTimersByTimeAsync(0);
    expect(page.probes()).toHaveLength(1);
  });

  it("does nothing at all in off mode", async () => {
    const page = boot({ mode: "off", live: BUILD_B });
    await poll();
    expect(page.probes()).toHaveLength(0);
    expect((window as unknown as Record<string, unknown>).__oxyUpdate).toBeUndefined();
  });
});

describe("runtime.js updates — observe never reloads", () => {
  it("never reloads on route, idle or resume in observe mode", async () => {
    // No focused input: the idle path is open, and still nothing reloads.
    const page = boot({ mode: "observe", live: BUILD_B });
    await poll();
    await vi.advanceTimersByTimeAsync(2 * MIN);
    history.pushState(null, "", `${BASE}orders`);
    await vi.advanceTimersByTimeAsync(3 * MIN);
    page.setVisibility("hidden");
    await vi.advanceTimersByTimeAsync(11 * MIN);
    page.setVisibility("visible");
    await vi.advanceTimersByTimeAsync(3 * MIN);
    expect(page.reload).not.toHaveBeenCalled();
  });
});

describe("runtime.js — a push before the person has used the page", () => {
  /// An app rewriting its URL as it boots must leave no Back entry: from the
  /// home page or the dock it opens in the same tab, and Back has to leave it.
  it("is made a replace, and is not a safe moment to reload", async () => {
    focusAnInput();
    const page = boot({ live: BUILD_B, acted: false });
    await poll();
    await vi.advanceTimersByTimeAsync(61_000);
    const entries = history.length;
    history.pushState(null, "", `${BASE}orders?month=2026-10`);
    expect(window.location.pathname).toBe(`${BASE}orders`);
    expect(history.length).toBe(entries);
    expect(page.reload).not.toHaveBeenCalled();

    page.act();
    history.pushState(null, "", `${BASE}checklist`);
    expect(history.length).toBe(entries + 1);
    expect(page.reload).toHaveBeenCalledTimes(1);
  });
});

describe("runtime.js updates — auto applies at a safe moment", () => {
  it("waits 60 s after detection, then reloads on a path-changing push", async () => {
    focusAnInput();
    const page = boot({ live: BUILD_B });
    await poll();
    history.pushState(null, "", `${BASE}orders`);
    expect(page.reload).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(61_000);
    history.replaceState(null, "", `${BASE}orders?tab=2`);
    history.replaceState(null, "", `${BASE}elsewhere`);
    expect(page.reload).not.toHaveBeenCalled();
    history.pushState(null, "", `${BASE}elsewhere`);
    expect(page.reload).not.toHaveBeenCalled();

    history.pushState(null, "", `${BASE}checklist`);
    expect(page.reload).toHaveBeenCalledTimes(1);
    expect(JSON.parse(window.sessionStorage.getItem(GUARD) ?? "{}")).toMatchObject({
      to: BUILD_B,
      from: BUILD_A,
      how: "route",
      n: 1
    });
  });

  it("does not reload when the app cancels the event, but apply() still does", async () => {
    focusAnInput();
    const page = boot({ live: BUILD_B });
    window.addEventListener("oxy:update-available", (e) => e.preventDefault());
    await poll();
    await vi.advanceTimersByTimeAsync(5 * MIN);
    history.pushState(null, "", `${BASE}orders`);
    expect(page.reload).not.toHaveBeenCalled();

    const api = (window as unknown as { __oxyUpdate: { apply: () => boolean } }).__oxyUpdate;
    expect(api.apply()).toBe(true);
    expect(page.reload).toHaveBeenCalledTimes(1);
  });

  it("a reload the viewer refuses at the app's leave prompt frees apply(), and only apply()", async () => {
    focusAnInput();
    const page = boot({ live: BUILD_B });
    appAsksBeforeLeaving(page);
    await poll();
    await vi.advanceTimersByTimeAsync(61_000);
    history.pushState(null, "", `${BASE}orders`);
    expect(page.reload).toHaveBeenCalledTimes(1);

    // The viewer picks "Stay", and their next input on the page says so:
    // nothing reloads onto that build by itself again.
    window.dispatchEvent(new Event("pointerdown"));
    history.pushState(null, "", `${BASE}checklist`);
    expect(page.reload).toHaveBeenCalledTimes(1);

    // The Update ready pill (or the app) still can — after a second refusal
    // too, once the 5 min budget has room: a refusal is not held against the
    // build's own two tries.
    const api = (window as unknown as { __oxyUpdate: { apply: () => boolean } }).__oxyUpdate;
    expect(api.apply()).toBe(true);
    expect(page.reload).toHaveBeenCalledTimes(2);
    window.dispatchEvent(new Event("pointerdown"));
    expect(api.apply()).toBe(false);
    await vi.advanceTimersByTimeAsync(5 * MIN);
    expect(api.apply()).toBe(true);
    expect(page.reload).toHaveBeenCalledTimes(3);
  });

  it("a tap after Leave, before the reload commits, keeps the loop guard: the new page reports applied", async () => {
    focusAnInput();
    const page = boot({ live: BUILD_B });
    appAsksBeforeLeaving(page);
    await poll();
    await vi.advanceTimersByTimeAsync(61_000);
    history.pushState(null, "", `${BASE}orders`);
    // The viewer picked Leave; the old page still takes a tap while the
    // reload is under way, which reads exactly like a Stay.
    window.dispatchEvent(new Event("pointerdown"));
    expect(JSON.parse(window.sessionStorage.getItem(GUARD) ?? "{}")).toMatchObject({
      to: BUILD_B,
      how: "route",
      n: 1
    });

    unloadPage();
    const next = boot({ build: BUILD_B, live: BUILD_B });
    expect(updateRows(next.rows())).toContainEqual(
      expect.objectContaining({ phase: "applied", how: "route", from: BUILD_A, to: BUILD_B })
    );
  });

  it("input during a reload no prompt held up does not start another", async () => {
    focusAnInput();
    const page = boot({ live: BUILD_B });
    // The browser runs `beforeunload`, but no listener asks.
    page.reload.mockImplementation(() => {
      window.dispatchEvent(new Event("beforeunload", { cancelable: true }));
    });
    await poll();
    await vi.advanceTimersByTimeAsync(61_000);
    history.pushState(null, "", `${BASE}orders`);
    window.dispatchEvent(new Event("pointerdown"));
    history.pushState(null, "", `${BASE}checklist`);
    const api = (window as unknown as { __oxyUpdate: { apply: () => boolean } }).__oxyUpdate;
    expect(api.apply()).toBe(true);
    expect(page.reload).toHaveBeenCalledTimes(1);
    expect(JSON.parse(window.sessionStorage.getItem(GUARD) ?? "{}").n).toBe(1);
  });

  it("holds while a defer token is held", async () => {
    focusAnInput();
    const page = boot({ live: BUILD_B });
    const api = (window as unknown as { __oxyUpdate: { defer: (r: string) => () => void } })
      .__oxyUpdate;
    const release = api.defer("unsaved checklist");
    await poll();
    await vi.advanceTimersByTimeAsync(MIN);
    history.pushState(null, "", `${BASE}a`);
    expect(page.reload).not.toHaveBeenCalled();
    release();
    history.pushState(null, "", `${BASE}b`);
    expect(page.reload).toHaveBeenCalledTimes(1);
  });

  it("a leaked token stops blocking after 12 h", async () => {
    const page = boot({ live: BUILD_B });
    const api = (window as unknown as { __oxyUpdate: { defer: (r: string) => () => void } })
      .__oxyUpdate;
    api.defer("leaked");
    await poll();
    focusAnInput();
    await vi.advanceTimersByTimeAsync(11 * 60 * MIN);
    history.pushState(null, "", `${BASE}a`);
    expect(page.reload).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(61 * MIN);
    history.pushState(null, "", `${BASE}b`);
    expect(page.reload).toHaveBeenCalledTimes(1);
  });

  it("reloads after 2 min idle, but not while an input is focused", async () => {
    const input = focusAnInput();
    const page = boot({ live: BUILD_B });
    await poll();
    await vi.advanceTimersByTimeAsync(5 * MIN);
    expect(page.reload).not.toHaveBeenCalled();

    input.blur();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(page.reload).toHaveBeenCalledTimes(1);
    expect(JSON.parse(window.sessionStorage.getItem(GUARD) ?? "{}").how).toBe("idle");
  });

  it("a hidden tab is never 'idle' — that is the resume path's call", async () => {
    const page = boot({ live: BUILD_B });
    await poll();
    page.setVisibility("hidden");
    await vi.advanceTimersByTimeAsync(5 * MIN);
    expect(page.reload).not.toHaveBeenCalled();
  });

  it("input resets the idle clock", async () => {
    const page = boot({ live: BUILD_B });
    await poll();
    window.dispatchEvent(new Event("pointerdown"));
    for (let i = 0; i < 6; i++) {
      await vi.advanceTimersByTimeAsync(MIN);
      window.dispatchEvent(new Event("pointerdown"));
    }
    expect(page.reload).not.toHaveBeenCalled();
  });

  it("reloads on resume after 10+ min hidden, not after a short lock", async () => {
    focusAnInput();
    const page = boot({ live: BUILD_B });
    await poll();
    await vi.advanceTimersByTimeAsync(2 * MIN);

    page.setVisibility("hidden");
    await vi.advanceTimersByTimeAsync(3 * MIN);
    page.setVisibility("visible");
    await vi.advanceTimersByTimeAsync(0);
    expect(page.reload).not.toHaveBeenCalled();

    page.setVisibility("hidden");
    await vi.advanceTimersByTimeAsync(11 * MIN);
    page.setVisibility("visible");
    await vi.advanceTimersByTimeAsync(0);
    expect(page.reload).toHaveBeenCalledTimes(1);
  });

  it("prompt mode waits for the app, then escalates to auto after 24 h", async () => {
    focusAnInput();
    const page = boot({ mode: "prompt", live: BUILD_B });
    await poll();
    await vi.advanceTimersByTimeAsync(5 * MIN);
    history.pushState(null, "", `${BASE}a`);
    expect(page.reload).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(24 * 60 * MIN);
    history.pushState(null, "", `${BASE}b`);
    expect(page.reload).toHaveBeenCalled();
  });
});

describe("runtime.js updates — loop guard", () => {
  it("reports applied when the reload landed on the target, clearing the target but not the budget", () => {
    const at = Date.now() - 90_000;
    window.sessionStorage.setItem(
      GUARD,
      JSON.stringify({ to: BUILD_A, from: BUILD_B, how: "route", n: 1, at, log: [at] })
    );
    const page = boot();
    expect(updateRows(page.rows())).toEqual([
      expect.objectContaining({ phase: "applied", how: "route", from: BUILD_B, to: BUILD_A })
    ]);
    expect(JSON.parse(window.sessionStorage.getItem(GUARD) ?? "null")).toEqual({ log: [at] });
  });

  it("after a reload that stayed on the old build, retries once after 90 s", async () => {
    focusAnInput();
    window.sessionStorage.setItem(
      GUARD,
      JSON.stringify({ to: BUILD_B, from: BUILD_A, how: "route", n: 1, at: Date.now() })
    );
    const page = boot({ live: BUILD_B });
    await vi.advanceTimersByTimeAsync(0);
    expect(page.probes()).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(70_000);
    history.pushState(null, "", `${BASE}a`);
    expect(page.reload).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(25_000);
    history.pushState(null, "", `${BASE}b`);
    expect(page.reload).toHaveBeenCalledTimes(1);
    expect(JSON.parse(window.sessionStorage.getItem(GUARD) ?? "{}").n).toBe(2);
  });

  it("after the second failed reload, reports guarded and never reloads to that target", async () => {
    focusAnInput();
    window.sessionStorage.setItem(
      GUARD,
      JSON.stringify({ to: BUILD_B, from: BUILD_A, how: "idle", n: 2, at: Date.now() })
    );
    const page = boot({ live: BUILD_B });
    await poll();
    await vi.advanceTimersByTimeAsync(5 * MIN);
    history.pushState(null, "", `${BASE}a`);
    expect(page.reload).not.toHaveBeenCalled();
    expect(updateRows(page.rows())).toContainEqual(
      expect.objectContaining({ phase: "guarded", to: BUILD_B })
    );
  });

  it("does not reload automatically when sessionStorage is unavailable", async () => {
    focusAnInput();
    const throwing = {
      getItem: () => {
        throw new Error("SecurityError");
      },
      setItem: () => {
        throw new Error("SecurityError");
      },
      removeItem: () => {
        throw new Error("SecurityError");
      }
    } as unknown as Storage;
    const page = boot({ live: BUILD_B, storage: throwing });
    await poll();
    await vi.advanceTimersByTimeAsync(2 * MIN);
    history.pushState(null, "", `${BASE}a`);
    expect(page.reload).not.toHaveBeenCalled();
  });
});

function rejectWith(reason: unknown) {
  window.dispatchEvent(Object.assign(new Event("unhandledrejection"), { reason }));
}

const CHUNK_404 = "Failed to fetch dynamically imported module: https://x/assets/Orders-1a2b.js";

describe("runtime.js updates — stale lazy-chunk recovery", () => {
  it("reloads, even in observe mode, when a chunk fails and the build moved", async () => {
    const page = boot({ mode: "observe", live: BUILD_B });
    rejectWith(new TypeError(CHUNK_404));
    await vi.advanceTimersByTimeAsync(MIN);
    expect(page.probes()).toHaveLength(1);
    expect(page.reload).toHaveBeenCalledTimes(1);
    expect(JSON.parse(window.sessionStorage.getItem(GUARD) ?? "{}").how).toBe("chunk");
    // Version skew, not a bug: it is not reported as an app error.
    expect(page.rows().filter((r) => r.n === "oxy-error")).toHaveLength(0);
  });

  it("reports the error as before when the build has not moved", async () => {
    const page = boot({ mode: "observe", live: BUILD_A });
    rejectWith(new TypeError(CHUNK_404));
    await vi.advanceTimersByTimeAsync(MIN);
    expect(page.reload).not.toHaveBeenCalled();
    const errors = page.rows().filter((r) => r.n === "oxy-error");
    expect(errors).toHaveLength(1);
    expect(errors[0].p.counts).toEqual({ TypeError: 1 });
  });

  it("recognises webpack's ChunkLoadError and Safari's wording", async () => {
    for (const reason of [
      Object.assign(new Error("Loading chunk 7 failed."), { name: "ChunkLoadError" }),
      new TypeError("Importing a module script failed.")
    ]) {
      window.sessionStorage.clear();
      const page = boot({ mode: "observe", live: BUILD_B });
      rejectWith(reason);
      await vi.advanceTimersByTimeAsync(MIN);
      expect(page.reload).toHaveBeenCalledTimes(1);
    }
  });

  it("leaves an ordinary rejection alone", async () => {
    const page = boot({ mode: "observe", live: BUILD_B });
    rejectWith(new TypeError("x is undefined"));
    await vi.advanceTimersByTimeAsync(0);
    expect(page.probes()).toHaveLength(0);
    expect(page.reload).not.toHaveBeenCalled();
  });

  it("cancels Vite's preload error synchronously when the new build is already known", async () => {
    const page = boot({ mode: "observe", live: BUILD_B });
    await poll();
    const ev = new Event("vite:preloadError", { cancelable: true });
    window.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(true);
    expect(page.reload).toHaveBeenCalledTimes(1);
  });

  it("probes on Vite's preload error when the new build is not yet known", async () => {
    const page = boot({ mode: "observe", live: BUILD_B });
    const ev = new Event("vite:preloadError", { cancelable: true });
    window.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(false);
    await vi.advanceTimersByTimeAsync(MIN);
    expect(page.reload).toHaveBeenCalledTimes(1);
  });

  it("respects a defer token: unsaved work outranks a broken lazy route", async () => {
    const page = boot({ mode: "observe", live: BUILD_B });
    (window as unknown as { __oxyUpdate: { defer: (r: string) => () => void } }).__oxyUpdate.defer(
      "unsaved"
    );
    rejectWith(new TypeError(CHUNK_404));
    await vi.advanceTimersByTimeAsync(MIN);
    expect(page.reload).not.toHaveBeenCalled();
    expect(page.rows().filter((r) => r.n === "oxy-error")).toHaveLength(1);
  });

  it("does nothing beyond reporting in off mode, shipped whole or cut", async () => {
    expect(RUNTIME_OFF.length).toBeLessThan(RUNTIME.length - 4000);
    expect(RUNTIME_OFF).not.toContain("__oxy/build");
    for (const source of [RUNTIME, RUNTIME_OFF]) {
      // Analytics on, so the error row carries its message.
      const page = boot({ mode: "off", live: BUILD_B, source, analytics: true });
      rejectWith(new TypeError(CHUNK_404));
      // A route change reaches the cut agent only through its hooks.
      history.pushState(null, "", `${BASE}orders`);
      await vi.advanceTimersByTimeAsync(MIN);
      expect(page.probes()).toHaveLength(0);
      expect(page.reload).not.toHaveBeenCalled();
      // The chunk error itself, not a TypeError thrown by a hook that was cut.
      const errors = page.rows().filter((r) => r.n === "oxy-error");
      expect(errors).toHaveLength(1);
      expect(errors[0].p.details).toEqual([
        expect.objectContaining({ k: "unhandledrejection", m: CHUNK_404 })
      ]);
      expect((window as unknown as Record<string, unknown>).__oxyUpdate).toBeUndefined();
      for (const [target, type, fn, opts] of listeners) {
        target.removeEventListener(type, fn, opts as EventListenerOptions);
      }
      listeners = [];
      history.pushState = originalPush;
      history.replaceState = originalReplace;
    }
  });

  it("reports a chunk error with the page when it goes away before the decision", async () => {
    // A broken lazy route 5 s after load, and the viewer reloads it at 6 s: the
    // decision is still 54 s away, and a page that is unloading is not one a
    // reload will recover, so the error must leave with it.
    const page = boot({ mode: "observe", live: BUILD_A });
    await vi.advanceTimersByTimeAsync(5000);
    rejectWith(new TypeError(CHUNK_404));
    await vi.advanceTimersByTimeAsync(1000);
    const errors = page.rows().filter((r) => r.n === "oxy-error");
    expect(errors).toHaveLength(1);
    expect(errors[0].p.counts).toEqual({ TypeError: 1 });
    // The decision, when it comes, does not report it a second time.
    await vi.advanceTimersByTimeAsync(MIN);
    expect(page.probes()).toHaveLength(1);
    expect(page.rows().filter((r) => r.n === "oxy-error")).toHaveLength(1);
  });

  it("reports a pending chunk error when the page is hidden, the end of a mobile session", async () => {
    const page = boot({ mode: "observe", live: BUILD_A });
    rejectWith(new TypeError(CHUNK_404));
    page.setVisibility("hidden");
    // Read straight off the beacon: `rows()` would dispatch a pagehide too.
    await vi.advanceTimersByTimeAsync(0);
    const sent = (vi.mocked(fetch).mock.calls as unknown as [string, RequestInit][])
      .filter(([url]) => url === `${BASE}__oxy/beacon`)
      .flatMap(([, init]) => JSON.parse(init.body as string).events);
    expect(sent.filter((r: { n: string }) => r.n === "oxy-error")).toHaveLength(1);
  });

  it("still recovers a chunk error settled while hidden, once shown again (an accepted over-count)", async () => {
    const page = boot({ mode: "observe", live: BUILD_B });
    rejectWith(new TypeError(CHUNK_404));
    page.setVisibility("hidden");
    page.setVisibility("visible");
    await vi.advanceTimersByTimeAsync(MIN);
    // The broken screen is still recovered…
    expect(page.reload).toHaveBeenCalledTimes(1);
    // …and the error, already sent on hide, is not sent a second time.
    expect(page.rows().filter((r) => r.n === "oxy-error")).toHaveLength(1);
  });

  it("waits out the 60 s after load in which pods may disagree, then asks again", async () => {
    // A fresh page on A whose chunk 404'd while a pod still answered B: by the
    // time the floor passes the pods agree on A, so nothing reloads.
    const page = boot({ mode: "observe", live: BUILD_B });
    rejectWith(new TypeError(CHUNK_404));
    await vi.advanceTimersByTimeAsync(MIN - 1000);
    expect(page.probes()).toHaveLength(0);
    page.setLive(BUILD_A);
    await vi.advanceTimersByTimeAsync(1000);
    expect(page.probes()).toHaveLength(1);
    expect(page.reload).not.toHaveBeenCalled();
    expect(page.rows().filter((r) => r.n === "oxy-error")).toHaveLength(1);
  });

  it("caps automatic reloads per tab whatever the target, so two pods cannot bounce it", async () => {
    // The two reloads of the last 5 min went A→B and B→A; a third, to B, is refused.
    const now = Date.now();
    window.sessionStorage.setItem(GUARD, JSON.stringify({ log: [now - 2 * MIN, now - MIN] }));
    const page = boot({ mode: "observe", live: BUILD_B });
    rejectWith(new TypeError(CHUNK_404));
    rejectWith(new TypeError(`${CHUNK_404}?again`));
    await vi.advanceTimersByTimeAsync(MIN);
    expect(page.reload).not.toHaveBeenCalled();
    const guarded = updateRows(page.rows()).filter((r) => r.phase === "guarded");
    expect(guarded).toEqual([expect.objectContaining({ how: "chunk", to: BUILD_B })]);

    // Once the oldest reload is 5 min old, the budget has room again.
    await vi.advanceTimersByTimeAsync(3 * MIN);
    rejectWith(new TypeError(`${CHUNK_404}?later`));
    await vi.advanceTimersByTimeAsync(0);
    expect(page.reload).toHaveBeenCalledTimes(1);
  });

  it("does not reload while an editable element is focused", async () => {
    focusAnInput();
    const page = boot({ mode: "observe", live: BUILD_B });
    rejectWith(new TypeError(CHUNK_404));
    await vi.advanceTimersByTimeAsync(MIN);
    expect(page.reload).not.toHaveBeenCalled();
    expect(page.rows().filter((r) => r.n === "oxy-error")).toHaveLength(1);
  });

  it("does not reload when the app cancelled the announcement for that build", async () => {
    const page = boot({ mode: "observe", live: BUILD_B });
    window.addEventListener("oxy:update-available", (e) => e.preventDefault());
    rejectWith(new TypeError(CHUNK_404));
    await vi.advanceTimersByTimeAsync(MIN);
    expect(page.reload).not.toHaveBeenCalled();
  });

  it("announces a cancelable oxy:chunk-stale first, and a cancel keeps the page", async () => {
    const page = boot({ mode: "observe", live: BUILD_B });
    const stale: CustomEvent[] = [];
    window.addEventListener("oxy:chunk-stale", (e) => {
      stale.push(e as CustomEvent);
      e.preventDefault();
    });
    rejectWith(new TypeError(CHUNK_404));
    await vi.advanceTimersByTimeAsync(MIN);
    expect(stale.map((e) => e.detail)).toEqual([{ from: BUILD_A, to: BUILD_B }]);
    expect(page.reload).not.toHaveBeenCalled();
    expect(page.rows().filter((r) => r.n === "oxy-error")).toHaveLength(1);
  });
});

describe("runtime.js updates — triggers, gates and status", () => {
  it("probes when the network comes back", async () => {
    const page = boot({ mode: "observe" });
    window.dispatchEvent(new Event("online"));
    await vi.advanceTimersByTimeAsync(0);
    expect(page.probes()).toHaveLength(1);
  });

  it("probes on a bfcache restore, not on an ordinary pageshow", async () => {
    const page = boot({ mode: "observe" });
    window.dispatchEvent(Object.assign(new Event("pageshow"), { persisted: false }));
    await vi.advanceTimersByTimeAsync(0);
    expect(page.probes()).toHaveLength(0);
    window.dispatchEvent(Object.assign(new Event("pageshow"), { persisted: true }));
    await vi.advanceTimersByTimeAsync(0);
    expect(page.probes()).toHaveLength(1);
  });

  it("never applies while offline — a reload then lands on the cached shell", async () => {
    focusAnInput();
    const online = vi.spyOn(navigator, "onLine", "get").mockReturnValue(false);
    const page = boot({ live: BUILD_B });
    await poll();
    await vi.advanceTimersByTimeAsync(MIN);
    history.pushState(null, "", `${BASE}a`);
    expect(page.reload).not.toHaveBeenCalled();
    online.mockReturnValue(true);
    history.pushState(null, "", `${BASE}b`);
    expect(page.reload).toHaveBeenCalledTimes(1);
  });

  it("jitters the poll between 4 and 6 minutes", async () => {
    for (const [random, at] of [
      [0, 4 * MIN],
      [0.999_999, 6 * MIN]
    ] as const) {
      vi.spyOn(Math, "random").mockReturnValue(random);
      const page = boot({ mode: "observe" });
      await vi.advanceTimersByTimeAsync(at - 1000);
      expect(page.probes()).toHaveLength(0);
      await vi.advanceTimersByTimeAsync(1000);
      expect(page.probes()).toHaveLength(1);
      vi.clearAllTimers();
    }
  });

  it("fires oxy:update-status on a rollback, a re-detection and a restored session", async () => {
    const page = boot({ mode: "observe", live: BUILD_B });
    const api = (window as unknown as { __oxyUpdate: { status: () => Record<string, unknown> } })
      .__oxyUpdate;
    const changes = vi.fn();
    const announced = vi.fn();
    window.addEventListener("oxy:update-status", changes);
    window.addEventListener("oxy:update-available", announced);

    await poll();
    expect(changes).toHaveBeenCalledTimes(1);
    page.setLive(BUILD_A);
    await poll();
    expect(changes).toHaveBeenCalledTimes(2);
    expect(api.status().available).toBeNull();
    page.setLive(BUILD_B);
    await poll();
    expect(changes).toHaveBeenCalledTimes(3);
    expect(api.status().available).toBe(BUILD_B);
    expect(announced).toHaveBeenCalledTimes(1);

    page.setLive("redirect");
    await poll();
    expect(api.status().sessionExpired).toBe(true);
    page.setLive(BUILD_B);
    await poll();
    expect(api.status().sessionExpired).toBe(false);
    expect(changes).toHaveBeenCalledTimes(5);
  });

  it("a tab still behind re-reports every 6 h so the open-builds window keeps it", async () => {
    const page = boot({ mode: "observe", live: BUILD_B });
    await poll();
    for (let i = 0; i < 6 * 12; i++) await poll();
    const rows = updateRows(page.rows());
    expect(rows.map((r) => r.phase)).toEqual(["detected", "stale"]);
    expect(rows[1]).toMatchObject({ how: "poll", from: BUILD_A, to: BUILD_B, build: BUILD_A });
  });

  it("ignores another app's loop guard on the same origin", async () => {
    window.sessionStorage.setItem(
      "oxy-update:22222222-2222-4222-8222-222222222222",
      JSON.stringify({ to: BUILD_B, from: BUILD_A, how: "idle", n: 2, at: Date.now() })
    );
    const page = boot({ mode: "observe" });
    await vi.advanceTimersByTimeAsync(0);
    expect(updateRows(page.rows())).toEqual([]);
    expect(page.probes()).toHaveLength(0);
  });
});
