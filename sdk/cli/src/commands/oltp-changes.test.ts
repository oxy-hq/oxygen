/**
 * `oxyc oltp changes resync` — the request it makes, what it prints, and its
 * refusals, driven with `globalThis.fetch` stubbed.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { stubFetch } from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import { type ResyncReport, runOltpChangesResync } from "./oltp-changes.js";

const TARGET = "https://oxy.test";
const APP_ID = "a1a1a1a1-2222-3333-4444-555555555555";
const RESYNC = `/api/customer-apps/${APP_ID}/oltp-changes/resync`;

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

const REPORT: ResyncReport = {
  writer: "app_store",
  requested: ["leases"],
  released: ["leases"],
  discarded: [{ table: "leases", rows: 2 }]
};

const ROUTES = {
  "GET /api/customer-apps?limit=100&offset=0": () => ({
    status: 200,
    body: { items: [{ id: APP_ID, slug: "store", org_slug: "acme" }], next_offset: null }
  }),
  [`POST ${RESYNC}`]: () => ({ status: 200, body: REPORT })
};

afterEach(() => vi.unstubAllGlobals());

function capture(): { printed: () => string; restore: () => void } {
  let printed = "";
  const out = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
    printed += String(chunk);
    return true;
  });
  const errs = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
  return {
    printed: () => printed,
    restore: () => {
      out.mockRestore();
      errs.mockRestore();
    }
  };
}

describe("oltp changes resync", () => {
  it("refuses without --yes and sends nothing", async () => {
    const calls = stubFetch(TARGET, ROUTES);
    const err = (await runOltpChangesResync(fakeContext(), "acme/store", {
      table: "leases"
    }).catch((e: unknown) => e)) as CliError;
    expect(err).toBeInstanceOf(CliError);
    expect(err.code).toBe(ExitCode.REFUSED);
    expect(calls).toEqual([]);
  });

  it("posts table and environment to the app's resync route", async () => {
    const calls = stubFetch(TARGET, ROUTES);
    const out = capture();
    await runOltpChangesResync(fakeContext(), "acme/store", { table: "leases", yes: true });
    out.restore();
    const post = calls.find((c) => c.method === "POST");
    expect(post?.path).toBe(RESYNC);
    expect(JSON.parse(post?.body ?? "")).toEqual({ table: "leases", environment: "production" });
  });

  it("prints each discarded table and its rows", async () => {
    stubFetch(TARGET, ROUTES);
    const out = capture();
    await runOltpChangesResync(fakeContext(), "acme/store", { yes: true });
    out.restore();
    expect(out.printed()).toContain("requested a baseline of: leases");
    expect(out.printed()).toContain("released holds: leases");
    expect(out.printed()).toContain("    leases: 2");
  });

  it("--app-env staging is a usage error", async () => {
    const calls = stubFetch(TARGET, ROUTES);
    const err = (await runOltpChangesResync(fakeContext(), "acme/store", {
      env: "staging",
      yes: true
    }).catch((e: unknown) => e)) as CliError;
    expect(err).toBeInstanceOf(CliError);
    expect(err.code).toBe(ExitCode.USAGE);
    expect(calls).toEqual([]);
  });
});
