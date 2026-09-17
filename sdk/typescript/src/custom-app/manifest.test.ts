// Unit tests for the manifest parser (v2 only) and the SQL param
// interpolation helper used by useQuery.
//
// These tests exercise the validation paths without hitting the network
// or requiring a browser environment — `validateManifest` is the private
// function we reach indirectly via `loadCustomAppManifest`.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { _resetCustomAppManifestCacheForTest, loadCustomAppManifest } from "./manifest";

// ── Helpers ──────────────────────────────────────────────────────────────────

// We test the parser indirectly through loadCustomAppManifest with a
// mocked fetch because the validation logic lives inside the private
// `validateManifest` function. The cache must be cleared before each test.

function mockFetchReturning(body: unknown): void {
  globalThis.fetch = async () =>
    ({
      ok: true,
      status: 200,
      json: async () => body
    }) as Response;
}

function injectRuntime(orgSlug = "test-org", slug = "test-app"): void {
  const config = {
    orgSlug,
    slug,
    appId: "app-uuid",
    apiBaseUrl: ""
  };
  // readInjectedConfig checks `window.__OXY_APP__`. In the vitest node
  // environment `window` is not defined, so we shim it here. This is
  // safe because the shim is reset between tests via _resetCache.
  (globalThis as Record<string, unknown>).__OXY_APP__ = config;
  if (typeof window === "undefined") {
    (globalThis as Record<string, unknown>).window = { __OXY_APP__: config };
  } else {
    (window as Record<string, unknown>).__OXY_APP__ = config;
  }
}

beforeEach(() => {
  _resetCustomAppManifestCacheForTest();
  injectRuntime();
});

// ── v1 manifest — now rejected ────────────────────────────────────────────────

describe("parseOxyAppManifest — v1 (rejected)", () => {
  it("rejects a v1 manifest with schemaVersion 1", async () => {
    mockFetchReturning({
      schemaVersion: 1,
      name: "My App",
      products: {
        summary_kpis: {
          from: { type: "execute_sql", database: "main", sql: "SELECT 1" }
        }
      }
    });

    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /schemaVersion must be 2.*v1 manifests are no longer supported/i
    );
  });

  it("rejects a v1 manifest with optional identity fields", async () => {
    mockFetchReturning({
      schemaVersion: 1,
      slug: "my-app",
      orgSlug: "acme",
      projectId: "proj-uuid",
      products: {
        kpis: { from: { type: "execute_sql", database: "db", sql: "SELECT 1" } }
      }
    });

    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /schemaVersion must be 2/i
    );
  });

  it("rejects a v1 manifest with an empty products object", async () => {
    mockFetchReturning({ schemaVersion: 1, products: {} });

    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /schemaVersion must be 2/i
    );
  });
});

// ── v2 manifest ───────────────────────────────────────────────────────────────

describe("parseOxyAppManifest — v2", () => {
  it("accepts a v2 identity-only manifest and returns schemaVersion 2 with no products", async () => {
    mockFetchReturning({
      schemaVersion: 2,
      name: "Dashboard v2",
      slug: "dashboard-v2",
      orgSlug: "acme",
      projectId: "proj-uuid-2"
    });

    const resolved = await loadCustomAppManifest({ manifestUrl: "/oxy-app.json" });

    expect(resolved.manifest.schemaVersion).toBe(2);
    expect(resolved.productNames).toEqual([]);
    expect(resolved.projectId).toBe("proj-uuid-2");
    expect(resolved.manifest.name).toBe("Dashboard v2");
  });

  it("accepts a minimal v2 manifest with only schemaVersion and slug (no other optional fields)", async () => {
    mockFetchReturning({ schemaVersion: 2, slug: "my-app" });

    const resolved = await loadCustomAppManifest({ manifestUrl: "/oxy-app.json" });

    expect(resolved.manifest.schemaVersion).toBe(2);
    expect(resolved.manifest.slug).toBe("my-app");
    expect(resolved.productNames).toEqual([]);
    expect(resolved.projectId).toBeUndefined();
  });

  it("rejects a slug with an underscore (would alias a hyphenated sibling onto one OLTP schema)", async () => {
    mockFetchReturning({ schemaVersion: 2, slug: "my_app" });

    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /slug.*invalid.*no underscore/i
    );
  });

  it("rejects a slug with a leading hyphen", async () => {
    mockFetchReturning({ schemaVersion: 2, slug: "-app" });

    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /slug.*invalid/i
    );
  });

  it("rejects a v2 manifest that also declares products", async () => {
    mockFetchReturning({
      schemaVersion: 2,
      products: { kpis: { from: { type: "execute_sql", database: "db", sql: "SELECT 1" } } }
    });

    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /schemaVersion 2.*identity-only.*products.*writers/i
    );
  });

  it("rejects a v2 manifest that also declares writers", async () => {
    mockFetchReturning({
      schemaVersion: 2,
      writers: { my_writer: { table: "annotations" } }
    });

    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /schemaVersion 2.*identity-only/i
    );
  });

  it("rejects a v2 manifest with a missing slug", async () => {
    mockFetchReturning({ schemaVersion: 2, name: "No Slug" });

    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /`slug` is required and must be a non-empty string/
    );
  });

  it("rejects a v2 manifest with an empty slug", async () => {
    mockFetchReturning({ schemaVersion: 2, slug: "" });

    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /`slug` is required and must be a non-empty string/
    );
  });

  it("rejects a v2 manifest with a whitespace-only slug", async () => {
    mockFetchReturning({ schemaVersion: 2, slug: "   " });

    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /`slug` is required and must be a non-empty string/
    );
  });
});

// ── unsupported schemaVersion ─────────────────────────────────────────────────

describe("parseOxyAppManifest — unsupported version", () => {
  it("rejects manifests with schemaVersion 0", async () => {
    mockFetchReturning({ schemaVersion: 0 });

    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /schemaVersion must be 2/i
    );
  });

  it("rejects manifests with an unknown schemaVersion 99", async () => {
    mockFetchReturning({ schemaVersion: 99 });

    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /schemaVersion must be 2.*got 99/i
    );
  });

  it("rejects manifests with no schemaVersion", async () => {
    mockFetchReturning({ name: "No version" });

    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /schemaVersion must be 2/i
    );
  });
});

// ── v2 manifest accepts identity-only fields (contract lock) ─────────────────

describe("v2 manifest accepts identity-only fields", () => {
  beforeEach(() => {
    _resetCustomAppManifestCacheForTest();
    delete (globalThis as { __OXY_APP__?: unknown }).__OXY_APP__;
    if (typeof window !== "undefined") {
      delete (window as { __OXY_APP__?: unknown }).__OXY_APP__;
    }
  });

  it("validates a manifest with just schemaVersion + slug", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(
        JSON.stringify({
          schemaVersion: 2,
          slug: "minimal"
        }),
        { status: 200 }
      )
    );
    try {
      const resolved = await loadCustomAppManifest();
      expect(resolved.manifest.slug).toBe("minimal");
      expect(resolved.manifest.orgSlug).toBeUndefined();
      expect(resolved.manifest.projectId).toBeUndefined();
    } finally {
      fetchMock.mockRestore();
    }
  });

  it("validates a manifest with just schemaVersion + slug + name", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(
        JSON.stringify({
          schemaVersion: 2,
          slug: "minimal",
          name: "Display Name"
        }),
        { status: 200 }
      )
    );
    try {
      const resolved = await loadCustomAppManifest();
      expect(resolved.manifest.name).toBe("Display Name");
    } finally {
      fetchMock.mockRestore();
    }
  });
});

// ── identity precedence (injection > manifest) ────────────────────────────────

describe("identity precedence (injection > manifest)", () => {
  beforeEach(() => {
    _resetCustomAppManifestCacheForTest();
    // Clear both the global and the window shim set by the outer injectRuntime()
    delete (globalThis as { __OXY_APP__?: unknown }).__OXY_APP__;
    if (typeof window !== "undefined") {
      delete (window as { __OXY_APP__?: unknown }).__OXY_APP__;
    }
  });

  it("uses injected projectId when both injection and manifest are present", async () => {
    const config = {
      orgSlug: "acme",
      slug: "demo",
      projectId: "11111111-1111-1111-1111-111111111111",
      apiBaseUrl: ""
    };
    (globalThis as { __OXY_APP__?: unknown }).__OXY_APP__ = config;
    if (typeof window !== "undefined") {
      (window as { __OXY_APP__?: unknown }).__OXY_APP__ = config;
    }
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(
        JSON.stringify({
          schemaVersion: 2,
          slug: "demo",
          projectId: "22222222-2222-2222-2222-222222222222"
        }),
        { status: 200 }
      )
    );
    try {
      const resolved = await loadCustomAppManifest();
      expect(resolved.projectId).toBe("11111111-1111-1111-1111-111111111111");
    } finally {
      fetchMock.mockRestore();
    }
  });

  it("falls back to manifest projectId when no injection", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(
        JSON.stringify({
          schemaVersion: 2,
          slug: "demo",
          projectId: "33333333-3333-3333-3333-333333333333"
        }),
        { status: 200 }
      )
    );
    try {
      const resolved = await loadCustomAppManifest();
      expect(resolved.projectId).toBe("33333333-3333-3333-3333-333333333333");
    } finally {
      fetchMock.mockRestore();
    }
  });
});

// ── v2 functions block ────────────────────────────────────────────────────────

describe("parseOxyAppManifest — functions", () => {
  it("parses a valid functions map", async () => {
    mockFetchReturning({
      schemaVersion: 2,
      slug: "demo",
      functions: {
        "refresh-sales": { schedule: "0 6 * * *", timezone: "UTC", timeoutSeconds: 60 },
        "sync-stripe": { airwayStep: { pipeline: "stripe_sync", resource: "transform" } }
      }
    });
    const resolved = await loadCustomAppManifest({ manifestUrl: "/oxy-app.json" });
    expect(Object.keys(resolved.manifest.functions ?? {})).toEqual([
      "refresh-sales",
      "sync-stripe"
    ]);
    expect(resolved.manifest.functions?.["refresh-sales"].timeoutSeconds).toBe(60);
  });

  it("rejects an invalid function name", async () => {
    mockFetchReturning({
      schemaVersion: 2,
      slug: "demo",
      functions: { Bad_Name: { route: true } }
    });
    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /function name "Bad_Name" must match/
    );
  });

  it("rejects a function with no active invocation surface", async () => {
    mockFetchReturning({
      schemaVersion: 2,
      slug: "demo",
      functions: { noop: { route: false } }
    });
    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /must enable at least one of route\/schedule\/airwayStep/
    );
  });

  it("rejects an out-of-range timeoutSeconds", async () => {
    mockFetchReturning({
      schemaVersion: 2,
      slug: "demo",
      functions: { slow: { route: true, timeoutSeconds: 9999 } }
    });
    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /timeoutSeconds.*\[1, 300\]/
    );
  });

  it("parses check: true", async () => {
    mockFetchReturning({
      schemaVersion: 2,
      slug: "demo",
      functions: { smoke: { check: true, schedule: "*/15 * * * *" } }
    });
    const resolved = await loadCustomAppManifest({ manifestUrl: "/oxy-app.json" });
    expect(resolved.manifest.functions?.smoke.check).toBe(true);
  });

  it("rejects a non-boolean check", async () => {
    mockFetchReturning({
      schemaVersion: 2,
      slug: "demo",
      functions: { smoke: { check: "yes", route: true } }
    });
    await expect(loadCustomAppManifest({ manifestUrl: "/oxy-app.json" })).rejects.toThrow(
      /function "smoke" `check` must be a boolean/
    );
  });
});

describe("webhook declarations", () => {
  const load = (webhook: unknown) => {
    mockFetchReturning({
      schemaVersion: 2,
      slug: "demo",
      functions: { "ingest-report": { route: false, webhook } }
    });
    return loadCustomAppManifest({ manifestUrl: "/oxy-app.json" });
  };

  it("accepts a complete declaration and leaves the encoding to default", async () => {
    const resolved = await load({ secretVar: "UBER_KEYS", signatureHeader: "x-uber-signature" });
    expect(resolved.manifest.functions?.["ingest-report"].webhook).toEqual({
      secretVar: "UBER_KEYS",
      signatureHeader: "x-uber-signature"
    });
  });

  it("keeps an explicit encoding", async () => {
    const resolved = await load({
      secretVar: "K",
      signatureHeader: "x-sig",
      encoding: "base64"
    });
    expect(resolved.manifest.functions?.["ingest-report"].webhook?.encoding).toBe("base64");
  });

  // A half-written block is the dangerous case: the server reads no contract
  // and answers 404 forever, on a URL the author has already handed to a
  // provider, with nothing anywhere saying why. Fail at authoring time instead.
  it.each([
    [{ signatureHeader: "x-sig" }, /webhook\.secretVar/],
    [{ secretVar: "K" }, /webhook\.signatureHeader/],
    [{ secretVar: "  ", signatureHeader: "x-sig" }, /webhook\.secretVar/],
    [{ secretVar: "K", signatureHeader: "x-sig", encoding: "base32" }, /webhook\.encoding/],
    ["not-an-object", /`webhook` must be an object/]
  ])("rejects an unusable declaration %#", async (webhook, message) => {
    await expect(load(webhook)).rejects.toThrow(message);
  });

  // A webhook is an invocation surface in its own right, so declaring one must
  // not force the function to also expose the authenticated `/fn/` route.
  it("is a sufficient invocation surface on its own", async () => {
    const resolved = await load({ secretVar: "K", signatureHeader: "x-sig" });
    expect(resolved.manifest.functions?.["ingest-report"].route).toBe(false);
  });
});
