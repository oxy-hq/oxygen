/**
 * Tests for getDuckDB() initialization and the CDN init race condition.
 *
 * The tests run in the Node environment (the default in vitest.config.ts).
 * Because `window` is undefined in Node, `isLocalhost()` inside duckdb.ts
 * returns false, so the CDN init path is exercised.
 *
 * Module-level state (`duckDB` and `initPromise`) is reset between tests via
 * `vi.resetModules()` + per-test `vi.doMock()` + dynamic `import()`.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// ── Vite ?url imports ─────────────────────────────────────────────────────────
// Vite transforms these to file URLs at build time; in tests we return plain
// placeholder strings so the module loads without a real WASM bundle.
// These vi.mock calls are hoisted and survive vi.resetModules() between tests.
vi.mock("@duckdb/duckdb-wasm/dist/duckdb-browser-eh.worker.js?url", () => ({
  default: "eh-worker.js"
}));
vi.mock("@duckdb/duckdb-wasm/dist/duckdb-browser-mvp.worker.js?url", () => ({
  default: "mvp-worker.js"
}));
vi.mock("@duckdb/duckdb-wasm/dist/duckdb-eh.wasm?url", () => ({ default: "eh.wasm" }));
vi.mock("@duckdb/duckdb-wasm/dist/duckdb-mvp.wasm?url", () => ({ default: "mvp.wasm" }));
vi.mock("@/libs/encoding", () => ({
  encodeBase64: (s: string) => Buffer.from(s).toString("base64")
}));

// ── setup / teardown ──────────────────────────────────────────────────────────

/** Flush all pending microtasks and one macrotask tick. */
const flush = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  // Fresh module → duckDB = null and initPromise = null for each test.
  vi.resetModules();
  // Worker is a browser API absent from Node. Must be a real constructor
  // (not a vi.fn() arrow mock) because duckdb.ts calls `new Worker(...)`.
  vi.stubGlobal("Worker", class MockWorker {});
  // URL.createObjectURL / revokeObjectURL are browser-only; patch them in.
  (URL as Record<string, unknown>).createObjectURL = vi.fn().mockReturnValue("blob:mock");
  (URL as Record<string, unknown>).revokeObjectURL = vi.fn();
});

afterEach(() => {
  vi.unstubAllGlobals();
  delete (URL as Record<string, unknown>).createObjectURL;
  delete (URL as Record<string, unknown>).revokeObjectURL;
});

// ── tests ─────────────────────────────────────────────────────────────────────

describe("getDuckDB – CDN init path", () => {
  /**
   * Regression: race condition on the CDN init path in duckdb.ts.
   *
   * Before the fix, the module-level `duckDB` variable was assigned
   * synchronously as a side-effect inside the argument list of
   * `origInstantiate.call(...)`:
   *
   *   await origInstantiate.call(
   *     (duckDB = new duckdb.AsyncDuckDB(...)),   // ← synchronous assignment
   *     bundle.mainModule,
   *     bundle.pthreadWorker
   *   );
   *
   * This meant that once the IIFE had progressed past `await selectBundle()`
   * and reached the `instantiate` call, `duckDB` was already truthy — even
   * though `instantiate()` was still in-flight.  Any `getDuckDB()` call made
   * at that moment would see `!duckDB === false`, skip `await init()`, and
   * return the uninstantiated instance, causing "DuckDB not initialized" errors.
   *
   * After the fix, `duckDB` is only assigned after `await cdnDb.instantiate()`
   * resolves, so concurrent callers must wait on `initPromise` together.
   *
   * How this test works
   * ───────────────────
   * 1. `selectBundle` is controlled by a deferred gate so we can hold the IIFE
   *    at a precise point.
   * 2. We resolve the gate, flush microtasks, and then fire a second
   *    `getDuckDB()` call at the moment the IIFE is blocked inside
   *    `await instantiate()`.
   * 3. With the buggy code, `duckDB` would already be truthy at that moment,
   *    so the second call would resolve immediately → `p2Settled` would be
   *    true before `resolveInstantiate()` is called.
   * 4. With the fixed code, `duckDB` is still null at that moment, so the
   *    second call must wait → `p2Settled` stays false until we unblock.
   */
  it("second concurrent call must not return an uninstantiated instance (race condition regression)", async () => {
    // ── deferred selectBundle ─────────────────────────────────────────────
    type Bundle = { mainModule: string; mainWorker: string; pthreadWorker: undefined };
    let resolveSelectBundle!: (b: Bundle) => void;
    const selectBundleGate = new Promise<Bundle>((res) => {
      resolveSelectBundle = res;
    });

    // ── deferred instantiate ──────────────────────────────────────────────
    let resolveInstantiate!: () => void;
    const instantiateGate = new Promise<void>((res) => {
      resolveInstantiate = res;
    });
    const instantiate = vi.fn().mockReturnValue(instantiateGate);

    // Put instantiate on the prototype so the old `origInstantiate.call()`
    // approach also finds it (instance-property mocks would shadow it and
    // make the pre-fix code throw before revealing the race).
    vi.doMock("@duckdb/duckdb-wasm", () => {
      class MockAsyncDuckDB {
        connect = vi.fn().mockResolvedValue({ close: vi.fn() });
      }
      (MockAsyncDuckDB.prototype as Record<string, unknown>).instantiate = instantiate;
      return {
        AsyncDuckDB: MockAsyncDuckDB,
        ConsoleLogger: class {},
        selectBundle: vi.fn().mockReturnValue(selectBundleGate),
        getJsDelivrBundles: vi.fn().mockReturnValue({})
      };
    });

    const { getDuckDB } = (await import("@/libs/duckdb")) as {
      getDuckDB: () => Promise<{ connect: () => Promise<unknown> }>;
    };

    // ── Step 1: fire p1 — the IIFE starts and blocks on selectBundleGate ──
    const p1 = getDuckDB();

    // ── Step 2: unblock selectBundle, then flush ───────────────────────────
    // After the flush, the IIFE has resumed, run all synchronous setup code
    // (URL.createObjectURL, new Worker, new AsyncDuckDB / cdnDb), and is now
    // suspended on `await instantiateGate`.
    //
    // At THIS exact moment the pre-fix code had already set `duckDB` (sync
    // side-effect in the call arguments), while the fix keeps `duckDB` null.
    resolveSelectBundle({ mainModule: "m", mainWorker: "w", pthreadWorker: undefined });
    await flush();

    // ── Step 3: fire p2 — the concurrent call that would race ─────────────
    const p2 = getDuckDB();
    let p2Settled = false;
    // Deliberately not awaited: p2 must still be pending at step 4, so awaiting
    // here would hang the test. A rejection surfaces at the `Promise.all` in step 5.
    void p2.then(() => {
      p2Settled = true;
    });

    // ── Step 4: flush again and assert p2 is still pending ─────────────────
    // Buggy code: `duckDB` was already truthy → p2 resolved immediately.
    // Fixed code: `duckDB` is still null → p2 is waiting on initPromise.
    await flush();
    expect(p2Settled).toBe(false);

    // ── Step 5: unblock instantiation and verify both calls settle ─────────
    resolveInstantiate();
    await Promise.all([p1, p2]);

    expect(p2Settled).toBe(true);
    // instantiate must have been invoked exactly once — not twice.
    expect(instantiate).toHaveBeenCalledTimes(1);
  });

  it("returns the same singleton instance on repeated sequential calls", async () => {
    vi.doMock("@duckdb/duckdb-wasm", () => ({
      AsyncDuckDB: class {
        instantiate = vi.fn().mockResolvedValue(undefined);
        connect = vi.fn().mockResolvedValue({ close: vi.fn() });
      },
      ConsoleLogger: class {},
      selectBundle: vi.fn().mockResolvedValue({
        mainModule: "m",
        mainWorker: "w",
        pthreadWorker: undefined
      }),
      getJsDelivrBundles: vi.fn().mockReturnValue({})
    }));

    const { getDuckDB } = await import("@/libs/duckdb");

    const db1 = await getDuckDB();
    const db2 = await getDuckDB();

    expect(db1).toBe(db2);
  });

  it("rejects when instantiation fails", async () => {
    vi.doMock("@duckdb/duckdb-wasm", () => ({
      AsyncDuckDB: class {
        instantiate = vi.fn().mockRejectedValue(new Error("WASM load failed"));
      },
      ConsoleLogger: class {},
      selectBundle: vi.fn().mockResolvedValue({
        mainModule: "m",
        mainWorker: "w",
        pthreadWorker: undefined
      }),
      getJsDelivrBundles: vi.fn().mockReturnValue({})
    }));

    const { getDuckDB } = await import("@/libs/duckdb");

    await expect(getDuckDB()).rejects.toThrow("WASM load failed");
  });
});
