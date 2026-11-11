// @vitest-environment jsdom
//
// A read that answers 200 with something other than rows (`{}`, an error page a
// proxy answered with). useQuery's cache kept that body before reading rows out
// of it: the first read failed inside a promise and showed as the hook's error,
// but the next component mounting the same query took the cached body in its
// effect, where `rows.map` threw with nothing to catch it, and React unmounted
// the whole app (Site Scout, pokehouse-oxy #558). Such a body now fails like any
// other read, is never cached, and `refetch` recovers.

import * as React from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { OxyApiError } from "./errors";
import { _resetCustomAppManifestCacheForTest } from "./manifest";
import { __clearQueryCache, getCached, sharedQuery } from "./query-cache";
import { OxyAppProvider, useQuery, useSemanticQuery } from "./react";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const ROWS = { columns: ["store"], rows: [["Almaden"]] };
const SQL = "select store from sites";

/** A fetcher answering every data read with 200 and whatever `answer.body` holds now. */
function answering(initial: unknown) {
  const answer = { body: initial };
  const fetcher = vi.fn(
    async () =>
      new Response(JSON.stringify(answer.body), {
        status: 200,
        headers: { "content-type": "application/json" }
      })
  );
  return { answer, fetcher };
}

describe("sharedQuery with a 200 that carries no rows", () => {
  beforeEach(() => __clearQueryCache());

  it("fails as a read error and keeps nothing", async () => {
    for (const body of [
      {},
      { columns: ["a"] },
      { columns: ["a"], rows: null },
      { columns: [], rows: [null] }
    ]) {
      __clearQueryCache();
      const { fetcher } = answering(body);
      const err = await sharedQuery(fetcher, "p1", SQL, undefined).catch((e: unknown) => e);
      expect(err).toBeInstanceOf(OxyApiError);
      expect((err as OxyApiError).code).toBe("malformed_read");
      expect(getCached("p1", SQL, undefined)).toBeUndefined();
    }
  });

  it("asks again next time, and keeps the rows once they come", async () => {
    const { answer, fetcher } = answering({});
    await expect(sharedQuery(fetcher, "p1", SQL, undefined)).rejects.toThrow();
    answer.body = ROWS;
    await expect(sharedQuery(fetcher, "p1", SQL, undefined)).resolves.toEqual(ROWS);
    expect(fetcher).toHaveBeenCalledTimes(2);
    expect(getCached("p1", SQL, undefined)).toEqual(ROWS);
  });

  it("keeps a body that breaks off mid-read as that error, not a read without rows", async () => {
    const dropped = new TypeError("network error");
    const resp = {
      ok: true,
      status: 200,
      json: async () => Promise.reject(dropped)
    } as unknown as Response;
    const err = await sharedQuery(async () => resp, "p1", SQL, undefined).catch((e: unknown) => e);
    expect(err).toBe(dropped);
  });

  it("fails on a body that is not JSON at all", async () => {
    const fetcher = vi.fn(async () => new Response("<html>bad gateway</html>", { status: 200 }));
    const err = await sharedQuery(fetcher, "p1", SQL, undefined).catch((e: unknown) => e);
    expect((err as OxyApiError).code).toBe("malformed_read");
  });
});

// ── Mounted ─────────────────────────────────────────────────────────────────

let container: HTMLDivElement;
let root: Root;
const crashes: unknown[] = [];

class Boundary extends React.Component<{ children: React.ReactNode }, { crashed: boolean }> {
  state = { crashed: false };
  static getDerivedStateFromError() {
    return { crashed: true };
  }
  componentDidCatch(err: unknown) {
    crashes.push(err);
  }
  render() {
    return this.state.crashed ? <p data-testid='crashed' /> : this.props.children;
  }
}

beforeEach(() => {
  _resetCustomAppManifestCacheForTest();
  __clearQueryCache();
  crashes.length = 0;
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
    new Response(
      JSON.stringify({
        schemaVersion: 2,
        name: "T",
        slug: "test-app",
        orgSlug: "acme",
        projectId: "proj-uuid"
      }),
      { status: 200 }
    )) as typeof fetch;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await React.act(async () => root.unmount());
  container.remove();
});

type Seen = { rows: unknown[]; error: Error | null; loading: boolean; refetch: () => void };

function Reader({ id, seen }: { id: string; seen: Record<string, Seen> }) {
  const q = useQuery({ sql: SQL });
  seen[id] = q;
  return <p data-testid={id}>{q.error ? "error" : q.rows.length}</p>;
}

async function show(ui: React.ReactNode, fetcher: typeof fetch) {
  await React.act(async () => {
    root.render(
      <Boundary>
        <OxyAppProvider fetcher={fetcher}>{ui}</OxyAppProvider>
      </Boundary>
    );
  });
  // The read resolves a few microtasks later.
  await React.act(async () => {
    await new Promise((r) => setTimeout(r, 0));
  });
}

describe("useQuery with a 200 that carries no rows", () => {
  it("reports it, survives a second mount of the same query, and recovers on retry", async () => {
    const { answer, fetcher } = answering({});
    const seen: Record<string, Seen> = {};
    await show(<Reader id='first' seen={seen} />, fetcher as unknown as typeof fetch);
    expect(seen.first.error).toBeInstanceOf(OxyApiError);
    expect(seen.first.loading).toBe(false);

    // Another component with the same query: coming back to a tab, picking a market again.
    await show(
      <>
        <Reader id='first' seen={seen} />
        <Reader id='second' seen={seen} />
      </>,
      fetcher as unknown as typeof fetch
    );
    expect(crashes).toEqual([]);
    expect(container.querySelector('[data-testid="crashed"]')).toBeNull();
    expect(seen.second.error).toBeInstanceOf(OxyApiError);

    // The app's own retry, once the service answers with rows.
    answer.body = ROWS;
    await React.act(async () => seen.second.refetch());
    await React.act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(seen.second.error).toBeNull();
    expect(seen.second.rows).toEqual([{ store: "Almaden" }]);
  });

  it("still serves a good read from the cache to the next mount, without asking again", async () => {
    const { fetcher } = answering(ROWS);
    const seen: Record<string, Seen> = {};
    await show(<Reader id='first' seen={seen} />, fetcher as unknown as typeof fetch);
    await show(
      <>
        <Reader id='first' seen={seen} />
        <Reader id='second' seen={seen} />
      </>,
      fetcher as unknown as typeof fetch
    );
    expect(seen.second.rows).toEqual([{ store: "Almaden" }]);
    expect(fetcher).toHaveBeenCalledTimes(1);
  });
});

describe("useSemanticQuery with a 200 that carries no rows", () => {
  function Semantic({ seen }: { seen: Record<string, Seen> }) {
    seen.semantic = useSemanticQuery({ topic: "sites", measures: ["sites.count"] });
    return null;
  }

  it("reports the read error, not a TypeError from inside the SDK", async () => {
    const { fetcher } = answering({});
    const seen: Record<string, Seen> = {};
    await show(<Semantic seen={seen} />, fetcher as unknown as typeof fetch);
    expect(seen.semantic.error).toBeInstanceOf(OxyApiError);
    expect((seen.semantic.error as OxyApiError).code).toBe("malformed_read");
  });
});
