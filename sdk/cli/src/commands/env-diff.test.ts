/**
 * `oxyc env diff` — the request, the human rendering and the usage errors,
 * driven with `globalThis.fetch` stubbed.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { stubFetch } from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import { describeEnvDiff, envDiff, runEnvDiff } from "./env-diff.js";

const TARGET = "https://oxy.test";
const APP_ID = "a1a1a1a1-2222-3333-4444-555555555555";
const DIFF_PATH = `/api/customer-apps/${APP_ID}/environments/dev-a1/diff`;

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

const OLTP = {
  schema: "app_store",
  tables_added: ["refunds"],
  tables_removed: [],
  columns_added: ["orders.note text"],
  columns_removed: ["orders.total"],
  columns_changed: ["orders.qty: integer → bigint"]
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

describe("env diff", () => {
  it("renders an added column as `+ orders.note text`, and the rest by sign", async () => {
    stubFetch(TARGET, {
      ...APPS,
      [`GET ${DIFF_PATH}`]: () => ({
        status: 200,
        body: { environment: "dev-a1", planes: { oltp: OLTP } }
      })
    });
    const out = capture();
    await runEnvDiff(fakeContext(), "acme/store", "dev-a1", { json: false });
    out.restore();
    expect(out.printed()).toBe(
      [
        "dev-a1 — oltp, schema app_store, against production",
        "  + table refunds",
        "  + orders.note text",
        "  - orders.total",
        "  ~ orders.qty: integer → bigint",
        ""
      ].join("\n")
    );
  });

  it("says so when the schemas match", () => {
    const same = {
      ...OLTP,
      tables_added: [],
      columns_added: [],
      columns_removed: [],
      columns_changed: []
    };
    expect(describeEnvDiff({ environment: "dev-a1", planes: { oltp: same } })).toEqual([
      "dev-a1 — oltp, schema app_store, against production",
      "  no difference"
    ]);
  });

  it("says so when the environment has no branch of its own", async () => {
    stubFetch(TARGET, {
      ...APPS,
      [`GET ${DIFF_PATH}`]: () => ({ status: 200, body: { environment: "dev-a1", planes: {} } })
    });
    const out = capture();
    await runEnvDiff(fakeContext(), "acme/store", "dev-a1", { json: false });
    out.restore();
    expect(out.printed()).toContain("dev-a1 has no plane on a branch of its own");
  });

  it("--json prints the response as one line", async () => {
    const body = { environment: "dev-a1", planes: { oltp: OLTP } };
    stubFetch(TARGET, { ...APPS, [`GET ${DIFF_PATH}`]: () => ({ status: 200, body }) });
    const out = capture();
    await runEnvDiff(fakeContext(), "acme/store", "dev-a1", { json: true, plane: ["oltp"] });
    out.restore();
    expect(JSON.parse(out.printed())).toEqual(body);
  });

  it("refuses an unknown --plane or a bad name before any request (exit 2)", async () => {
    const calls = stubFetch(TARGET, {});
    for (const run of [
      envDiff(fakeContext(), "acme/store", "dev-a1", ["storage"]),
      envDiff(fakeContext(), "acme/store", "Not Valid")
    ]) {
      const err = await run.catch((e: unknown) => e);
      expect(err).toBeInstanceOf(CliError);
      expect((err as CliError).code).toBe(ExitCode.USAGE);
    }
    expect(calls).toEqual([]);
  });

  it("surfaces a branch that is not ready as a request error with the server's fix", async () => {
    stubFetch(TARGET, {
      ...APPS,
      [`GET ${DIFF_PATH}`]: () => ({
        status: 409,
        body: {
          error: "oltp_branch_unready",
          message: "this sandbox's OLTP branch has not been cut yet"
        }
      })
    });
    const err = await envDiff(fakeContext(), "acme/store", "dev-a1").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(CliError);
    const shown = err as CliError;
    expect(`${shown.message} ${shown.detail ?? ""}`).toContain("has not been cut");
  });
});
