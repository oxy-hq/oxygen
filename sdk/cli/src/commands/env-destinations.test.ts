/**
 * `oxyc env create|update --destination` and `--fetch-host` — the warehouse
 * plane's per-environment overrides and the fetch plane's host list: how the
 * flags parse, and the bodies `env create` and `env update` send
 * (`globalThis.fetch` stubbed).
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { stubFetch } from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import { envCreate, runEnvCreate } from "./env.js";
import {
  applyDestinations,
  applyFetchHosts,
  noDestinationEdits,
  parseDestinationFlags,
  parseFetchHosts
} from "./env-plane-args.js";
import { runEnvUpdate } from "./env-planes.js";

const TARGET = "https://oxy.test";
const APP_ID = "a1a1a1a1-2222-3333-4444-555555555555";
const ENVS = `/api/customer-apps/${APP_ID}/environments`;
const PLANES = `${ENVS}/dev-a1/planes`;

function fakeContext(): Context {
  return {
    cwd: "/tmp",
    flags: { env: "production", apiKeyEnv: "OXY_API_KEY" },
    target: () => TARGET,
    env: () => ({ target: TARGET, orgSlug: undefined }) as ReturnType<Context["env"]>,
    bearer: async () => "tok",
    maybeBearer: async () => "tok",
    storedBearer: () => "tok",
    async credential() {
      return { token: "tok", source: "env" };
    },
    serviceAccount: () => undefined,
    apiKey: () => undefined,
    customer: () => undefined,
    repoDir: () => undefined,
    placeholders: () => ({}),
    withEnv: () => fakeContext()
  };
}

const ENVIRONMENT = { name: "dev-a1", kind: "dev", status: "active", build_id: null, url: null };

const routes = (planesSet: unknown) => ({
  "GET /api/customer-apps?limit=100&offset=0": () => ({
    status: 200,
    body: { items: [{ id: APP_ID, slug: "store", org_slug: "acme" }], next_offset: null }
  }),
  [`GET ${ENVS}/dev-a1`]: () => ({
    status: 200,
    body: { ...ENVIRONMENT, planes_set: planesSet }
  }),
  [`PUT ${PLANES}`]: () => ({ status: 200, body: ENVIRONMENT }),
  [`POST ${ENVS}`]: () => ({ status: 201, body: ENVIRONMENT })
});

afterEach(() => vi.unstubAllGlobals());

function silenced(): () => void {
  const write = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
  return () => write.mockRestore();
}

async function usageOf(run: Promise<unknown>): Promise<CliError> {
  const err = await run.catch((e: unknown) => e);
  expect(err).toBeInstanceOf(CliError);
  expect((err as CliError).code).toBe(ExitCode.USAGE);
  return err as CliError;
}

describe("parseDestinationFlags", () => {
  it("reads <db>=<mapped>, trims, and keeps the last of a repeated db", () => {
    expect(parseDestinationFlags(["ch = ch_dev", "pg=pg_dev", "ch=ch_two"], [])).toEqual({
      set: { ch: "ch_two", pg: "pg_dev" },
      clear: []
    });
  });

  it("refuses a flag without both names, pointing at --clear-destination", () => {
    for (const bad of ["ch", "ch=", "=ch_dev", " = "]) {
      const message = (() => {
        try {
          parseDestinationFlags([bad], []);
        } catch (e) {
          return (e as CliError).message + ((e as CliError).hint ?? "");
        }
        return "";
      })();
      expect(message, bad).toMatch(/not <db>=<mapped>/);
    }
  });

  it("refuses a database set and cleared in one command, and a blank clear", () => {
    expect(() => parseDestinationFlags(["ch=ch_dev"], ["ch"])).toThrow(/both set/);
    expect(() => parseDestinationFlags([], [" "])).toThrow(/needs a database name/);
    expect(parseDestinationFlags([], ["ch", "ch"])).toEqual({ set: {}, clear: ["ch"] });
  });
});

describe("applyDestinations", () => {
  it("builds the object form from nothing, and leaves other planes alone", () => {
    expect(applyDestinations({ storage: "own" }, { set: { ch: "ch_dev" }, clear: [] })).toEqual({
      storage: "own",
      warehouse: { mode: "mapped", destinations: { ch: "ch_dev" } }
    });
  });

  it("merges key by key into the bare string or the object form", () => {
    const flags = { set: { pg: "pg_dev" }, clear: [] };
    expect(applyDestinations({ warehouse: "mapped" }, flags)).toEqual({
      warehouse: { mode: "mapped", destinations: { pg: "pg_dev" } }
    });
    expect(
      applyDestinations(
        { warehouse: { mode: "mapped", destinations: { ch: "ch_dev", pg: "old" } } },
        flags
      )
    ).toEqual({ warehouse: { mode: "mapped", destinations: { ch: "ch_dev", pg: "pg_dev" } } });
  });

  it("clears one override, and falls back to the bare mode when none is left", () => {
    const planes = { warehouse: { mode: "mapped", destinations: { ch: "ch_dev", pg: "pg_dev" } } };
    expect(applyDestinations(planes, { set: {}, clear: ["ch"] })).toEqual({
      warehouse: { mode: "mapped", destinations: { pg: "pg_dev" } }
    });
    expect(applyDestinations(planes, { set: {}, clear: ["ch", "pg"] })).toEqual({
      warehouse: "mapped"
    });
  });

  it("clearing what was never set changes nothing", () => {
    expect(applyDestinations({ storage: "own" }, { set: {}, clear: ["ch"] })).toEqual({
      storage: "own"
    });
    expect(noDestinationEdits({ set: {}, clear: [] })).toBe(true);
    expect(noDestinationEdits(undefined)).toBe(true);
  });
});

describe("env update --destination", () => {
  const put = (calls: ReturnType<typeof stubFetch>) =>
    JSON.parse(calls.find((c) => c.method === "PUT")?.body ?? "null");

  it("merges the overrides into planes_set.warehouse and keeps the other planes", async () => {
    const calls = stubFetch(
      TARGET,
      routes({
        storage: "own",
        warehouse: { mode: "mapped", destinations: { ch: "ch_dev" } }
      })
    );
    const restore = silenced();
    await runEnvUpdate(fakeContext(), "acme/store", "dev-a1", {
      destination: ["pg=pg_dev"],
      json: true
    });
    restore();
    expect(put(calls)).toEqual({
      planes: {
        storage: "own",
        warehouse: { mode: "mapped", destinations: { ch: "ch_dev", pg: "pg_dev" } }
      }
    });
  });

  it("--clear-destination removes one override and leaves the rest", async () => {
    const calls = stubFetch(
      TARGET,
      routes({ warehouse: { mode: "mapped", destinations: { ch: "ch_dev", pg: "pg_dev" } } })
    );
    const restore = silenced();
    await runEnvUpdate(fakeContext(), "acme/store", "dev-a1", {
      clearDestination: ["ch"],
      json: true
    });
    restore();
    expect(put(calls)).toEqual({
      planes: { warehouse: { mode: "mapped", destinations: { pg: "pg_dev" } } }
    });
  });

  it("--replace starts the warehouse plane from the flags alone", async () => {
    const calls = stubFetch(
      TARGET,
      routes({ warehouse: { mode: "mapped", destinations: { ch: "ch_dev" } } })
    );
    const restore = silenced();
    await runEnvUpdate(fakeContext(), "acme/store", "dev-a1", {
      destination: ["pg=pg_dev"],
      replace: true,
      json: true
    });
    restore();
    expect(put(calls)).toEqual({
      planes: { warehouse: { mode: "mapped", destinations: { pg: "pg_dev" } } }
    });
  });

  it("a malformed flag is refused before any request (exit 2)", async () => {
    const calls = stubFetch(TARGET, routes({}));
    await usageOf(
      runEnvUpdate(fakeContext(), "acme/store", "dev-a1", { destination: ["ch="], json: true })
    );
    expect(calls).toEqual([]);
  });
});

describe("env create --destination", () => {
  const post = (calls: ReturnType<typeof stubFetch>) =>
    JSON.parse(calls.find((c) => c.method === "POST" && c.path === ENVS)?.body ?? "null");

  it("sends the overrides as the warehouse plane, beside --plane flags", async () => {
    const calls = stubFetch(TARGET, routes({}));
    const restore = silenced();
    await runEnvCreate(fakeContext(), "acme/store", "dev-a1", {
      json: true,
      plane: ["storage=empty"],
      destination: ["ch=ch_dev"]
    });
    restore();
    expect(post(calls)).toEqual({
      name: "dev-a1",
      planes: {
        storage: "empty",
        warehouse: { mode: "mapped", destinations: { ch: "ch_dev" } }
      }
    });
  });

  it("sends nothing extra when no override is given", async () => {
    const calls = stubFetch(TARGET, routes({}));
    await envCreate(fakeContext(), "acme/store", "dev-a1", { destinations: {} });
    expect(post(calls)).toEqual({ name: "dev-a1" });
  });
});

describe("parseFetchHosts", () => {
  it("lowercases, trims and de-duplicates", () => {
    expect(
      parseFetchHosts([" API.Stripe.com", "api.stripe.com", "connect.squareupsandbox.com"])
    ).toEqual(["api.stripe.com", "connect.squareupsandbox.com"]);
  });

  it.each(["", "https://api.stripe.com", "api.stripe.com/v1", "api.stripe.com:443"])(
    "refuses %j (exit 2)",
    (raw) => {
      const err = (() => {
        try {
          parseFetchHosts([raw]);
        } catch (e) {
          return e;
        }
      })();
      expect(err).toBeInstanceOf(CliError);
      expect((err as CliError).code).toBe(ExitCode.USAGE);
    }
  );
});

describe("applyFetchHosts", () => {
  it("hosts set fetch to allow with exactly that list", () => {
    expect(applyFetchHosts({ storage: "empty" }, ["api.stripe.com"])).toEqual({
      storage: "empty",
      fetch: { mode: "allow", hosts: ["api.stripe.com"] }
    });
    expect(
      applyFetchHosts({ fetch: { mode: "allow", hosts: ["old.example"] } }, ["api.stripe.com"])
    ).toEqual({ fetch: { mode: "allow", hosts: ["api.stripe.com"] } });
  });

  it("refuses hosts beside another fetch mode", () => {
    expect(() => applyFetchHosts({ fetch: "held-writes" }, ["api.stripe.com"])).toThrow(CliError);
    expect(() => applyFetchHosts({ fetch: "default" }, ["api.stripe.com"])).toThrow(CliError);
  });

  it("a bare fetch=allow keeps the stored list, and is refused with none", () => {
    const stored = { mode: "allow", hosts: ["api.stripe.com"] };
    expect(applyFetchHosts({ fetch: "allow" }, [], stored)).toEqual({ fetch: stored });
    expect(() => applyFetchHosts({ fetch: "allow" }, [])).toThrow(CliError);
    expect(() => applyFetchHosts({ fetch: "allow" }, [], "held-writes")).toThrow(CliError);
  });

  it("leaves planes alone with no hosts and no fetch=allow", () => {
    const planes = { fetch: "held-writes" };
    expect(applyFetchHosts(planes, [])).toBe(planes);
  });
});

describe("env update --fetch-host", () => {
  const put = (calls: ReturnType<typeof stubFetch>) =>
    JSON.parse(calls.find((c) => c.method === "PUT")?.body ?? "null");

  it("--plane fetch=allow --fetch-host sends the object form and keeps the other planes", async () => {
    const calls = stubFetch(TARGET, routes({ storage: "empty" }));
    const restore = silenced();
    await runEnvUpdate(fakeContext(), "acme/store", "dev-a1", {
      plane: ["fetch=allow"],
      fetchHost: ["connect.squareupsandbox.com", "API.STRIPE.COM"],
      json: true
    });
    restore();
    expect(put(calls)).toEqual({
      planes: {
        storage: "empty",
        fetch: { mode: "allow", hosts: ["connect.squareupsandbox.com", "api.stripe.com"] }
      }
    });
  });

  it("--fetch-host alone implies fetch=allow", async () => {
    const calls = stubFetch(TARGET, routes(null));
    const restore = silenced();
    await runEnvUpdate(fakeContext(), "acme/store", "dev-a1", {
      fetchHost: ["api.stripe.com"],
      json: true
    });
    restore();
    expect(put(calls)).toEqual({
      planes: { fetch: { mode: "allow", hosts: ["api.stripe.com"] } }
    });
  });

  it("--plane fetch=allow with no host and none stored is refused before the PUT", async () => {
    const calls = stubFetch(TARGET, routes({}));
    await usageOf(
      runEnvUpdate(fakeContext(), "acme/store", "dev-a1", { plane: ["fetch=allow"], json: true })
    );
    expect(calls.some((c) => c.method === "PUT")).toBe(false);
  });

  it("--plane fetch=default clears the plane, hosts and all", async () => {
    const calls = stubFetch(
      TARGET,
      routes({ fetch: { mode: "allow", hosts: ["api.stripe.com"] }, storage: "empty" })
    );
    const restore = silenced();
    await runEnvUpdate(fakeContext(), "acme/store", "dev-a1", {
      plane: ["fetch=default"],
      json: true
    });
    restore();
    expect(put(calls)).toEqual({ planes: { storage: "empty" } });
  });
});

describe("env create --fetch-host", () => {
  const post = (calls: ReturnType<typeof stubFetch>) =>
    JSON.parse(calls.find((c) => c.method === "POST" && c.path === ENVS)?.body ?? "null");

  it("sends the fetch plane beside --plane and --destination", async () => {
    const calls = stubFetch(TARGET, routes({}));
    const restore = silenced();
    await runEnvCreate(fakeContext(), "acme/store", "dev-a1", {
      json: true,
      plane: ["storage=empty"],
      destination: ["ch=ch_dev"],
      fetchHost: ["api.stripe.com"]
    });
    restore();
    expect(post(calls)).toEqual({
      name: "dev-a1",
      planes: {
        storage: "empty",
        warehouse: { mode: "mapped", destinations: { ch: "ch_dev" } },
        fetch: { mode: "allow", hosts: ["api.stripe.com"] }
      }
    });
  });

  it("--plane fetch=allow with no host is refused before any request", async () => {
    const calls = stubFetch(TARGET, routes({}));
    await usageOf(
      runEnvCreate(fakeContext(), "acme/store", "dev-a1", { json: true, plane: ["fetch=allow"] })
    );
    expect(calls).toEqual([]);
  });
});
