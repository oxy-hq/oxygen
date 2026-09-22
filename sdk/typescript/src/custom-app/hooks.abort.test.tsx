// @vitest-environment jsdom
//
// What settles a fetching hook when its request is aborted.
//
// Every hook here creates an `AbortController` per effect run and aborts it in
// the cleanup, then decides in the `.catch` whether the rejection deserves to
// be reported. Getting that decision wrong strands the hook on `loading: true`
// with `error: null` — the worst failure shape there is, because the app
// renders skeletons forever with nothing to explain them.
//
// Two orderings matter and they are NOT the same:
//
//   1. An abort WE caused (an `enabled` transition, a changed input, a
//      `refetch`, unmount). The per-run `cancelled` flag marks it and the
//      successor effect run owns the state — the disabled branch clears
//      `loading`, the fetching branch sets it again. These tests pin that,
//      because it is the ordering a reader assumes is broken and isn't.
//   2. An abort we did NOT cause — a fetcher carrying its own timeout (see
//      `OxyClient.request`), a dev-proxy socket drop, a navigation. Here
//      `cancelled` is false and there is no successor run, so matching
//      `AbortError` by name and returning was the hang.
//
// Mounted against jsdom and driven through React's real commit ordering: a
// hand-rolled effect simulation would only re-state what we assumed.

import * as React from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { _resetCustomAppManifestCacheForTest } from "./manifest";
import { useMetricTree, useSensitivity } from "./metric-tree-hooks";
import { __clearQueryCache } from "./query-cache";
import { OxyAppProvider, useAgentRun, useProcedureRun, useQuery, useSemanticQuery } from "./react";
import { useWorldModelGraph, useWorldModelInstances } from "./world-model-hooks";

// ── Harness ─────────────────────────────────────────────────────────────────

/** One in-flight request, handed to the test instead of the network. */
interface Pending {
  url: string;
  /** Resolve it with a 200 carrying `body`. */
  settle: (body: unknown) => void;
  /** Reject it — a transport failure, or an abort we did not cause. */
  fail: (err: unknown) => void;
  aborted: boolean;
}

/**
 * A fetcher that queues every call for the test and — like a real `fetch` —
 * rejects with an `AbortError` DOMException the moment its signal aborts.
 */
function deferredFetcher(): { fetcher: typeof fetch; pending: Pending[] } {
  const pending: Pending[] = [];
  const fetcher = ((input: RequestInfo | URL, init?: RequestInit) =>
    new Promise<Response>((resolve, reject) => {
      const entry: Pending = {
        url: String(input),
        settle: (body) => resolve({ ok: true, status: 200, json: async () => body } as Response),
        fail: reject,
        aborted: false
      };
      init?.signal?.addEventListener("abort", () => {
        entry.aborted = true;
        reject(new DOMException("The user aborted a request.", "AbortError"));
      });
      pending.push(entry);
    })) as typeof fetch;
  return { fetcher, pending };
}

/** An `AbortError` nobody in this package asked for. */
function foreignAbort(): DOMException {
  return new DOMException("The user aborted a request.", "AbortError");
}

/** The hook's own requests — the manifest load goes through global fetch. */
function dataCalls(pending: Pending[]): Pending[] {
  return pending.filter((p) => !p.url.endsWith("oxy-app.json"));
}

/**
 * A body that satisfies every hook under test at once. Each reads its own
 * fields and ignores the rest, which keeps the shared matrix below readable.
 */
const ANY_BODY = {
  columns: ["n"],
  rows: [[1]],
  truncated: false,
  nodes: [],
  edges: [],
  instances: [],
  drivers: []
};

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  _resetCustomAppManifestCacheForTest();
  __clearQueryCache();
  window.__OXY_APP__ = {
    appId: "app-uuid",
    slug: "test-app",
    orgId: "org-uuid",
    orgSlug: "acme",
    projectId: "proj-uuid",
    branch: "main",
    apiBaseUrl: ""
  };
  globalThis.fetch = (async () =>
    ({
      ok: true,
      status: 200,
      json: async () => ({
        schemaVersion: 2,
        name: "Test App",
        slug: "test-app",
        orgSlug: "acme",
        projectId: "proj-uuid"
      })
    }) as Response) as typeof fetch;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await React.act(async () => {
    root.unmount();
  });
  container.remove();
});

/** The slice of a hook result every hook here has in common. */
interface Probe {
  loading: boolean;
  error: Error | null;
  refetch: () => void;
}

/** Mount `hook` behind the provider and drive its `enabled` through React. */
async function mountHook(
  hook: (enabled: boolean) => Probe,
  fetcher: typeof fetch
): Promise<{
  setEnabled: (v: boolean) => Promise<void>;
  refetch: () => Promise<void>;
  last: () => Probe;
}> {
  let snapshot: Probe = { loading: false, error: null, refetch: () => {} };
  let drive: ((v: boolean) => void) | undefined;

  function ProbeComponent(): React.JSX.Element {
    const [enabled, setEnabled] = React.useState(true);
    drive = setEnabled;
    snapshot = hook(enabled);
    return <div />;
  }

  await React.act(async () => {
    root.render(
      <OxyAppProvider fetcher={fetcher}>
        <ProbeComponent />
      </OxyAppProvider>
    );
  });

  return {
    setEnabled: async (v) => {
      await React.act(async () => {
        drive?.(v);
      });
    },
    refetch: async () => {
      await React.act(async () => {
        snapshot.refetch();
      });
    },
    last: () => snapshot
  };
}

// ── The hang: an abort nobody here caused ───────────────────────────────────

describe("useSemanticQuery — an abort we did not cause", () => {
  it("is reported instead of stranding the hook on loading", async () => {
    const { fetcher, pending } = deferredFetcher();
    const h = await mountHook(
      (enabled) => useSemanticQuery({ topic: "orders" }, { enabled }),
      fetcher
    );
    expect(h.last().loading).toBe(true);

    // Nothing in the hook aborted this one: the signal is untouched and the
    // effect is still live. A fetcher with its own timeout looks like this.
    await React.act(async () => {
      dataCalls(pending)[0].fail(foreignAbort());
    });

    expect(dataCalls(pending)[0].aborted).toBe(false);
    expect(h.last().loading).toBe(false);
    expect(h.last().error).not.toBeNull();
    // Reported, and worded for whoever reads it: no user aborted anything.
    // The platform's own message survives as `cause`.
    expect(h.last().error?.message).toMatch(/interrupted/);
    expect(String((h.last().error as { cause?: unknown }).cause)).toContain("AbortError");
  });

  it("still aborts the request it supersedes", async () => {
    const { fetcher, pending } = deferredFetcher();
    const h = await mountHook(
      (enabled) => useSemanticQuery({ topic: "orders" }, { enabled }),
      fetcher
    );

    await h.setEnabled(false);

    // Aborting the superseded request is the point of the controller — the
    // fix is about who settles state afterwards, not about letting it run on.
    expect(dataCalls(pending)[0].aborted).toBe(true);
    expect(h.last().loading).toBe(false);

    await h.setEnabled(true);
    expect(dataCalls(pending)).toHaveLength(2);
  });

  it("stays recoverable — refetch re-issues after one", async () => {
    const { fetcher, pending } = deferredFetcher();
    const h = await mountHook(
      (enabled) => useSemanticQuery({ topic: "orders" }, { enabled }),
      fetcher
    );
    await React.act(async () => {
      dataCalls(pending)[0].fail(foreignAbort());
    });

    await h.refetch();
    expect(dataCalls(pending)).toHaveLength(2);

    await React.act(async () => {
      dataCalls(pending)[1].settle(ANY_BODY);
    });
    expect(h.last().loading).toBe(false);
    expect(h.last().error).toBeNull();
  });
});

// ── The ordering that is fine, pinned so it stays fine ──────────────────────
//
// An `enabled` transition aborts the in-flight request in the cleanup, and
// then the effect body runs again in the same commit: the `!enabled` branch
// clears `loading`, so the swallowed abort strands nothing. That branch is
// load-bearing — these cases fail if it is ever "simplified" away.

const GATED_HOOKS: Array<[string, (enabled: boolean) => Probe]> = [
  ["useSemanticQuery", (enabled) => useSemanticQuery({ topic: "orders" }, { enabled })],
  ["useQuery", (enabled) => useQuery({ sql: "SELECT 1" }, { enabled })],
  ["useMetricTree", (enabled) => useMetricTree({ enabled })],
  ["useSensitivity", (enabled) => useSensitivity("orders.net_sales", { enabled })],
  ["useWorldModelGraph", (enabled) => useWorldModelGraph({ enabled })],
  ["useWorldModelInstances", (enabled) => useWorldModelInstances("location", { enabled })]
];

for (const [name, hook] of GATED_HOOKS) {
  describe(`${name} — enabled transitions`, () => {
    it("settles to not-loading when enabled goes false mid-flight", async () => {
      const { fetcher, pending } = deferredFetcher();
      const h = await mountHook(hook, fetcher);
      expect(dataCalls(pending)).toHaveLength(1);
      expect(h.last().loading).toBe(true);

      await h.setEnabled(false);

      expect(h.last().loading).toBe(false);
      expect(h.last().error).toBeNull();
    });

    it("converges when enabled goes false then true", async () => {
      const { fetcher, pending } = deferredFetcher();
      const h = await mountHook(hook, fetcher);

      await h.setEnabled(false);
      await h.setEnabled(true);
      expect(h.last().loading).toBe(true);

      // Settle everything outstanding: which request answers is the hook's
      // business (`useQuery` shares one through `sharedQuery`), that it
      // answers at all is ours.
      await React.act(async () => {
        for (const call of dataCalls(pending)) call.settle(ANY_BODY);
      });

      expect(h.last().loading).toBe(false);
      expect(h.last().error).toBeNull();
    });

    it("recovers through refetch after a failed read", async () => {
      const { fetcher, pending } = deferredFetcher();
      const h = await mountHook(hook, fetcher);

      await React.act(async () => {
        dataCalls(pending)[0].fail(new Error("warehouse unreachable"));
      });
      expect(h.last().loading).toBe(false);
      expect(h.last().error).not.toBeNull();

      await h.refetch();
      expect(dataCalls(pending)).toHaveLength(2);

      await React.act(async () => {
        dataCalls(pending)[1].settle(ANY_BODY);
      });
      expect(h.last().loading).toBe(false);
      expect(h.last().error).toBeNull();
    });

    it("refetch mid-flight still converges", async () => {
      const { fetcher, pending } = deferredFetcher();
      const h = await mountHook(hook, fetcher);

      await h.refetch();
      await React.act(async () => {
        for (const call of dataCalls(pending)) call.settle(ANY_BODY);
      });

      expect(h.last().loading).toBe(false);
      expect(h.last().error).toBeNull();
    });
  });
}

// ── The imperative runners ──────────────────────────────────────────────────
//
// `useProcedureRun` / `useAgentRun` have no `enabled`: their aborts come from
// a re-entrant call, `cancel()`, or unmount — all of which go through the
// run's own `ctrl.abort()`. So the signal itself, not the error's name, says
// whether an abort was ours. Before that, a foreign `AbortError` on the start
// request parked the run on "running" with no error, the same dead end.

describe("useProcedureRun — an abort we did not cause", () => {
  it("fails the run instead of parking it on running", async () => {
    const { fetcher, pending } = deferredFetcher();
    let hook: ReturnType<typeof useProcedureRun> | undefined;
    function ProbeComponent(): React.JSX.Element {
      hook = useProcedureRun({ procedureId: "rebuild" });
      return <div />;
    }
    await React.act(async () => {
      root.render(
        <OxyAppProvider fetcher={fetcher}>
          <ProbeComponent />
        </OxyAppProvider>
      );
    });

    await React.act(async () => {
      hook?.run();
    });
    expect(hook?.state).toBe("running");

    await React.act(async () => {
      dataCalls(pending)[0].fail(foreignAbort());
    });

    expect(hook?.state).toBe("failed");
    expect(hook?.error).not.toBeNull();
  });
});

describe("useAgentRun — an abort we did not cause", () => {
  it("fails the ask instead of parking it on running", async () => {
    const { fetcher, pending } = deferredFetcher();
    let hook: ReturnType<typeof useAgentRun> | undefined;
    function ProbeComponent(): React.JSX.Element {
      hook = useAgentRun({ agentId: "analyst" });
      return <div />;
    }
    await React.act(async () => {
      root.render(
        <OxyAppProvider fetcher={fetcher}>
          <ProbeComponent />
        </OxyAppProvider>
      );
    });

    await React.act(async () => {
      hook?.ask("why did net sales drop?");
    });
    expect(hook?.state).toBe("running");

    await React.act(async () => {
      dataCalls(pending)[0].fail(foreignAbort());
    });

    expect(hook?.state).toBe("failed");
    expect(hook?.error).not.toBeNull();
  });
});

/**
 * A fetcher for `useAgentRun`: answers the start POST, then serves an SSE
 * stream that optionally delivers one event before the connection goes away.
 *
 * `ending` is the half that matters: `"error"` is a transport error (an abort
 * nobody here asked for), `"eof"` is a graceful close — an HTTP/2 GOAWAY, a
 * proxy with a max connection lifetime. The hook must judge the two the same
 * way, so the tests below run the matrix. `streamOpens` counts reconnects.
 */
function droppingSseFetcher(opts: { deliverEvent: boolean; ending: "error" | "eof" }): {
  fetcher: typeof fetch;
  streamOpens: () => number;
} {
  let opens = 0;
  const fetcher = ((input: RequestInfo | URL) => {
    const url = String(input);
    if (!url.endsWith("/events")) {
      return Promise.resolve({
        ok: true,
        status: 200,
        json: async () => ({ run_id: "run-1", thread_id: "thread-1" })
      } as Response);
    }
    opens += 1;
    const seq = opens;
    // `pull` rather than `start`: `controller.error()` discards anything still
    // queued, so the event has to be READ before the connection drops.
    let delivered = !opts.deliverEvent;
    const body = new ReadableStream<Uint8Array>({
      pull(controller) {
        if (!delivered) {
          // A real event with a real id: this is the server answering.
          delivered = true;
          controller.enqueue(
            new TextEncoder().encode(`id: ${seq}\nevent: heartbeat\ndata: {}\n\n`)
          );
          return;
        }
        if (opts.ending === "eof") {
          controller.close();
          return;
        }
        controller.error(foreignAbort());
      }
    });
    return Promise.resolve({ ok: true, status: 200, body } as unknown as Response);
  }) as typeof fetch;
  return { fetcher, streamOpens: () => opens };
}

/** Mount `useAgentRun` and start a run. */
async function startAgentRun(fetcher: typeof fetch): Promise<() => ReturnType<typeof useAgentRun>> {
  let hook: ReturnType<typeof useAgentRun> | undefined;
  function ProbeComponent(): React.JSX.Element {
    hook = useAgentRun({ agentId: "analyst" });
    return <div />;
  }
  await React.act(async () => {
    root.render(
      <OxyAppProvider fetcher={fetcher}>
        <ProbeComponent />
      </OxyAppProvider>
    );
  });
  await React.act(async () => {
    hook?.ask("why did net sales drop?");
  });
  return () => hook as ReturnType<typeof useAgentRun>;
}

/** Walk the reconnect loop's 1s sleeps forward without waiting on them. */
async function runReconnects(cycles: number): Promise<void> {
  for (let i = 0; i < cycles; i += 1) {
    await React.act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
  }
}

describe("useAgentRun — the reconnect budget", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  // How a window ended must not change the verdict — only whether it made
  // progress. `"error"` and `"eof"` are the two halves of the transport, and
  // the loop reads one budget for both.
  for (const ending of ["error", "eof"] as const) {
    it(`keeps going past the ceiling when every window delivers (${ending})`, async () => {
      // Every window goes away, but every window delivered an event first —
      // a `fetcher` with its own request timeout on a healthy long run, or a
      // proxy recycling the connection. The ceiling must not fire on it.
      const { fetcher, streamOpens } = droppingSseFetcher({ deliverEvent: true, ending });
      const hook = await startAgentRun(fetcher);

      await runReconnects(8);

      expect(streamOpens()).toBeGreaterThan(5);
      expect(hook().state).toBe("running");
      expect(hook().error).toBeNull();
    });

    it(`still gives up after five windows that deliver nothing (${ending})`, async () => {
      const { fetcher, streamOpens } = droppingSseFetcher({ deliverEvent: false, ending });
      const hook = await startAgentRun(fetcher);

      await runReconnects(8);

      expect(streamOpens()).toBe(5);
      expect(hook().state).toBe("failed");
      expect(hook().error).not.toBeNull();
    });
  }
});

// ── The reported shape: reads gated on another read ─────────────────────────

describe("reads gated on an earlier read", () => {
  it("do not hang when the gate drops mid-flight and comes back", async () => {
    const { fetcher, pending } = deferredFetcher();
    let openGate: ((v: boolean) => void) | undefined;
    const dependents: Probe[] = [];

    function Dependent({ gateOpen }: { gateOpen: boolean }): React.JSX.Element {
      dependents.push(useSemanticQuery({ topic: "orders" }, { enabled: gateOpen }));
      return <div />;
    }
    function Tree(): React.JSX.Element {
      const [gateOpen, setGateOpen] = React.useState(true);
      openGate = setGateOpen;
      return (
        <>
          {[0, 1, 2, 3].map((i) => (
            <Dependent key={i} gateOpen={gateOpen} />
          ))}
        </>
      );
    }

    await React.act(async () => {
      root.render(
        <OxyAppProvider fetcher={fetcher}>
          <Tree />
        </OxyAppProvider>
      );
    });
    expect(dataCalls(pending)).toHaveLength(4);

    // The bridge read failed and retried, so the gate shuts and reopens.
    await React.act(async () => {
      openGate?.(false);
    });
    await React.act(async () => {
      openGate?.(true);
    });

    const reissued = dataCalls(pending).slice(4);
    expect(reissued).toHaveLength(4);
    await React.act(async () => {
      for (const call of reissued) call.settle(ANY_BODY);
    });

    const latest = dependents.slice(-4);
    expect(latest.map((d) => d.loading)).toEqual([false, false, false, false]);
    expect(latest.map((d) => d.error)).toEqual([null, null, null, null]);
  });
});
