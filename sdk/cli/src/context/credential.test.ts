/**
 * Credential resolution, for every command: `OXY_TOKEN`, then the login
 * cache, then GitHub OIDC. The order is the contract — an explicit token must
 * never be shadowed by one the CLI could mint, and a laptop that happens to
 * export GitHub's variables must not start minting over its own login.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { saveCredential } from "../auth/credentials.js";
import { runExitRevokes } from "../auth/exit-revoke.js";
import { OidcExchangeError, resetOidcExchanges } from "../auth/oidc.js";
import {
  type Call,
  GITHUB_ID_TOKEN_ROUTES,
  inGithubActions,
  SERVICE_ACCOUNT_ID,
  stubFetch
} from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import { createContext } from "./resolve.js";

const TARGET = "https://oxy.test";

const MINTED = {
  token: "oxy_ci_minted",
  token_id: "tok-1",
  expires_at: "2099-01-01T00:00:00Z",
  service_account: "acme/deployer",
  grants: []
};

const EXCHANGE_OK = {
  ...GITHUB_ID_TOKEN_ROUTES,
  "POST /api/auth/oidc/exchange": () => ({ status: 200, body: MINTED }),
  "DELETE /api/auth/token": () => ({ status: 204 })
};

let scratch: string;

const context = (flags: { serviceAccount?: string; tokenEnv?: string } = {}) =>
  createContext({ env: "production", target: TARGET, ...flags }, scratch);

const exchanged = (calls: Call[]) => calls.filter((c) => c.path === "/api/auth/oidc/exchange");

beforeEach(() => {
  scratch = mkdtempSync(join(tmpdir(), "oxyc-ctx-"));
  vi.stubEnv("OXY_CREDENTIALS_PATH", join(scratch, "credentials.json"));
  vi.stubEnv("OXY_TOKEN", "");
  vi.stubEnv("OXY_SERVICE_ACCOUNT", "");
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_URL", "");
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "");
  resetOidcExchanges();
});

afterEach(async () => {
  await runExitRevokes();
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  rmSync(scratch, { recursive: true, force: true });
});

describe("the resolution order", () => {
  it("1. OXY_TOKEN wins over the cache and over OIDC", async () => {
    const calls = stubFetch(TARGET, EXCHANGE_OK);
    vi.stubEnv("OXY_TOKEN", "oxy_pat_from_env");
    saveCredential(TARGET, { token: "cached", email: "", is_app_admin: false });
    inGithubActions(TARGET);

    const ctx = context();
    expect(await ctx.credential()).toEqual({ token: "oxy_pat_from_env", source: "env" });
    expect(await ctx.bearer()).toBe("oxy_pat_from_env");
    expect(calls).toHaveLength(0);
  });

  it("2. the login cache wins over OIDC, and carries what the login recorded", async () => {
    const calls = stubFetch(TARGET, EXCHANGE_OK);
    saveCredential(TARGET, {
      token: "oxy_pat_cached",
      email: "a@b.c",
      is_app_admin: false,
      token_id: "tok-9",
      expires_at: "2026-12-30T00:00:00Z"
    });
    inGithubActions(TARGET);

    expect(await context().credential()).toEqual({
      token: "oxy_pat_cached",
      source: "file",
      tokenId: "tok-9",
      expiresAt: "2026-12-30T00:00:00Z"
    });
    expect(calls).toHaveLength(0);
  });

  it("3. with neither, a GitHub Actions job that names its service account exchanges its OIDC token", async () => {
    const calls = stubFetch(TARGET, EXCHANGE_OK);
    inGithubActions(TARGET);
    vi.stubEnv("OXY_SERVICE_ACCOUNT", SERVICE_ACCOUNT_ID);

    expect(await context().credential()).toEqual({
      token: "oxy_ci_minted",
      source: "oidc",
      tokenId: "tok-1",
      expiresAt: "2099-01-01T00:00:00Z",
      // The readable name the deployment answered with — not what was sent.
      serviceAccount: "acme/deployer"
    });
    expect(exchanged(calls)).toHaveLength(1);
  });

  /**
   * NO ACCOUNT, NO EXCHANGE. Anyone can register a trust policy naming this
   * repository, so a run that does not say which account it trusts is never
   * matched against whatever happens to be registered — oxyc does not even
   * ask. What the job sees is the missing variable, by name.
   */
  it("in GitHub Actions with no service account named, asks nobody and says OXY_SERVICE_ACCOUNT is needed", async () => {
    const calls = stubFetch(TARGET, EXCHANGE_OK);
    inGithubActions(TARGET);

    const ctx = context();
    const error = (await ctx.bearer().catch((e: unknown) => e)) as OidcExchangeError;
    expect(error).toBeInstanceOf(OidcExchangeError);
    expect(error.oidcCode).toBe("no_service_account");
    expect(error.code).toBe(ExitCode.AUTH);
    expect(error.message).toBe(`not authenticated for ${TARGET}`);
    expect(error.hint).toContain("OXY_SERVICE_ACCOUNT");
    expect(error.hint).toContain("OXY_TOKEN");
    // `maybeBearer` has nothing to offer either, and says nothing about it.
    const stderr = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
    expect(await ctx.maybeBearer()).toBeUndefined();
    const written = stderr.mock.calls.map((c) => String(c[0])).join("");
    stderr.mockRestore();
    expect(written).toBe("");
    // Not one request: no id token minted, nothing posted.
    expect(calls).toHaveLength(0);
  });

  it("with none of the three, it is the same 'not authenticated' it always was", async () => {
    const calls = stubFetch(TARGET, EXCHANGE_OK);
    const error = await context()
      .bearer()
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CliError);
    expect((error as CliError).code).toBe(ExitCode.AUTH);
    expect((error as CliError).message).toBe(`not authenticated for ${TARGET}`);
    expect((error as CliError).hint).toContain("oxyc login --env production");
    expect(await context().maybeBearer()).toBeUndefined();
    expect(calls).toHaveLength(0);
  });

  it("treats a blank OXY_TOKEN as unset — an unset secret arrives as an empty string", async () => {
    vi.stubEnv("OXY_TOKEN", "   ");
    saveCredential(TARGET, { token: "cached", email: "", is_app_admin: false });
    expect(await context().bearer()).toBe("cached");
  });

  it("reads the variable --token-env names instead of OXY_TOKEN", async () => {
    vi.stubEnv("OXY_TOKEN", "ignored");
    vi.stubEnv("MY_TOKEN", "from-my-token");
    expect(await context({ tokenEnv: "MY_TOKEN" }).bearer()).toBe("from-my-token");
  });

  it("accepts an existing cache holding a session JWT, unchanged", async () => {
    saveCredential(TARGET, { token: "eyJ.session.jwt", email: "a@b.c", is_app_admin: true });
    expect(await context().credential()).toEqual({ token: "eyJ.session.jwt", source: "file" });
  });
});

describe("the OIDC step", () => {
  it("exchanges once however many commands ask, and once across contexts", async () => {
    const calls = stubFetch(TARGET, EXCHANGE_OK);
    inGithubActions(TARGET);
    vi.stubEnv("OXY_SERVICE_ACCOUNT", SERVICE_ACCOUNT_ID);
    const ctx = context();
    await ctx.bearer();
    await ctx.bearer();
    await ctx.maybeBearer();
    await context().bearer();
    expect(exchanged(calls)).toHaveLength(1);
  });

  it("passes --service-account through unchanged, and OXY_SERVICE_ACCOUNT when the flag is absent", async () => {
    // Two ids. oxyc does not read them: what it was given is what it sends.
    const FROM_FLAG = "11111111-1111-4111-8111-111111111111";
    const FROM_ENV = "22222222-2222-4222-8222-222222222222";
    const calls = stubFetch(TARGET, EXCHANGE_OK);
    inGithubActions(TARGET);

    await context({ serviceAccount: FROM_FLAG }).bearer();
    vi.stubEnv("OXY_SERVICE_ACCOUNT", FROM_ENV);
    await context().bearer();
    // The flag beats the variable.
    await context({ serviceAccount: FROM_FLAG }).bearer();

    expect(exchanged(calls).map((c) => JSON.parse(c.body ?? "{}").service_account)).toEqual([
      FROM_FLAG,
      FROM_ENV
    ]);
  });

  it("storedBearer never mints — it is for callers not ready to spend a single-use token", () => {
    const calls = stubFetch(TARGET, EXCHANGE_OK);
    inGithubActions(TARGET);
    expect(context().storedBearer()).toBeUndefined();
    expect(calls).toHaveLength(0);
  });

  it("a refused exchange is bearer()'s error, with the fix — not a bare 'not authenticated'", async () => {
    stubFetch(TARGET, {
      ...GITHUB_ID_TOKEN_ROUTES,
      "POST /api/auth/oidc/exchange": () => ({
        status: 403,
        body: { code: "missing_environment" }
      })
    });
    inGithubActions(TARGET);
    const error = await context({ serviceAccount: SERVICE_ACCOUNT_ID })
      .bearer()
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(OidcExchangeError);
    expect((error as OidcExchangeError).oidcCode).toBe("missing_environment");
    expect((error as OidcExchangeError).hint).toContain("environment");
  });

  it("a refused exchange is maybeBearer()'s warning: the caller asked 'if there is one'", async () => {
    stubFetch(TARGET, {
      ...GITHUB_ID_TOKEN_ROUTES,
      "POST /api/auth/oidc/exchange": () => ({ status: 403, body: { code: "no_matching_policy" } })
    });
    inGithubActions(TARGET);
    const stderr = vi.spyOn(process.stderr, "write").mockImplementation(() => true);

    const ctx = context({ serviceAccount: SERVICE_ACCOUNT_ID });
    expect(await ctx.maybeBearer()).toBeUndefined();
    expect(await ctx.maybeBearer()).toBeUndefined();

    const written = stderr.mock.calls.map((c) => String(c[0])).join("");
    stderr.mockRestore();
    // Said once, not once per call.
    expect(written.match(/no trust policy of the named service account/g)).toHaveLength(1);
  });

  /**
   * THE OLD-SERVER FALLBACK. A deployment with no exchange answers 404, and
   * what the caller sees must be what it saw before OIDC was tried at all: no
   * bearer, quietly, from `maybeBearer`; the AUTH exit from `bearer` — now
   * saying why the job's OIDC identity could not help.
   */
  it("against a deployment with no exchange, behaves as it did before — and says why", async () => {
    stubFetch(TARGET, GITHUB_ID_TOKEN_ROUTES);
    inGithubActions(TARGET);
    const stderr = vi.spyOn(process.stderr, "write").mockImplementation(() => true);

    const ctx = context({ serviceAccount: SERVICE_ACCOUNT_ID });
    expect(await ctx.maybeBearer()).toBeUndefined();
    const error = (await ctx.bearer().catch((e: unknown) => e)) as OidcExchangeError;

    const written = stderr.mock.calls.map((c) => String(c[0])).join("");
    stderr.mockRestore();
    expect(written).toBe("");
    expect(error.message).toBe(`not authenticated for ${TARGET}`);
    expect(error.code).toBe(ExitCode.AUTH);
    expect(error.oidcCode).toBe("unsupported");
    expect(error.detail).toContain("/api/auth/oidc/exchange");
  });
});
