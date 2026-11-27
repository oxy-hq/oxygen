import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ConfigEnv, ResolvedConfig, UserConfig } from "vite";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import oxyApp, { manifestWarnings, resolveBase, validateManifest } from "./index.js";

describe("validateManifest", () => {
  test("accepts a v2 identity-only manifest", () => {
    expect(
      validateManifest({ schemaVersion: 2, slug: "my-app", name: "My App" })
    ).toEqual([]);
  });

  test("accepts a manifest with optional orgSlug and projectId hints", () => {
    expect(
      validateManifest({
        schemaVersion: 2,
        slug: "my-app",
        orgSlug: "acme",
        projectId: "00000000-0000-0000-0000-000000000001"
      })
    ).toEqual([]);
  });

  test("rejects v1 schemaVersion", () => {
    const errs = validateManifest({ schemaVersion: 1, slug: "x" });
    expect(errs.some((e) => e.includes("schemaVersion"))).toBe(true);
  });

  test("rejects v1 leakage (products field)", () => {
    const errs = validateManifest({
      schemaVersion: 2,
      slug: "x",
      products: { foo: {} }
    });
    expect(errs.some((e) => e.includes("v1 fields"))).toBe(true);
  });

  test("rejects v1 leakage (writers field)", () => {
    const errs = validateManifest({
      schemaVersion: 2,
      slug: "x",
      writers: { foo: {} }
    });
    expect(errs.some((e) => e.includes("v1 fields"))).toBe(true);
  });

  test("rejects missing slug", () => {
    const errs = validateManifest({ schemaVersion: 2 });
    expect(errs.some((e) => e.includes("`slug` is required"))).toBe(true);
  });

  test("rejects malformed slug (uppercase)", () => {
    const errs = validateManifest({ schemaVersion: 2, slug: "MyApp" });
    expect(errs.some((e) => e.includes("malformed"))).toBe(true);
  });

  test("rejects malformed slug (starts with dash)", () => {
    const errs = validateManifest({ schemaVersion: 2, slug: "-bad" });
    expect(errs.some((e) => e.includes("malformed"))).toBe(true);
  });

  // The cases that used to build cleanly and then 422 at `oxy publish` — the
  // whole reason to catch them at build.
  test("rejects malformed slug (trailing dash)", () => {
    const errs = validateManifest({ schemaVersion: 2, slug: "app-" });
    expect(errs.some((e) => e.includes("malformed"))).toBe(true);
  });

  test("rejects malformed slug (double dash)", () => {
    const errs = validateManifest({ schemaVersion: 2, slug: "a--b" });
    expect(errs.some((e) => e.includes("malformed"))).toBe(true);
  });

  test("rejects malformed slug (underscore)", () => {
    const errs = validateManifest({ schemaVersion: 2, slug: "my_app" });
    expect(errs.some((e) => e.includes("malformed"))).toBe(true);
  });

  test("rejects malformed slug (over 63 chars)", () => {
    const errs = validateManifest({ schemaVersion: 2, slug: "a".repeat(64) });
    expect(errs.some((e) => e.includes("malformed"))).toBe(true);
  });

  test("accepts a valid hyphenated slug", () => {
    const errs = validateManifest({ schemaVersion: 2, slug: "oltp-bookings" });
    expect(errs.some((e) => e.includes("malformed"))).toBe(false);
  });

  test("rejects a path-traversal function name", () => {
    const errs = validateManifest({
      schemaVersion: 2,
      slug: "app",
      functions: { "../../x": { route: true } }
    });
    expect(errs.some((e) => e.includes("function name") && e.includes("malformed"))).toBe(true);
  });

  test("rejects functions declared as an array", () => {
    const errs = validateManifest({
      schemaVersion: 2,
      slug: "app",
      functions: []
    });
    expect(errs.some((e) => e.includes("functions must be an object"))).toBe(true);
  });

  test("accepts a valid functions map", () => {
    const errs = validateManifest({
      schemaVersion: 2,
      slug: "app",
      functions: { notify: { route: true } }
    });
    expect(errs.some((e) => e.includes("function name"))).toBe(false);
  });

  test("rejects a notify kind in Oxygen's reserved namespace", () => {
    const errs = validateManifest({
      schemaVersion: 2,
      slug: "app",
      functions: {
        watch: { notify: { kinds: { "oxy.anomaly": {}, "oxygen.tank_low": {}, late: {} } } }
      }
    });
    expect(errs.filter((e) => e.includes("reserved for Oxygen's own notices"))).toEqual([
      expect.stringContaining('function "watch" declares notify kind "oxy.anomaly"')
    ]);
  });

  test("rejects missing schemaVersion as if it were v1", () => {
    const errs = validateManifest({ slug: "x" });
    expect(errs.some((e) => e.includes("schemaVersion"))).toBe(true);
  });
});

describe("resolveBase", () => {
  const savedEnv = process.env.OXY_APP_BASE_PATH;
  beforeEach(() => {
    delete process.env.OXY_APP_BASE_PATH;
  });
  afterEach(() => {
    if (savedEnv === undefined) delete process.env.OXY_APP_BASE_PATH;
    else process.env.OXY_APP_BASE_PATH = savedEnv;
  });

  test("env var wins over manifest", () => {
    process.env.OXY_APP_BASE_PATH = "/customer-apps/from-env/x/";
    expect(
      resolveBase({ schemaVersion: 2, slug: "from-manifest", orgSlug: "y" })
    ).toBe("/customer-apps/from-env/x/");
  });

  test("env var gets a trailing slash if absent", () => {
    process.env.OXY_APP_BASE_PATH = "/customer-apps/x/y";
    expect(resolveBase(null)).toBe("/customer-apps/x/y/");
  });

  test("derives base from manifest orgSlug + slug when env unset", () => {
    expect(
      resolveBase({ schemaVersion: 2, slug: "store-pulse", orgSlug: "acme" })
    ).toBe("/customer-apps/acme/store-pulse/");
  });

  test("falls back to / when neither env nor manifest hints exist", () => {
    expect(resolveBase({ schemaVersion: 2, slug: "x" })).toBe("/");
    expect(resolveBase(null)).toBe("/");
  });

  test("manifest with only one of orgSlug/slug also falls back to /", () => {
    expect(resolveBase({ schemaVersion: 2, slug: "x" })).toBe("/");
    expect(
      resolveBase({ schemaVersion: 2, slug: "x", orgSlug: "" })
    ).toBe("/");
  });
});

describe("the notification catalogue", () => {
  const CATALOGUE = {
    schemaVersion: 2,
    slug: "store-ops",
    functions: { temps: { notify: { kinds: { temp_alarm: {} } } } },
    notifications: {
      categories: { "operations/food-safety": { label: "Food safety" } },
      kinds: { temp_alarm: { category: "operations/food-safety", level: "critical" } }
    }
  };

  test("accepts a valid one", () => {
    expect(validateManifest(CATALOGUE)).toEqual([]);
    expect(manifestWarnings(CATALOGUE)).toEqual([]);
  });

  test("rejects what oxyc publish refuses, naming where", () => {
    const errs = validateManifest({
      ...CATALOGUE,
      notifications: {
        kinds: { temp_alarm: { category: "finance", level: "urgent" } }
      }
    });
    expect(errs).toEqual([
      expect.stringMatching(/^notifications\.kinds\.temp_alarm\.category: .*"finance"/),
      expect.stringMatching(/^notifications\.kinds\.temp_alarm\.level: .*"urgent"/)
    ]);
  });

  test("warns, without rejecting, about a kind no function sends", () => {
    const m = {
      ...CATALOGUE,
      notifications: { kinds: { temp_alarm: {}, unsent: {} } }
    };
    expect(validateManifest(m)).toEqual([]);
    expect(manifestWarnings(m)).toEqual([
      "notifications.kinds.unsent: no function declares it under notify.kinds, so nothing can send it yet"
    ]);
  });

  describe("fails `pnpm dev` and `pnpm build` early", () => {
    let dir = "";
    beforeEach(() => {
      dir = mkdtempSync(join(tmpdir(), "oxy-vite-plugin-"));
    });
    afterEach(() => {
      rmSync(dir, { recursive: true, force: true });
      vi.restoreAllMocks();
    });

    const configure = (manifest: unknown, command: ConfigEnv["command"]) => {
      writeFileSync(join(dir, "oxy-app.json"), JSON.stringify(manifest));
      const plugin = oxyApp();
      const hook = plugin.config as (c: UserConfig, e: ConfigEnv) => UserConfig;
      return () => hook({ root: dir }, { command, mode: "development" });
    };
    const bad = { ...CATALOGUE, notifications: { kinds: { temp_alarm: { level: "urgent" } } } };

    for (const command of ["serve", "build"] as const) {
      test(`throws on ${command}, with no v1 migration link for a catalogue error`, () => {
        const run = configure(bad, command);
        expect(run).toThrow(/notifications\.kinds\.temp_alarm\.level/);
        expect(run).not.toThrow(/Migration guide/);
      });
    }

    test("prints warnings and carries on", () => {
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
      const run = configure({ ...CATALOGUE, notifications: { kinds: { unsent: {} } } }, "serve");
      expect(run).not.toThrow();
      expect(warn).toHaveBeenCalledWith(
        expect.stringContaining("notifications.kinds.unsent: no function declares it")
      );
    });

    test("still links the migration guide for a v1 manifest", () => {
      expect(configure({ schemaVersion: 1, slug: "x" }, "build")).toThrow(/Migration guide/);
    });
  });
});

// `buildStart` looks the app's project up on the Oxy server. Vitest drives Vite
// as a dev server too (`command: "serve"`), so a scaffold's `vitest run` reached
// whatever answered on the default target — a developer's local server.
describe("the dev project lookup", () => {
  const TARGET = "http://127.0.0.1:46200";
  let dir = "";
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "oxy-vite-plugin-"));
    writeFileSync(
      join(dir, "oxy-app.json"),
      JSON.stringify({ schemaVersion: 2, slug: "storefront", orgSlug: "local" })
    );
    // A shell that already names the project would skip the lookup for its own reason.
    for (const key of ["OXY_ORG", "OXY_APP", "OXY_PROJECT"]) vi.stubEnv(key, "");
    fetchMock = vi.fn(async () => Response.json({ project_id: "p-1", branch: "main" }));
    vi.stubGlobal("fetch", fetchMock);
    vi.spyOn(console, "log").mockImplementation(() => {});
  });
  afterEach(() => {
    rmSync(dir, { recursive: true, force: true });
    vi.unstubAllEnvs();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  const startServer = async (mode: string) => {
    const plugin = oxyApp({ devProxyTarget: TARGET });
    (plugin.config as (c: UserConfig, e: ConfigEnv) => UserConfig)(
      { root: dir },
      { command: "serve", mode }
    );
    (plugin.configResolved as (c: ResolvedConfig) => void)({
      command: "serve",
      mode
    } as ResolvedConfig);
    await (plugin.buildStart as () => Promise<void>)();
  };

  test("a test run makes no request", async () => {
    vi.stubEnv("VITEST", "true");
    await startServer("test");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  test("a test run under another --mode makes no request either", async () => {
    vi.stubEnv("VITEST", "true");
    await startServer("staging");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  test("a dev server still resolves the project", async () => {
    vi.stubEnv("VITEST", undefined);
    await startServer("development");
    expect(fetchMock).toHaveBeenCalledWith(`${TARGET}/api/apps/local/storefront/build-config`);
  });
});
