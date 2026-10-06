/**
 * Revoke-on-exit is cleanup, and cleanup has one rule: it must never be the
 * reason a command fails. Each case here is a way the revoke can go wrong —
 * the server refuses, the network is gone — and the assertion is the same
 * every time: nothing throws.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { stubFetch } from "../testing/stub-fetch.js";
import { keepPastExit, pendingRevokes, revokeOnExit, runExitRevokes } from "./exit-revoke.js";
import { revokeCallingToken } from "./token-api.js";

const TARGET = "https://oxy.test";

afterEach(async () => {
  vi.unstubAllGlobals();
  // Drain whatever a failed case left queued, against no network at all.
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response(null, { status: 204 }))
  );
  await runExitRevokes();
  vi.unstubAllGlobals();
});

describe("runExitRevokes", () => {
  it("revokes each queued token with `DELETE /api/auth/token`, as that token", async () => {
    const calls = stubFetch(TARGET, { "DELETE /api/auth/token": () => ({ status: 204 }) });
    revokeOnExit(TARGET, "oxy_ci_one");
    revokeOnExit(TARGET, "oxy_ci_two");

    await runExitRevokes();

    expect(calls.map((c) => `${c.method} ${c.path} ${c.headers.authorization}`).sort()).toEqual([
      "DELETE /api/auth/token Bearer oxy_ci_one",
      "DELETE /api/auth/token Bearer oxy_ci_two"
    ]);
    expect(pendingRevokes()).toEqual([]);
  });

  it("revokes once: a second run has nothing left to do", async () => {
    const calls = stubFetch(TARGET, { "DELETE /api/auth/token": () => ({ status: 204 }) });
    revokeOnExit(TARGET, "oxy_ci_one");
    await runExitRevokes();
    await runExitRevokes();
    expect(calls).toHaveLength(1);
  });

  it("leaves alone a token the caller asked to keep", async () => {
    const calls = stubFetch(TARGET, { "DELETE /api/auth/token": () => ({ status: 204 }) });
    revokeOnExit(TARGET, "oxy_ci_printed");
    keepPastExit("oxy_ci_printed");
    await runExitRevokes();
    expect(calls).toHaveLength(0);
  });

  it("does nothing, and makes no request, in a process that minted nothing", async () => {
    const calls = stubFetch(TARGET, {});
    await runExitRevokes();
    expect(calls).toHaveLength(0);
  });

  it("never throws when the server refuses the revoke", async () => {
    stubFetch(TARGET, { "DELETE /api/auth/token": () => ({ status: 500, body: { error: "x" } }) });
    revokeOnExit(TARGET, "oxy_ci_one");
    await expect(runExitRevokes()).resolves.toBeUndefined();
  });

  it("never throws when the network is gone", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("fetch failed");
      })
    );
    revokeOnExit(TARGET, "oxy_ci_one");
    await expect(runExitRevokes()).resolves.toBeUndefined();
  });
});

describe("revokeCallingToken", () => {
  const outcome = async (status: number, body?: unknown) => {
    stubFetch(TARGET, { "DELETE /api/auth/token": () => ({ status, body }) });
    return revokeCallingToken(TARGET, "tok");
  };

  it("maps each answer the contract defines", async () => {
    expect(await outcome(204)).toBe("revoked");
    // A legacy key: only its owner ends it, in the web app.
    expect(await outcome(409, { code: "legacy_immutable" })).toBe("legacy");
    // A session — or a deployment with no such route.
    expect(await outcome(404, { code: "no_token" })).toBe("unsupported");
    // Already dead, which is where a revoke was headed anyway.
    expect(await outcome(401)).toBe("already_invalid");
    expect(await outcome(503)).toBe("failed");
  });

  it("answers 'unsupported' against a deployment that predates the route", async () => {
    stubFetch(TARGET, {});
    expect(await revokeCallingToken(TARGET, "tok")).toBe("unsupported");
  });
});
