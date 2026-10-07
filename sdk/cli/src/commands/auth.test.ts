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

  describe("under an agent token", () => {
    /** As `oxyc tokens create --agent` leaves one: a personal token, told apart by its source. */
    const AGENT = {
      ...PAT,
      id: "tok-agent-1",
      name: "agent on laptop",
      source: "oxyc_agent",
      all_access: true,
      grants: [],
      expires_at: new Date(Date.now() + 8 * 3_600_000).toISOString(),
      owner: { type: "user", id: "u-1", label: "ada@acme.test" }
    };
    const described = (over: Record<string, unknown> = {}) =>
      routes({ "GET /api/auth/token": () => ({ status: 200, body: { ...AGENT, ...over } }) });

    it("says it is an agent token, who approved it, that it carries no standing, and when it ends", async () => {
      vi.stubEnv("OXY_TOKEN", "oxy_pat_secret");
      stubFetch(TARGET, described());
      await runWhoami(context(), false);

      expect(stdout).toMatch(/token\s+agent on laptop {2}\(agent token, oxy_pat_ab12…9f3c\)/);
      expect(stdout).toMatch(/approved by\s+ada@acme\.test/);
      expect(stdout).toMatch(
        /reach\s+everything ada@acme\.test can reach through their organizations/
      );
      expect(stdout).toContain("no staff or partner access");
      expect(stdout).toMatch(/expires\s+\S+ \(in 8 h\) — it cannot be extended/);
      expect(stdout).toContain("oxyc tokens revoke --current");
      // Never called by its kind: `personal` would read as the person's own login.
      expect(stdout).not.toContain("(personal,");
      expect(stdout).not.toContain("oxy_pat_secret");
    });

    it("says which standing it carries: staff, partner, or both", async () => {
      vi.stubEnv("OXY_TOKEN", "oxy_pat_secret");
      stubFetch(TARGET, described({ platform: true }));
      await runWhoami(context(), false);
      expect(stdout).toContain("+ staff access: every organization on this deployment");
      expect(stdout).not.toContain("partner access:");
      expect(stdout).not.toContain("no staff or partner access");

      stdout = "";
      stubFetch(TARGET, described({ platform: true, partner: true }));
      await runWhoami(context(), false);
      expect(stdout).toContain("+ staff access: every organization on this deployment");
      expect(stdout).toContain("+ partner access: the approver's client organizations");
    });

    it("keeps calling an oxyc login what it is: the source is what makes an agent token", async () => {
      vi.stubEnv("OXY_TOKEN", "oxy_pat_secret");
      stubFetch(TARGET, described({ source: "oxyc_login", name: "oxyc on laptop" }));
      await runWhoami(context(), false);
      expect(stdout).toMatch(/token\s+oxyc on laptop {2}\(personal, /);
      expect(stdout).not.toContain("agent token");
      expect(stdout).not.toContain("approved by");
    });

    it("prints its name and its approver without control characters", async () => {
      const ESC = String.fromCharCode(27);
      vi.stubEnv("OXY_TOKEN", "oxy_pat_secret");
      stubFetch(
        TARGET,
        described({
          name: `agent${ESC}[2J`,
          owner: { type: "user", id: "u-1", label: `ada${ESC}]0;x@acme.test` }
        })
      );
      await runWhoami(context(), false);
      expect(stdout).not.toContain(ESC);
    });
  });

  describe("when the deployment refuses the credential (exit 4)", () => {
    const refusedBy = async (status: number) => {
      stubFetch(TARGET, { "GET /api/user": () => ({ status, body: { error: "nope" } }) });
      const cause = await runWhoami(context(), false).then(
        () => undefined,
        (thrown: unknown) => thrown
      );
      expect(cause).toBeInstanceOf(CliError);
      expect((cause as CliError).code).toBe(ExitCode.AUTH);
      return cause as CliError;
    };

    it.each([401, 403])(
      "tells an agent on a token in the variable to stop and report, never to log in (%i)",
      async (status) => {
        // An agent token that expired or was revoked: dead, so it cannot say what it was.
        vi.stubEnv("OXY_TOKEN", "oxy_pat_0123456789abcdefghijABCDEFGHIJ012345");
        const cause = await refusedBy(status);
        expect(cause.message).toBe(`the token in OXY_TOKEN is no longer accepted (${TARGET})`);
        expect(cause.hint).toContain("an agent: stop and report this to your operator");
        expect(cause.hint).toContain("do not look for another credential");
        expect(cause.detail).toContain("a login does not replace it");
        expect(cause.detail).toContain("oxyc tokens create --agent");
        // The one command it names for a login is the person's, and only as theirs.
        const lines = (cause.hint ?? "").split("\n");
        expect(lines.filter((line) => line.startsWith("an agent:"))).toHaveLength(1);
        expect(lines.find((line) => line.startsWith("an agent:"))).not.toMatch(/run `oxyc login`/);
        expect(cause.hint).not.toMatch(/^oxyc login/m);
      }
    );

    it("names the variable that was named, when --token-env picked another", async () => {
      vi.stubEnv("AGENT_TOKEN", "oxy_pat_0123456789abcdefghijABCDEFGHIJ012345");
      stubFetch(TARGET, { "GET /api/user": () => ({ status: 401, body: {} }) });
      const ctx = createContext(
        { env: "production", target: TARGET, tokenEnv: "AGENT_TOKEN" },
        scratch
      );
      const cause = (await runWhoami(ctx, false).then(
        () => undefined,
        (thrown: unknown) => thrown
      )) as CliError;
      expect(cause.code).toBe(ExitCode.AUTH);
      expect(cause.message).toContain("the token in AGENT_TOKEN is no longer accepted");
      expect(cause.hint).toContain("put a token that works in AGENT_TOKEN");
    });

    it("says the same of a token that no longer resolves to a user", async () => {
      vi.stubEnv("OXY_TOKEN", "oxy_pat_0123456789abcdefghijABCDEFGHIJ012345");
      vi.stubGlobal(
        "fetch",
        vi.fn(
          async () =>
            new Response("null", { status: 200, headers: { "content-type": "application/json" } })
        )
      );
      const cause = (await runWhoami(context(), false).then(
        () => undefined,
        (thrown: unknown) => thrown
      )) as CliError;
      expect(cause.code).toBe(ExitCode.AUTH);
      expect(cause.message).toContain("the token in OXY_TOKEN no longer resolves to a user");
      expect(cause.hint).toContain("an agent: stop and report this to your operator");
      expect(cause.hint).not.toMatch(/^oxyc login/m);
    });

    it("still sends a person with a cached login back to log in", async () => {
      cache("oxy_pat_cached_login");
      const cause = await refusedBy(401);
      expect(cause.message).toBe(`the cached token for ${TARGET} is no longer accepted`);
      expect(cause.hint).toBe("oxyc login --env production");
    });
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
