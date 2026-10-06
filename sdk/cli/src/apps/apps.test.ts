/**
 * `src/apps/environment.ts` and `src/apps/resolve.ts` — the two helpers every
 * sandbox-aware command shares.
 *
 * `environment.ts` is pure, so its grammar cases are pinned straight from the
 * Rust test list (`crates/app-core/src/custom_app_environment.rs`): the two
 * lists must accept and reject exactly the same names, since the database
 * carries the same CHECK constraint.
 *
 * `resolve.ts` is driven with `globalThis.fetch` stubbed, the way
 * `commands/checks.test.ts` drives `runChecks` — a route table, not a server.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import { isProduction, parseAppEnv, requireSandboxName } from "./environment.js";
import { type Creds, ensureOk, resolveApp, staffCreds } from "./resolve.js";

describe("parseAppEnv", () => {
  it.each(["production", "staging", "dev-luong", "dev-a1-b2", "dev-abcdefghijkl"])(
    "accepts %s",
    (name) => {
      expect(parseAppEnv(name)).toBe(name);
    }
  );

  // The Rust rejection list, `custom_app_environment.rs::rejects_malformed_names`,
  // plus the 13-character handle from `a_12_character_handle_is_the_longest_allowed`.
  it.each([
    "",
    "prod",
    "Production",
    "dev",
    "dev-",
    "dev--x",
    "dev-x-",
    "dev-UPPER",
    "dev-a--b",
    "dev-a_b",
    "dev-abcdefghijklm"
  ])("rejects %s with a usage error (exit 2)", (name) => {
    expect(() => parseAppEnv(name)).toThrow(CliError);
    try {
      parseAppEnv(name);
      expect.unreachable();
    } catch (e) {
      expect((e as CliError).code).toBe(ExitCode.USAGE);
    }
  });
});

describe("requireSandboxName", () => {
  it("passes a dev-<handle> through unchanged", () => {
    expect(requireSandboxName("dev-a1")).toBe("dev-a1");
  });

  it.each(["production", "staging"])("refuses %s — not a sandbox (exit 2)", (name) => {
    const err = (() => {
      try {
        requireSandboxName(name);
        return undefined;
      } catch (e) {
        return e as CliError;
      }
    })();
    expect(err).toBeInstanceOf(CliError);
    expect(err?.code).toBe(ExitCode.USAGE);
    expect(err?.message).toContain("not a sandbox");
  });

  it("refuses a malformed handle the same way parseAppEnv does", () => {
    expect(() => requireSandboxName("dev-UPPER")).toThrow(CliError);
  });
});

describe("isProduction", () => {
  it("is true for undefined (no --app-env) and for production", () => {
    expect(isProduction(undefined)).toBe(true);
    expect(isProduction("production")).toBe(true);
  });

  it("is false for staging and for a sandbox", () => {
    expect(isProduction("staging")).toBe(false);
    expect(isProduction("dev-a1")).toBe(false);
  });
});

// ── resolve.ts ──────────────────────────────────────────────────────────

const TARGET = "https://oxy.test";
const APP_ID = "a1a1a1a1-2222-3333-4444-555555555555";

function fakeContext(opts: { bearer?: string; apiKey?: string } = {}): Context {
  return {
    cwd: "/tmp",
    flags: { env: "production", tokenEnv: "OXY_TOKEN", apiKeyEnv: "OXY_API_KEY" },
    target: () => TARGET,
    env: () => ({ target: TARGET, orgSlug: undefined }) as ReturnType<Context["env"]>,
    bearer: async () => {
      if (opts.bearer) return opts.bearer;
      throw new CliError(`not authenticated for ${TARGET}`, {
        code: ExitCode.AUTH,
        hint: "oxyc login --env production"
      });
    },
    maybeBearer: async () => opts.bearer,
    storedBearer: () => opts.bearer,
    async credential() {
      return { token: await this.bearer(), source: "env" };
    },
    serviceAccount: () => undefined,
    apiKey: () => opts.apiKey,
    customer: () => undefined,
    repoDir: () => undefined,
    placeholders: () => ({}),
    withEnv: () => fakeContext(opts)
  };
}

type RouteTable = Record<
  string,
  (init: RequestInit | undefined) => { status: number; body: unknown }
>;

function stubFetch(routes: RouteTable, calls: { method: string; url: string }[]) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string | URL, init?: RequestInit) => {
      const url = String(input);
      const path = url.replace(TARGET, "");
      const method = (init?.method ?? "GET").toUpperCase();
      calls.push({ method, url });
      const key = `${method} ${path}`;
      const handler = routes[key];
      if (!handler)
        return new Response(JSON.stringify({ error: `no stub for ${key}` }), { status: 404 });
      const { status, body } = handler(init);
      return new Response(JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json" }
      });
    })
  );
}

describe("staffCreds", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("uses a stored bearer", () => {
    const creds = staffCreds(fakeContext({ bearer: "tok" }));
    expect(creds).toEqual({ target: TARGET, bearer: "tok" });
  });

  it("falls back to the API key, as X-API-Key", () => {
    const creds = staffCreds(fakeContext({ apiKey: "sekrit" }));
    expect(creds).toEqual({ target: TARGET, headers: { "X-API-Key": "sekrit" } });
  });

  it("refuses an oxypublish_ bearer with a usage error (exit 2)", () => {
    const err = (() => {
      try {
        staffCreds(fakeContext({ bearer: "oxypublish_abc" }));
        return undefined;
      } catch (e) {
        return e as CliError;
      }
    })();
    expect(err).toBeInstanceOf(CliError);
    expect(err?.code).toBe(ExitCode.USAGE);
  });

  it("throws the canonical authError when nothing resolves", () => {
    const err = (() => {
      try {
        staffCreds(fakeContext());
        return undefined;
      } catch (e) {
        return e as CliError;
      }
    })();
    expect(err?.code).toBe(ExitCode.AUTH);
  });
});

describe("resolveApp", () => {
  let calls: { method: string; url: string }[];
  const creds: Creds = { target: TARGET, bearer: "tok" };

  afterEach(() => vi.unstubAllGlobals());

  it("pages GET /api/admin/apps to match an org/app slug", async () => {
    calls = [];
    stubFetch(
      {
        "GET /api/admin/apps?limit=100&offset=0": () => ({
          status: 200,
          body: { items: [], next_offset: 100 }
        }),
        "GET /api/admin/apps?limit=100&offset=100": () => ({
          status: 200,
          body: {
            items: [{ id: APP_ID, slug: "store", org_slug: "acme" }],
            next_offset: null
          }
        })
      },
      calls
    );

    const resolved = await resolveApp(creds, "acme/store");
    expect(resolved).toEqual({
      appId: APP_ID,
      label: "acme/store",
      orgSlug: "acme",
      appSlug: "store"
    });
  });

  it("resolves a UUID through GET /api/admin/apps/{id}", async () => {
    calls = [];
    stubFetch(
      {
        [`GET /api/admin/apps/${APP_ID}`]: () => ({
          status: 200,
          body: { id: APP_ID, slug: "store", org_slug: "acme" }
        })
      },
      calls
    );

    const resolved = await resolveApp(creds, APP_ID);
    expect(resolved).toEqual({
      appId: APP_ID,
      label: "acme/store",
      orgSlug: "acme",
      appSlug: "store"
    });
    expect(calls).toHaveLength(1);
  });

  it("throws NOT_FOUND (exit 5) when no page matches", async () => {
    calls = [];
    stubFetch(
      {
        "GET /api/admin/apps?limit=100&offset=0": () => ({
          status: 200,
          body: { items: [], next_offset: null }
        })
      },
      calls
    );

    const err = await resolveApp(creds, "acme/no-such-app").catch((e: unknown) => e as CliError);
    expect(err).toBeInstanceOf(CliError);
    expect((err as CliError).code).toBe(ExitCode.NOT_FOUND);
  });
});

describe("ensureOk", () => {
  it("does nothing for a 2xx response", () => {
    expect(() =>
      ensureOk({
        status: 200,
        statusText: "OK",
        headers: {},
        body: "{}",
        url: TARGET,
        fromCache: false
      })
    ).not.toThrow();
  });

  it("throws the status-mapped error for a non-2xx response", () => {
    const err = (() => {
      try {
        ensureOk({
          status: 404,
          statusText: "Not Found",
          headers: {},
          body: '{"error":"environment_not_found","message":"no such sandbox"}',
          url: TARGET,
          fromCache: false
        });
        return undefined;
      } catch (e) {
        return e as CliError;
      }
    })();
    expect(err).toBeInstanceOf(CliError);
    expect(err?.code).toBe(ExitCode.NOT_FOUND);
    expect(err?.detail).toContain("environment_not_found");
  });
});
