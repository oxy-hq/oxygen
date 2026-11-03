/**
 * `oxyc env create` plane flags, `env update` and `env reset` — the request
 * bodies and the usage errors, driven with `globalThis.fetch` stubbed.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { stubFetch } from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import { envCreate, runEnvCreate } from "./env.js";
import {
  describePlanesSet,
  mergePlanes,
  parsePlaneFlags,
  parsePlaneKeys,
  requireForkAt,
  validatePlaneMap
} from "./env-plane-args.js";
import { describeReset, envReset, envUpdate, runEnvReset, runEnvUpdate } from "./env-planes.js";

const TARGET = "https://oxy.test";
const APP_ID = "a1a1a1a1-2222-3333-4444-555555555555";
const ENVS = `/api/customer-apps/${APP_ID}/environments`;

function fakeContext(bearer = "tok"): Context {
  return {
    cwd: "/tmp",
    flags: { env: "production", apiKeyEnv: "OXY_API_KEY" },
    target: () => TARGET,
    env: () => ({ target: TARGET, orgSlug: undefined }) as ReturnType<Context["env"]>,
    bearer: async () => bearer,
    maybeBearer: async () => bearer,
    storedBearer: () => bearer,
    async credential() {
      return { token: bearer, source: "env" };
    },
    serviceAccount: () => undefined,
    apiKey: () => undefined,
    customer: () => undefined,
    repoDir: () => undefined,
    placeholders: () => ({}),
    withEnv: () => fakeContext(bearer)
  };
}

const APPS = {
  "GET /api/customer-apps?limit=100&offset=0": () => ({
    status: 200,
    body: { items: [{ id: APP_ID, slug: "store", org_slug: "acme" }], next_offset: null }
  })
};

const ENVIRONMENT = {
  name: "dev-a1",
  kind: "dev",
  status: "active",
  build_id: null,
  url: null,
  planes: { build: "own", storage: "own", oltp: "shared" },
  planes_set: { storage: "own" }
};

afterEach(() => vi.unstubAllGlobals());

function capture(): { printed: () => string; restore: () => void } {
  let printed = "";
  const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
    printed += String(chunk);
    return true;
  });
  return { printed: () => printed, restore: () => write.mockRestore() };
}

async function usageOf(run: Promise<unknown>): Promise<CliError> {
  const err = await run.catch((e: unknown) => e);
  expect(err).toBeInstanceOf(CliError);
  expect((err as CliError).code).toBe(ExitCode.USAGE);
  return err as CliError;
}

describe("env create flags", () => {
  const post = (calls: ReturnType<typeof stubFetch>) =>
    JSON.parse(calls.find((c) => c.method === "POST" && c.path === ENVS)?.body ?? "null");

  it("sends only the name when no flag is given", async () => {
    const calls = stubFetch(TARGET, {
      ...APPS,
      [`POST ${ENVS}`]: () => ({ status: 201, body: ENVIRONMENT })
    });
    await envCreate(fakeContext(), "acme/store", "dev-a1");
    expect(post(calls)).toEqual({ name: "dev-a1" });
  });

  it("sends from, at (as an ISO instant), preset and planes", async () => {
    const calls = stubFetch(TARGET, {
      ...APPS,
      [`POST ${ENVS}`]: () => ({ status: 201, body: ENVIRONMENT })
    });
    const out = capture();
    await runEnvCreate(fakeContext(), "acme/store", "dev-a1", {
      json: true,
      from: "staging",
      at: "2026-11-01",
      preset: "full",
      plane: ["storage=own", "oltp=schema"]
    });
    out.restore();
    expect(post(calls)).toEqual({
      name: "dev-a1",
      from: "staging",
      at: "2026-11-01T00:00:00.000Z",
      preset: "full",
      planes: { storage: "own", oltp: "schema" }
    });
  });

  it("passes --plane semantic=branch:<b> through whole, colon and all", async () => {
    const calls = stubFetch(TARGET, {
      ...APPS,
      [`POST ${ENVS}`]: () => ({ status: 201, body: ENVIRONMENT })
    });
    const out = capture();
    await runEnvCreate(fakeContext(), "acme/store", "dev-a1", {
      json: true,
      plane: ["semantic=branch:feat-x", "workspace-actions=preview", "semantic=branch:feat/y"]
    });
    out.restore();
    // The first `=` splits; the value keeps every later `:` and `/`, and the
    // last of a repeated key wins.
    expect(post(calls)).toEqual({
      name: "dev-a1",
      planes: { semantic: "branch:feat/y", "workspace-actions": "preview" }
    });
  });

  it("prints a branch-pinned semantic plane as set", async () => {
    stubFetch(TARGET, {
      ...APPS,
      [`POST ${ENVS}`]: () => ({
        status: 201,
        body: {
          ...ENVIRONMENT,
          planes: { build: "own", semantic: "branch" },
          planes_set: { semantic: "branch:feat-x" }
        }
      })
    });
    const out = capture();
    await runEnvCreate(fakeContext(), "acme/store", "dev-a1", {
      json: false,
      plane: ["semantic=branch:feat-x"]
    });
    out.restore();
    expect(out.printed()).toContain("  build=own semantic=branch\n");
    expect(out.printed()).toContain("  set: semantic=branch:feat-x\n");
  });

  it("sends an OLTP branch with its fork point: --plane oltp=branch --at <ts>", async () => {
    const calls = stubFetch(TARGET, {
      ...APPS,
      [`POST ${ENVS}`]: () => ({ status: 201, body: ENVIRONMENT })
    });
    const out = capture();
    await runEnvCreate(fakeContext(), "acme/store", "dev-a1", {
      json: true,
      at: "2026-11-01T09:00:00+01:00",
      plane: ["oltp=branch"]
    });
    out.restore();
    expect(post(calls)).toEqual({
      name: "dev-a1",
      at: "2026-11-01T08:00:00.000Z",
      planes: { oltp: "branch" }
    });
  });

  it("surfaces the server's refusal of an OLTP branch (400 invalid_planes) as a request error", async () => {
    stubFetch(TARGET, {
      ...APPS,
      [`POST ${ENVS}`]: () => ({
        status: 400,
        body: {
          error: "invalid_planes",
          message: "this org has 8 sandbox OLTP branches; use --plane oltp=schema"
        }
      })
    });
    const err = await envCreate(fakeContext(), "acme/store", "dev-a1", {
      planes: { oltp: "branch" }
    }).catch((e: unknown) => e as CliError);
    expect((err as CliError).code).toBe(ExitCode.REQUEST);
    expect((err as CliError).detail).toContain("oltp=schema");
  });

  it("refuses a bad --preset, --at, --from or --plane before any request (exit 2)", async () => {
    const calls = stubFetch(TARGET, {});
    const run = (opts: Parameters<typeof runEnvCreate>[3]) =>
      runEnvCreate(fakeContext(), "acme/store", "dev-a1", opts);
    expect((await usageOf(run({ json: true, preset: "heavy" }))).message).toMatch(/not a preset/);
    expect((await usageOf(run({ json: true, at: "last tuesday-ish" }))).message).toMatch(
      /not a date/
    );
    await usageOf(run({ json: true, from: "Not Valid" }));
    expect((await usageOf(run({ json: true, plane: ["storage"] }))).message).toMatch(
      /not <key>=<mode>/
    );
    expect(calls).toEqual([]);
  });

  it("prints the explicitly set planes under the resolved ones", async () => {
    stubFetch(TARGET, {
      ...APPS,
      [`POST ${ENVS}`]: () => ({ status: 201, body: ENVIRONMENT })
    });
    const out = capture();
    await runEnvCreate(fakeContext(), "acme/store", "dev-a1", { json: false });
    out.restore();
    expect(out.printed()).toContain("  build=own oltp=shared storage=own\n");
    expect(out.printed()).toContain("  set: storage=own\n");
  });
});

describe("env update", () => {
  const PLANES = `${ENVS}/dev-a1/planes`;
  const put = (calls: ReturnType<typeof stubFetch>) =>
    JSON.parse(calls.find((c) => c.method === "PUT")?.body ?? "null");
  const SET = { storage: "own", oltp: "schema" };
  const routes = (planesSet: unknown = SET) => ({
    ...APPS,
    [`GET ${ENVS}/dev-a1`]: () => ({
      status: 200,
      body: { ...ENVIRONMENT, planes_set: planesSet }
    }),
    [`PUT ${PLANES}`]: () => ({ status: 200, body: ENVIRONMENT })
  });

  it("merges the flags into planes_set and PUTs the whole object", async () => {
    const calls = stubFetch(TARGET, routes());
    await envUpdate(fakeContext(), "acme/store", "dev-a1", { planes: { airhouse: "own" } });
    expect(put(calls)).toEqual({ planes: { storage: "own", oltp: "schema", airhouse: "own" } });
  });

  it("sets, repoints and clears a semantic branch with the same flags", async () => {
    const set = { storage: "own", semantic: "branch:feat-x" };
    const calls = stubFetch(TARGET, routes(set));
    await envUpdate(fakeContext(), "acme/store", "dev-a1", {
      planes: { semantic: "branch:feat-z", "workspace-actions": "preview" }
    });
    expect(put(calls)).toEqual({
      planes: { storage: "own", semantic: "branch:feat-z", "workspace-actions": "preview" }
    });
    // `semantic=default` drops the key, which the server reads as build-pin
    // and answers by clearing the pin; `semantic=build-pin` says it outright.
    const cleared = stubFetch(TARGET, routes(set));
    await envUpdate(fakeContext(), "acme/store", "dev-a1", { planes: { semantic: "default" } });
    expect(put(cleared)).toEqual({ planes: { storage: "own" } });
    const explicit = stubFetch(TARGET, routes(set));
    await envUpdate(fakeContext(), "acme/store", "dev-a1", { planes: { semantic: "build-pin" } });
    expect(put(explicit)).toEqual({ planes: { storage: "own", semantic: "build-pin" } });
  });

  it("lets a flag win over what is set, and key=default deletes it", async () => {
    const calls = stubFetch(TARGET, routes());
    await envUpdate(fakeContext(), "acme/store", "dev-a1", {
      planes: { oltp: "default", storage: "shared" }
    });
    expect(put(calls)).toEqual({ planes: { storage: "shared" } });
  });

  it("treats an environment with nothing set (or an older server) as empty", async () => {
    for (const set of [{}, null]) {
      const calls = stubFetch(TARGET, routes(set));
      await envUpdate(fakeContext(), "acme/store", "dev-a1", { planes: { storage: "own" } });
      expect(put(calls)).toEqual({ planes: { storage: "own" } });
    }
  });

  it("treats a GET with no planes_set field (an older server) as nothing set", async () => {
    const { planes_set: _omitted, ...older } = ENVIRONMENT;
    const calls = stubFetch(TARGET, {
      ...routes(),
      [`GET ${ENVS}/dev-a1`]: () => ({ status: 200, body: older })
    });
    await envUpdate(fakeContext(), "acme/store", "dev-a1", { planes: { storage: "own" } });
    expect(put(calls)).toEqual({ planes: { storage: "own" } });
  });

  it("--replace sends exactly the flags (minus =default) without reading the environment", async () => {
    const calls = stubFetch(TARGET, routes());
    await envUpdate(fakeContext(), "acme/store", "dev-a1", {
      planes: { airhouse: "own", storage: "default" },
      replace: true
    });
    expect(put(calls)).toEqual({ planes: { airhouse: "own" } });
    expect(calls.filter((c) => c.method === "GET" && c.path.endsWith("/dev-a1"))).toEqual([]);
  });

  it("works on staging and prints the returned environment like env show", async () => {
    const calls = stubFetch(TARGET, {
      ...APPS,
      [`GET ${ENVS}/staging`]: () => ({ status: 200, body: { ...ENVIRONMENT, planes_set: {} } }),
      [`PUT ${ENVS}/staging/planes`]: () => ({ status: 200, body: ENVIRONMENT })
    });
    const out = capture();
    await runEnvUpdate(fakeContext(), "acme/store", "staging", {
      plane: ["storage=own"],
      json: false
    });
    out.restore();
    expect(put(calls)).toEqual({ planes: { storage: "own" } });
    expect(out.printed().split("\n")[0]).toBe("dev-a1 (dev) — active, no build yet");
    expect(out.printed()).toContain("  set: storage=own\n");
  });

  it("needs at least one well-formed --plane, before any request (exit 2)", async () => {
    const calls = stubFetch(TARGET, routes());
    const none = await usageOf(runEnvUpdate(fakeContext(), "acme/store", "dev-a1", { json: true }));
    expect(none.message).toMatch(/at least one --plane/);
    await usageOf(
      runEnvUpdate(fakeContext(), "acme/store", "dev-a1", { plane: ["storage="], json: true })
    );
    await usageOf(
      runEnvUpdate(fakeContext(), "acme/store", "dev-a1", { plane: ["=own"], json: true })
    );
    await usageOf(
      runEnvUpdate(fakeContext(), "acme/store", "not valid!", { plane: ["a=b"], json: true })
    );
    expect(calls).toEqual([]);
  });

  it("refuses a sandbox agent token outside a dev-<handle> sandbox, and a publish token", async () => {
    const calls = stubFetch(TARGET, routes());
    await usageOf(
      envUpdate(fakeContext("oxy_sbx_abc"), "acme/store", "staging", { planes: { a: "b" } })
    );
    await usageOf(
      envUpdate(fakeContext("oxypublish_stored"), "acme/store", "dev-a1", { planes: { a: "b" } })
    );
    expect(calls).toEqual([]);
  });

  it("maps the server's 409 and 400 to exit 6", async () => {
    stubFetch(TARGET, {
      ...APPS,
      [`GET ${ENVS}/production`]: () => ({
        status: 200,
        body: { ...ENVIRONMENT, planes_set: null }
      }),
      [`PUT ${ENVS}/production/planes`]: () => ({
        status: 409,
        body: { error: "production_has_no_planes", message: "production has no planes" }
      }),
      [`GET ${ENVS}/dev-a1`]: () => ({ status: 200, body: ENVIRONMENT }),
      [`PUT ${PLANES}`]: () => ({
        status: 400,
        body: { error: "invalid_planes", message: "unknown plane nope" }
      })
    });
    const prod = await envUpdate(fakeContext(), "acme/store", "production", {
      planes: { storage: "own" }
    }).catch((e: unknown) => e as CliError);
    expect((prod as CliError).code).toBe(ExitCode.REQUEST);
    expect((prod as CliError).detail).toContain("production_has_no_planes");
    const bad = await envUpdate(fakeContext(), "acme/store", "dev-a1", {
      planes: { nope: "own" }
    }).catch((e: unknown) => e as CliError);
    expect((bad as CliError).detail).toContain("invalid_planes");
  });
});

describe("env reset", () => {
  const RESET = `${ENVS}/dev-a1/reset`;
  const ANSWER = {
    name: "dev-a1",
    planes: [
      { plane: "storage", status: "done", run_id: null },
      { plane: "oltp", status: "queued", run_id: "run-9" }
    ]
  };
  const body = (calls: ReturnType<typeof stubFetch>) =>
    JSON.parse(calls.find((c) => c.method === "POST" && c.path === RESET)?.body ?? "null");

  it("POSTs the keys — comma lists and repeats alike, de-duplicated", async () => {
    const calls = stubFetch(TARGET, {
      ...APPS,
      [`POST ${RESET}`]: () => ({ status: 200, body: ANSWER })
    });
    const out = capture();
    await runEnvReset(fakeContext(), "acme/store", "dev-a1", {
      plane: ["storage,oltp", "storage"],
      json: true
    });
    out.restore();
    expect(body(calls)).toEqual({ planes: ["storage", "oltp"] });
    expect(JSON.parse(out.printed())).toEqual(ANSWER);
  });

  it("sends --at with the keys, as an ISO instant, and refuses a bad one before any request", async () => {
    const calls = stubFetch(TARGET, {
      ...APPS,
      [`POST ${RESET}`]: () => ({ status: 200, body: ANSWER })
    });
    const out = capture();
    await runEnvReset(fakeContext(), "acme/store", "dev-a1", {
      plane: ["oltp"],
      at: "2026-11-01T09:00:00Z",
      json: true
    });
    out.restore();
    expect(body(calls)).toEqual({ planes: ["oltp"], at: "2026-11-01T09:00:00.000Z" });

    const none = stubFetch(TARGET, {});
    const bad = envReset(fakeContext(), "acme/store", "dev-a1", ["oltp"], "last tuesday-ish");
    expect((await usageOf(bad)).message).toMatch(/not a date/);
    expect(none).toEqual([]);
  });

  it("sends storage, secrets and messages as the planes to reset, and prints each as done", async () => {
    const answer = {
      name: "dev-a1",
      planes: ["storage", "secrets", "messages"].map((plane) => ({
        plane,
        status: "done",
        run_id: null
      }))
    };
    const calls = stubFetch(TARGET, {
      ...APPS,
      [`POST ${RESET}`]: () => ({ status: 200, body: answer })
    });
    const out = capture();
    await runEnvReset(fakeContext(), "acme/store", "dev-a1", {
      plane: ["storage,secrets", "messages"],
      json: false
    });
    out.restore();
    expect(body(calls)).toEqual({ planes: ["storage", "secrets", "messages"] });
    expect(out.printed()).toBe(
      "dev-a1 — reset\n  storage: done\n  secrets: done\n  messages: done\n"
    );
  });

  it("prints each plane's status and run id", async () => {
    stubFetch(TARGET, { ...APPS, [`POST ${RESET}`]: () => ({ status: 200, body: ANSWER }) });
    const out = capture();
    await runEnvReset(fakeContext(), "acme/store", "dev-a1", {
      plane: ["storage,oltp"],
      json: false
    });
    out.restore();
    expect(out.printed()).toBe("dev-a1 — reset\n  storage: done\n  oltp: queued (run run-9)\n");
    expect(describeReset(ANSWER)).toEqual(["storage: done", "oltp: queued (run run-9)"]);
  });

  it("needs a plane, before any request (exit 2)", async () => {
    const calls = stubFetch(TARGET, {});
    await usageOf(runEnvReset(fakeContext(), "acme/store", "dev-a1", { json: true }));
    await usageOf(envReset(fakeContext(), "acme/store", "dev-a1", [" , "]));
    await usageOf(envReset(fakeContext(), "acme/store", "dev-a1", ["Bad Key"]));
    await usageOf(envReset(fakeContext("oxy_sbx_abc"), "acme/store", "staging", ["storage"]));
    expect(calls).toEqual([]);
  });

  it("maps plane_not_resettable (409) to exit 6", async () => {
    stubFetch(TARGET, {
      ...APPS,
      [`POST ${RESET}`]: () => ({
        status: 409,
        body: { error: "plane_not_resettable", message: "storage cannot be reset yet" }
      })
    });
    const err = await envReset(fakeContext(), "acme/store", "dev-a1", ["storage"]).catch(
      (e: unknown) => e as CliError
    );
    expect((err as CliError).code).toBe(ExitCode.REQUEST);
    expect((err as CliError).detail).toContain("plane_not_resettable");
  });
});

describe("plane argument helpers", () => {
  it("parsePlaneFlags splits on the first =, keeps the last of a repeated key", () => {
    expect(parsePlaneFlags(["a=b", "a=c", "x-y=z"])).toEqual({ a: "c", "x-y": "z" });
  });

  it("parsePlaneFlags keeps semantic=branch:<b> whole, including a nested name", () => {
    expect(parsePlaneFlags(["semantic=branch:feat-x"])).toEqual({ semantic: "branch:feat-x" });
    expect(parsePlaneFlags(["semantic=branch:release/1.2=rc"])).toEqual({
      semantic: "branch:release/1.2=rc"
    });
    expect(describePlanesSet({ semantic: "branch:feat-x", "workspace-actions": "preview" })).toBe(
      "semantic=branch:feat-x workspace-actions=preview"
    );
  });

  it("validatePlaneMap accepts mode strings and {mode} objects only", () => {
    expect(validatePlaneMap({ a: "own", b: { mode: "mapped" } })).toEqual({
      a: "own",
      b: { mode: "mapped" }
    });
    for (const bad of [null, [], "x", { a: 1 }, { a: "" }, { a: null }, { A: "x" }]) {
      expect(() => validatePlaneMap(bad)).toThrow(CliError);
    }
  });

  it("mergePlanes: merge, default deletes, replace ignores current", () => {
    expect(mergePlanes({ a: "1", b: "2" }, { b: "default", c: "3" }, false)).toEqual({
      a: "1",
      c: "3"
    });
    expect(mergePlanes({ a: "1" }, { b: "2", c: "default" }, true)).toEqual({ b: "2" });
    expect(mergePlanes(null, { a: "1" }, false)).toEqual({ a: "1" });
  });

  it("parsePlaneKeys and requireForkAt", () => {
    expect(parsePlaneKeys(["a, b", "c"])).toEqual(["a", "b", "c"]);
    expect(requireForkAt("2026-11-01T09:00:00+02:00")).toBe("2026-11-01T07:00:00.000Z");
  });

  it("describePlanesSet is null for nothing set", () => {
    expect(describePlanesSet({})).toBeNull();
    expect(describePlanesSet(null)).toBeNull();
    expect(describePlanesSet({ a: "x", b: { mode: "mapped" } })).toBe("a=x b=mapped…");
  });
});
