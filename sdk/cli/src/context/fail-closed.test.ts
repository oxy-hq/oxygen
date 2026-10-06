/**
 * Fail-closed credential resolution: a caller that NAMED its token variable
 * (`--token-env`), and `oxyc mcp` without `--login`, get that variable or
 * nothing. The login cache is not a fallback, and neither is a GitHub OIDC
 * exchange — a mistyped variable name must not run an agent as whoever last
 * ran `oxyc login` on the machine.
 *
 * `loadCredential` is wrapped so "never read the cache" is an assertion about
 * the read itself, not an inference from the result.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { staffCreds } from "../apps/resolve.js";
import * as credentials from "../auth/credentials.js";
import { resetOidcExchanges } from "../auth/oidc.js";
import { resolveCredentials } from "../commands/checks-credentials.js";
import {
  GITHUB_ID_TOKEN_ROUTES,
  inGithubActions,
  SERVICE_ACCOUNT_ID,
  stubFetch
} from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import { createContext, type GlobalFlags } from "./resolve.js";

vi.mock("../auth/credentials.js", async (importOriginal) => {
  const real = await importOriginal<typeof import("../auth/credentials.js")>();
  return { ...real, loadCredential: vi.fn(real.loadCredential) };
});

const TARGET = "https://oxy.test";
const cacheReads = vi.mocked(credentials.loadCredential);

let scratch: string;

const context = (flags: Partial<GlobalFlags> = {}) =>
  createContext({ env: "production", target: TARGET, ...flags }, scratch);

async function refusal(run: Promise<unknown>): Promise<CliError> {
  const cause = await run.then(
    () => undefined,
    (thrown: unknown) => thrown
  );
  expect(cause).toBeInstanceOf(CliError);
  return cause as CliError;
}

beforeEach(() => {
  scratch = mkdtempSync(join(tmpdir(), "oxyc-closed-"));
  vi.stubEnv("OXY_CREDENTIALS_PATH", join(scratch, "credentials.json"));
  vi.stubEnv("OXY_TOKEN", "");
  vi.stubEnv("OXY_API_KEY", "");
  vi.stubEnv("AGENT_TOKEN", "");
  vi.stubEnv("OXY_SERVICE_ACCOUNT", "");
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_URL", "");
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "");
  resetOidcExchanges();
  // The human's login, sitting in the cache the whole time.
  credentials.saveCredential(TARGET, {
    token: "oxy_pat_the_humans_login",
    email: "luong@oxy.tech",
    is_app_admin: true
  });
  cacheReads.mockClear();
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  rmSync(scratch, { recursive: true, force: true });
});

describe("--token-env naming an unset variable", () => {
  it("exits AUTH (4) without reading the login cache", async () => {
    const calls = stubFetch(TARGET, {});
    const ctx = context({ tokenEnv: "AGENT_TOKEN" });

    const cause = await refusal(ctx.bearer());
    expect(cause.code).toBe(ExitCode.AUTH);
    expect(cause.message).toContain("AGENT_TOKEN is not set");
    // It must not send the caller to the fallback it just refused to take.
    expect(cause.hint).not.toContain("oxyc login --env");

    expect(ctx.storedBearer()).toBeUndefined();
    expect(await ctx.maybeBearer()).toBeUndefined();
    expect(cacheReads).not.toHaveBeenCalled();
    expect(calls).toHaveLength(0);
  });

  it("treats an empty or blank variable as unset", async () => {
    vi.stubEnv("AGENT_TOKEN", "   ");
    const ctx = context({ tokenEnv: "AGENT_TOKEN" });
    expect((await refusal(ctx.bearer())).code).toBe(ExitCode.AUTH);
    expect(cacheReads).not.toHaveBeenCalled();
  });

  it("is closed even when the name is the default one", async () => {
    // `--token-env OXY_TOKEN`, typed out, is still a caller naming its source.
    const ctx = context({ tokenEnv: "OXY_TOKEN" });
    expect((await refusal(ctx.bearer())).code).toBe(ExitCode.AUTH);
    expect(cacheReads).not.toHaveBeenCalled();
  });

  it("does not mint from GitHub OIDC either", async () => {
    const calls = stubFetch(TARGET, {
      ...GITHUB_ID_TOKEN_ROUTES,
      "POST /api/auth/oidc/exchange": () => ({ status: 200, body: { token: "oxy_ci_minted" } })
    });
    inGithubActions(TARGET);
    const ctx = context({ tokenEnv: "AGENT_TOKEN", serviceAccount: SERVICE_ACCOUNT_ID });
    expect((await refusal(ctx.bearer())).code).toBe(ExitCode.AUTH);
    expect(await ctx.maybeBearer()).toBeUndefined();
    expect(calls).toHaveLength(0);
  });

  it("reaches every command that asks for a staff credential", () => {
    const thrown = (() => {
      try {
        return staffCreds(context({ tokenEnv: "AGENT_TOKEN" }));
      } catch (cause) {
        return cause;
      }
    })();
    expect(thrown).toBeInstanceOf(CliError);
    expect((thrown as CliError).code).toBe(ExitCode.AUTH);
    expect((thrown as CliError).message).toContain("AGENT_TOKEN is not set");
    expect(cacheReads).not.toHaveBeenCalled();
  });

  it("uses the variable when it is set — the cache is still never opened", async () => {
    vi.stubEnv("AGENT_TOKEN", "oxy_sbx_agent");
    const ctx = context({ tokenEnv: "AGENT_TOKEN" });
    expect(await ctx.credential()).toEqual({ token: "oxy_sbx_agent", source: "env" });
    expect(cacheReads).not.toHaveBeenCalled();
  });

  it("still lets an API key the caller also named stand alone", async () => {
    // What the release checks do on purpose: a never-set `--token-env` so that
    // `OXY_API_KEY` is the credential (`custom-app-checks.yaml`).
    vi.stubEnv("OXY_API_KEY", "oxy_0123abcd");
    const ctx = context({ tokenEnv: "OXY_CHECKS_NO_BEARER" });
    expect(staffCreds(ctx)).toEqual({ target: TARGET, headers: { "X-API-Key": "oxy_0123abcd" } });
    expect(await resolveCredentials(ctx, TARGET, "acme/store")).toEqual({
      apiKey: "oxy_0123abcd",
      surface: "/api/admin/apps"
    });
    expect(cacheReads).not.toHaveBeenCalled();
  });
});

describe("no --token-env", () => {
  it("falls back to the login cache exactly as before", async () => {
    const ctx = context();
    expect(await ctx.bearer()).toBe("oxy_pat_the_humans_login");
    expect((await ctx.credential()).source).toBe("file");
    expect(cacheReads).toHaveBeenCalled();
  });

  it("names `oxyc login` when nothing resolves at all", async () => {
    credentials.clearCredential(TARGET);
    const cause = await refusal(context().bearer());
    expect(cause.code).toBe(ExitCode.AUTH);
    expect(cause.hint).toContain("oxyc login --env production");
  });
});

describe("oxyc mcp without --login (requireTokenEnv)", () => {
  it("is closed on the default variable, and says how to opt in", async () => {
    const ctx = context({ requireTokenEnv: true });
    const cause = await refusal(ctx.bearer());
    expect(cause.code).toBe(ExitCode.AUTH);
    expect(cause.message).toContain("OXY_TOKEN is not set");
    expect(cause.hint).toContain("oxyc mcp --login");
    expect(cacheReads).not.toHaveBeenCalled();
  });

  it("uses OXY_TOKEN when it is set", async () => {
    vi.stubEnv("OXY_TOKEN", "oxy_sbx_agent");
    expect(await context({ requireTokenEnv: true }).bearer()).toBe("oxy_sbx_agent");
    expect(cacheReads).not.toHaveBeenCalled();
  });
});
