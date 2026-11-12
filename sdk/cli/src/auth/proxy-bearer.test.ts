/**
 * `OXY_TOKEN=from-proxy`: the credential is added on the way out by the
 * environment's egress proxy (a Claude Code cloud network secret), so oxyc
 * must send requests with no `Authorization` and read the kind as unknown.
 *
 * The source guard is the part that keeps it true: a new site that builds
 * `Bearer ${token}` by hand would send `Bearer from-proxy`, which the proxy
 * may or may not replace. Every header goes through `bearerHeader`.
 */

import { mkdtempSync, readdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { authHeaders } from "../api/request.js";
import { runWhoami } from "../commands/auth.js";
import { runTokensRevokeCurrent } from "../commands/tokens-sandbox.js";
import { createContext } from "../context/resolve.js";
import { stubFetch } from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import { forgetVariableCredential, hintToShow } from "./agent-token.js";
import {
  bearerHeader,
  credentialShape,
  isMachineIdentity,
  isProxyCredential,
  isRevocable,
  isSandboxAgentToken,
  PROXY_CREDENTIAL,
  usableAsApiKey
} from "./token-kind.js";

const TARGET = "https://oxy.test";
const SRC = fileURLToPath(new URL("../", import.meta.url));

describe("the from-proxy sentinel", () => {
  it("is its own kind, and none of the kinds a prefix decides anything for", () => {
    expect(PROXY_CREDENTIAL).toBe("from-proxy");
    expect(isProxyCredential("from-proxy")).toBe(true);
    expect(isProxyCredential(" from-proxy")).toBe(false);
    expect(credentialShape(PROXY_CREDENTIAL)).toBe("proxy");
    expect(isSandboxAgentToken(PROXY_CREDENTIAL)).toBe(false);
    expect(isMachineIdentity(PROXY_CREDENTIAL)).toBe(false);
    // `DELETE /api/auth/token` would end the network secret's token.
    expect(isRevocable(PROXY_CREDENTIAL)).toBe(false);
    expect(usableAsApiKey(PROXY_CREDENTIAL)).toBe(false);
  });

  it("builds no header, where a real token builds one", () => {
    expect(bearerHeader(PROXY_CREDENTIAL)).toEqual({});
    expect(bearerHeader(undefined)).toEqual({});
    expect(bearerHeader("")).toEqual({});
    expect(bearerHeader("oxy_pat_x")).toEqual({ authorization: "Bearer oxy_pat_x" });
  });

  it("sends neither Authorization nor X-API-Key, on either surface", () => {
    for (const path of ["/api/user", "/external/api/v1/query"]) {
      expect(
        authHeaders({ target: TARGET, path, method: "GET", bearer: PROXY_CREDENTIAL })
      ).toEqual({});
    }
    // An API key the caller named still goes, beside no bearer.
    expect(
      authHeaders({
        target: TARGET,
        path: "/external/api/v1/query",
        method: "GET",
        bearer: PROXY_CREDENTIAL,
        apiKey: "oxy_pat_key"
      })
    ).toEqual({ "X-API-Key": "oxy_pat_key" });
  });
});

/** Every non-test source file under `src/`. */
function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return name === "generated" ? [] : sources(path);
    return name.endsWith(".ts") && !name.endsWith(".test.ts") ? [path] : [];
  });
}

describe("every bearer header", () => {
  it("is built by bearerHeader, so the sentinel is never sent", () => {
    // Two sites send something that is never the stored credential: GitHub's
    // own request token, and the GitHub OIDC token a publish exchanges.
    const allowed = new Set(["auth/oidc.ts", "publish/server.ts:oidc"]);
    const offenders: string[] = [];
    for (const file of sources(SRC)) {
      const rel = relative(SRC, file);
      if (rel === "auth/token-kind.ts") continue;
      for (const match of readFileSync(file, "utf8").matchAll(/[Bb]earer \$\{([^}]*)\}/g)) {
        if (allowed.has(rel) || allowed.has(`${rel}:${match[1]}`)) continue;
        offenders.push(`${rel}: ${match[0]}`);
      }
    }
    expect(offenders).toEqual([]);
  });
});

describe("oxyc whoami with OXY_TOKEN=from-proxy", () => {
  let scratch: string;
  let stdout: string;

  beforeEach(() => {
    scratch = mkdtempSync(join(tmpdir(), "oxyc-proxy-"));
    vi.stubEnv("OXY_CREDENTIALS_PATH", join(scratch, "credentials.json"));
    vi.stubEnv("OXY_TOKEN", PROXY_CREDENTIAL);
    vi.stubEnv("OXY_SERVICE_ACCOUNT", "");
    vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_URL", "");
    vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "");
    stdout = "";
    vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
      stdout += String(chunk);
      return true;
    });
    vi.spyOn(process.stderr, "write").mockImplementation(() => true);
  });

  afterEach(() => {
    forgetVariableCredential();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
    rmSync(scratch, { recursive: true, force: true });
  });

  const context = () => createContext({ env: "production", target: TARGET }, scratch);

  /** A deployment behind a proxy that adds the header to a request with none. */
  const behindProxy = (attach: boolean) => {
    const seen = (auth: string | undefined) =>
      auth ?? (attach ? "Bearer oxy_pat_secret" : undefined);
    return stubFetch(TARGET, {
      "GET /api/user": (call) =>
        seen(call.headers.authorization) === "Bearer oxy_pat_secret"
          ? { status: 200, body: { id: "u-1", email: "ada@acme.test", is_app_admin: false } }
          : { status: 401, body: { error: "unauthorized" } },
      "GET /api/auth/token": () => ({ status: 404, body: { code: "no_token" } })
    });
  };

  it("sends no Authorization, and says the credential is the network secret", async () => {
    const calls = behindProxy(true);
    await runWhoami(context(), false);
    expect(calls.length).toBeGreaterThan(0);
    for (const call of calls) expect(call.headers.authorization).toBeUndefined();
    expect(stdout).toContain("ada@acme.test");
    expect(stdout).toContain("network secret");
  });

  it("never reads the login cache under the sentinel", async () => {
    const { saveCredential } = await import("./credentials.js");
    saveCredential(TARGET, {
      token: "oxy_pat_cached",
      email: "cached@acme.test",
      is_app_admin: false
    });
    const calls = behindProxy(true);
    await runWhoami(context(), false);
    for (const call of calls) expect(call.headers.authorization).toBeUndefined();
  });

  it("names the proxy's causes when nothing was attached — not `oxyc login`", async () => {
    behindProxy(false);
    const error = await runWhoami(context(), false).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CliError);
    expect((error as CliError).code).toBe(ExitCode.AUTH);
    expect((error as CliError).message).toContain("the proxy attaches");
    expect((error as CliError).hint).toContain("Allowed websites");
    expect((error as CliError).hint).not.toContain("oxyc login --env");
  });

  it("swaps the generic 401 hint for the proxy's", async () => {
    // What `main.ts` shows for any command's 401: resolve the credential first,
    // as every command does, then ask for the hint.
    await context().credential();
    const { AUTH_HINT } = await import("../api/request.js");
    const hint = hintToShow(new CliError("401", { code: ExitCode.AUTH, hint: AUTH_HINT }));
    expect(hint).toContain("OXY_TOKEN=from-proxy");
    expect(hint).toContain("Allowed websites");
  });

  it("does not revoke the network secret's token from --current", async () => {
    const calls = stubFetch(TARGET, {});
    const error = await runTokensRevokeCurrent(context()).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CliError);
    expect((error as CliError).message).toContain("from-proxy holds none");
    expect(calls.filter((c) => c.method === "DELETE")).toEqual([]);
  });
});
