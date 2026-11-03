/**
 * `oxyc env outbox` — the request it makes, how it prints, and how the
 * server's refusals surface, driven with `globalThis.fetch` stubbed.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { stubFetch } from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import { describeMessage, envOutbox, type OutboxMessage, runEnvOutbox } from "./env-outbox.js";

const TARGET = "https://oxy.test";
const APP_ID = "a1a1a1a1-2222-3333-4444-555555555555";
const OUTBOX = `/api/customer-apps/${APP_ID}/environments/dev-a1/outbox`;

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

const EMAIL: OutboxMessage = {
  kind: "email",
  to: ["cfo@customer.example"],
  subject: "JE posted",
  body: "<p>posted</p>",
  invocation_id: "11111111-2222-3333-4444-555555555555",
  function: "send",
  captured_at: "2026-11-03T12:00:00.000Z",
  format: "html"
};

const NOTIFY: OutboxMessage = {
  kind: "notify",
  to: [{ userId: "u-1" }],
  title: "Swap requested",
  body: "Dana asked to swap",
  invocation_id: "11111111-2222-3333-4444-555555555556",
  function: "swap",
  captured_at: "2026-11-03T11:00:00.000Z",
  notify_kind: "shift_swap"
};

afterEach(() => vi.unstubAllGlobals());

function capture(): { printed: () => string; err: () => string; restore: () => void } {
  let printed = "";
  let err = "";
  const out = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
    printed += String(chunk);
    return true;
  });
  const errs = vi.spyOn(process.stderr, "write").mockImplementation((chunk) => {
    err += String(chunk);
    return true;
  });
  return {
    printed: () => printed,
    err: () => err,
    restore: () => {
      out.mockRestore();
      errs.mockRestore();
    }
  };
}

describe("env outbox", () => {
  it("GETs …/outbox with no query by default, and returns the server's page", async () => {
    const calls = stubFetch(TARGET, {
      ...APPS,
      [`GET ${OUTBOX}`]: () => ({ status: 200, body: { messages: [EMAIL], has_more: false } })
    });
    const outbox = await envOutbox(fakeContext(), "acme/store", "dev-a1");
    expect(outbox).toEqual({ messages: [EMAIL], has_more: false });
    expect(calls.some((c) => c.method === "GET" && c.path === OUTBOX)).toBe(true);
  });

  it("sends --limit as ?limit=", async () => {
    const calls = stubFetch(TARGET, {
      ...APPS,
      [`GET ${OUTBOX}?limit=7`]: () => ({ status: 200, body: { messages: [], has_more: true } })
    });
    const outbox = await envOutbox(fakeContext(), "acme/store", "dev-a1", { limit: 7 });
    expect(outbox.has_more).toBe(true);
    expect(calls.some((c) => c.path === `${OUTBOX}?limit=7`)).toBe(true);
  });

  it("refuses a bad --limit or environment name before any request (exit 2)", async () => {
    const calls = stubFetch(TARGET, {});
    for (const run of [
      () => envOutbox(fakeContext(), "acme/store", "dev-a1", { limit: 0 }),
      () => envOutbox(fakeContext(), "acme/store", "dev-a1", { limit: Number.NaN }),
      () => envOutbox(fakeContext(), "acme/store", "Not Valid")
    ]) {
      const err = (await run().catch((e: unknown) => e)) as CliError;
      expect(err).toBeInstanceOf(CliError);
      expect(err.code).toBe(ExitCode.USAGE);
    }
    expect(calls).toEqual([]);
  });

  it("--json prints the server's page as one line", async () => {
    stubFetch(TARGET, {
      ...APPS,
      [`GET ${OUTBOX}`]: () => ({ status: 200, body: { messages: [NOTIFY], has_more: false } })
    });
    const out = capture();
    await runEnvOutbox(fakeContext(), "acme/store", "dev-a1", { json: true });
    out.restore();
    expect(JSON.parse(out.printed())).toEqual({ messages: [NOTIFY], has_more: false });
  });

  it("prints a headline and the body of each message, and says when older ones exist", async () => {
    stubFetch(TARGET, {
      ...APPS,
      [`GET ${OUTBOX}`]: () => ({
        status: 200,
        body: { messages: [EMAIL, NOTIFY], has_more: true }
      })
    });
    const out = capture();
    await runEnvOutbox(fakeContext(), "acme/store", "dev-a1", { json: false });
    out.restore();
    expect(out.printed()).toBe(
      [
        "2026-11-03T12:00:00.000Z  email  send  → cfo@customer.example",
        "  JE posted",
        "    <p>posted</p>",
        '2026-11-03T11:00:00.000Z  notify  swap  → {"userId":"u-1"}',
        "  Swap requested",
        "    Dana asked to swap",
        "(older messages exist — raise --limit)",
        ""
      ].join("\n")
    );
    expect(describeMessage(NOTIFY)[1]).toBe("  Swap requested");
  });

  it("says so on stderr when nothing was captured", async () => {
    stubFetch(TARGET, {
      ...APPS,
      [`GET ${OUTBOX}`]: () => ({ status: 200, body: { messages: [], has_more: false } })
    });
    const out = capture();
    await runEnvOutbox(fakeContext(), "acme/store", "dev-a1", { json: false });
    out.restore();
    expect(out.printed()).toBe("");
    expect(out.err()).toBe("no captured messages\n");
  });

  it("maps production's 409 to exit 6, naming the code", async () => {
    stubFetch(TARGET, {
      ...APPS,
      [`GET /api/customer-apps/${APP_ID}/environments/production/outbox`]: () => ({
        status: 409,
        body: { error: "production_has_no_planes", message: "production has no planes" }
      })
    });
    const err = (await envOutbox(fakeContext(), "acme/store", "production").catch(
      (e: unknown) => e
    )) as CliError;
    expect(err.code).toBe(ExitCode.REQUEST);
    expect(err.detail).toContain("production_has_no_planes");
  });

  it("an older server (no such route) is a request error, not an empty outbox", async () => {
    stubFetch(TARGET, { ...APPS });
    const err = (await envOutbox(fakeContext(), "acme/store", "dev-a1").catch(
      (e: unknown) => e
    )) as CliError;
    expect(err).toBeInstanceOf(CliError);
  });
});
