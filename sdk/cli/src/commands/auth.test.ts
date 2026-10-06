/**
 * `oxyc logout`, `whoami` and `token` against the token routes.
 *
 * Each of the three has an OLD-DEPLOYMENT half that must stay exactly what it
 * was: a logout that clears the cache with nothing to revoke, a whoami with no
 * token lines, a `token` that prints what is stored. Those are pinned beside
 * the new behaviour, because the new behaviour is the part that could break
 * them.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { loadCredential, saveCredential } from "../auth/credentials.js";
import { pendingRevokes, runExitRevokes } from "../auth/exit-revoke.js";
import { resetOidcExchanges } from "../auth/oidc.js";
import { createContext } from "../context/resolve.js";
import {
  type Call,
  GITHUB_ID_TOKEN_ROUTES,
  inGithubActions,
  type Routes,
  SERVICE_ACCOUNT_ID,
  stubFetch
} from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import { runLogout, runToken, runWhoami } from "./auth.js";

const TARGET = "https://oxy.test";
const USER = { id: "u-1", email: "ada@acme.test", is_app_admin: false };

const PAT = {
  id: "tok-1",
  name: "oxyc on laptop",
  kind: "personal",
  display_prefix: "oxy_pat_ab12",
  last_four: "9f3c",
  all_access: false,
  platform: false,
  partner: false,
  grants: [
    {
      id: "g-1",
      kind: "workspace",
      org_id: "org-1",
      org_name: "Acme",
      workspace_id: "ws-1",
      workspace_name: "Sales",
      role_ceiling: "viewer",
      app_id: null,
      app_name: null,
      revoked_at: null
    }
  ],
  expires_at: null,
  status: "active",
  blocked_orgs: []
};

let scratch: string;
let stdout: string;
let stderr: string;

const context = () => createContext({ env: "production", target: TARGET }, scratch);
const cache = (token: string) =>
  saveCredential(TARGET, { token, email: USER.email, is_app_admin: false });
const revokes = (calls: Call[]) =>
  calls.filter((c) => c.method === "DELETE" && c.path === "/api/auth/token");

beforeEach(() => {
  scratch = mkdtempSync(join(tmpdir(), "oxyc-auth-"));
  vi.stubEnv("OXY_CREDENTIALS_PATH", join(scratch, "credentials.json"));
  vi.stubEnv("OXY_TOKEN", "");
  vi.stubEnv("OXY_SERVICE_ACCOUNT", "");
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_URL", "");
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "");
  resetOidcExchanges();
  stdout = "";
  stderr = "";
  vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
    stdout += String(chunk);
    return true;
  });
  vi.spyOn(process.stderr, "write").mockImplementation((chunk) => {
    stderr += String(chunk);
    return true;
  });
});

afterEach(async () => {
  await runExitRevokes();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  rmSync(scratch, { recursive: true, force: true });
});

describe("oxyc logout", () => {
  it("revokes the cached token on the server, then forgets it", async () => {
    cache("oxy_pat_cached");
    const calls = stubFetch(TARGET, { "DELETE /api/auth/token": () => ({ status: 204 }) });
    await runLogout(context());
    expect(revokes(calls)).toHaveLength(1);
    expect(revokes(calls)[0]?.headers.authorization).toBe("Bearer oxy_pat_cached");
    expect(loadCredential(TARGET)).toBeUndefined();
    expect(stderr).toContain("Logged out of https://oxy.test.");
    expect(stderr).toContain("revoked on the server");
  });

  it("still logs out when the server cannot be reached — and says the revoke is unconfirmed", async () => {
    cache("oxy_pat_cached");
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("fetch failed");
      })
    );
    await runLogout(context());
    expect(loadCredential(TARGET)).toBeUndefined();
    expect(stderr).toContain("did not confirm a revoke");
    expect(stderr).toContain("Account → Personal access tokens");
  });

  it("says a legacy API key cannot revoke itself, and where it is revoked (409 legacy_immutable)", async () => {
    // A legacy API key is `oxy_<hex>`; `oxy_pat_` is never one.
    cache("oxy_0123456789abcdef0123456789abcdef");
    stubFetch(TARGET, {
      "DELETE /api/auth/token": () => ({
        status: 409,
        body: {
          error: "a legacy API key can only be revoked by its owner",
          code: "legacy_immutable"
        }
      })
    });
    await runLogout(context());
    expect(loadCredential(TARGET)).toBeUndefined();
    expect(stderr).toContain("it is a legacy API key and cannot revoke itself");
    expect(stderr).toContain("Workspace → Legacy API keys");
    // Never sent to the token page, and never called a token.
    expect(stderr).not.toContain("Personal access tokens");
    expect(stderr).not.toMatch(/the token was removed/);
    expect(stderr).not.toContain("did not confirm a revoke");
  });

  it("logs a session token out of an older deployment exactly as before: no warning", async () => {
    cache("eyJhbGciOiJIUzI1NiJ9.session.jwt");
    // No route stubbed: DELETE /api/auth/token answers 404, as an old server does.
    stubFetch(TARGET, {});
    await runLogout(context());
    expect(loadCredential(TARGET)).toBeUndefined();
    expect(stderr).toContain("Logged out of https://oxy.test.");
    expect(stderr).not.toContain("warning");
  });

  it("never touches OXY_TOKEN — that secret is the caller's, not the cache's", async () => {
    vi.stubEnv("OXY_TOKEN", "oxy_pat_from_env");
    const calls = stubFetch(TARGET, { "DELETE /api/auth/token": () => ({ status: 204 }) });
    await runLogout(context());
    expect(calls).toHaveLength(0);
    expect(stderr).toContain("no cached credential");
  });
});

describe("oxyc whoami", () => {
  const routes = (extra: Routes = {}): Routes => ({
    "GET /api/user": () => ({ status: 200, body: USER }),
    ...extra
  });

  it("shows what the calling token is and can reach", async () => {
    vi.stubEnv("OXY_TOKEN", "oxy_pat_secret");
    stubFetch(TARGET, routes({ "GET /api/auth/token": () => ({ status: 200, body: PAT }) }));
    await runWhoami(context(), false);
    expect(stdout).toContain("ada@acme.test");
    expect(stdout).toMatch(/token\s+oxyc on laptop {2}\(personal, oxy_pat_ab12…9f3c\)/);
    expect(stdout).toMatch(/reach\s+Acme \/ Sales — up to viewer/);
    expect(stdout).toMatch(/expires\s+never/);
  });

  it("calls a legacy API key a Legacy API key, never a token", async () => {
    vi.stubEnv("OXY_TOKEN", "oxy_0123456789abcdef0123456789abcdef");
    const legacy = {
      ...PAT,
      id: "key-1",
      name: "old ci key",
      kind: "legacy_key",
      display_prefix: "oxy_0123",
      last_four: "cdef",
      // How every legacy API key is stored; it says nothing about its owner's standing.
      all_access: true,
      platform: true,
      partner: true,
      grants: [],
      expires_at: "2099-01-30T00:00:00Z"
    };
    stubFetch(TARGET, routes({ "GET /api/auth/token": () => ({ status: 200, body: legacy }) }));
    await runWhoami(context(), false);
    expect(stdout).toMatch(/credential\s+Legacy API key: old ci key {2}\(oxy_0123…cdef\)/);
    expect(stdout).toMatch(/reach\s+everything its owner can/);
    expect(stdout).toMatch(/expires\s+2099-01-30/);
    expect(stdout).not.toMatch(/^token\b/m);
    expect(stdout).not.toContain("legacy_key");
    expect(stdout).not.toMatch(/standing/);
  });

  it("says a browser session is one, on 404 `no_token`", async () => {
    vi.stubEnv("OXY_TOKEN", "session-jwt");
    stubFetch(
      TARGET,
      routes({ "GET /api/auth/token": () => ({ status: 404, body: { code: "no_token" } }) })
    );
    await runWhoami(context(), false);
    expect(stdout).toMatch(/credential\s+a browser session/);
  });

  it("adds nothing against a deployment with no introspection route", async () => {
    vi.stubEnv("OXY_TOKEN", "session-jwt");
    stubFetch(TARGET, routes());
    await runWhoami(context(), false);
    expect(stdout).toContain("ada@acme.test");
    expect(stdout).not.toMatch(/^(token|reach|credential)\b/m);
  });

  it("still refuses the null body an expired token produces (AUTH)", async () => {
    vi.stubEnv("OXY_TOKEN", "session-jwt");
    // A literal `null` body, which the route table cannot express (it sends
    // `{}` for a missing body).
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response("null", { status: 200, headers: { "content-type": "application/json" } })
      )
    );
    const cause = await runWhoami(context(), false).then(
      () => undefined,
      (thrown: unknown) => thrown
    );
    expect(cause).toBeInstanceOf(CliError);
    expect((cause as CliError).code).toBe(ExitCode.AUTH);
  });
});

describe("oxyc token", () => {
  it("prints the stored bearer and nothing else on stdout", async () => {
    cache("oxy_pat_cached");
    const calls = stubFetch(TARGET, {});
    await runToken(context());
    expect(stdout).toBe("oxy_pat_cached\n");
    expect(calls).toHaveLength(0);
  });

  it("in GitHub Actions, prints the exchanged token and does NOT revoke it on exit", async () => {
    inGithubActions(TARGET);
    vi.stubEnv("OXY_SERVICE_ACCOUNT", SERVICE_ACCOUNT_ID);
    const calls = stubFetch(TARGET, {
      ...GITHUB_ID_TOKEN_ROUTES,
      "POST /api/auth/oidc/exchange": () => ({
        status: 200,
        body: {
          token: "oxy_ci_minted",
          token_id: "tok-ci",
          expires_at: "2099-01-01T00:00:00Z",
          service_account: "acme/deployer",
          grants: []
        }
      }),
      "DELETE /api/auth/token": () => ({ status: 204 })
    });
    await runToken(context());
    expect(stdout).toBe("oxy_ci_minted\n");
    expect(stderr).toContain("acme/deployer");
    expect(stderr).toContain("not revoked on exit");
    // The output IS the token: one dead before the caller reads it is no output.
    expect(pendingRevokes()).toEqual([]);
    await runExitRevokes();
    expect(revokes(calls)).toHaveLength(0);
  });

  it("OXY_TOKEN wins inside Actions too — what setup-oxyc exported is what is printed", async () => {
    inGithubActions(TARGET);
    vi.stubEnv("OXY_TOKEN", "oxy_ci_from_action");
    const calls = stubFetch(TARGET, GITHUB_ID_TOKEN_ROUTES);
    await runToken(context());
    expect(stdout).toBe("oxy_ci_from_action\n");
    expect(calls).toHaveLength(0);
  });
});
