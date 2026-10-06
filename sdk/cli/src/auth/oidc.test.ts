/**
 * The GitHub OIDC exchange, against the wire contract.
 *
 * Every refusal the server can give has its own `code`, and each one is
 * something a person fixes differently — add an `environment:`, name a service
 * account, register a policy. A CLI that printed the status and the body would
 * leave all of that to be worked out from a 403 in a CI log, so what is pinned
 * here is that each code produces ITS message and ITS hint, and the exit code
 * the status maps to.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { GITHUB_ID_TOKEN_ROUTES, inGithubActions, stubFetch } from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import { pendingRevokes, runExitRevokes } from "./exit-revoke.js";
import {
  exchangeOidc,
  exchangeOidcOnce,
  fallsBackToPublisher,
  githubOidcAvailable,
  OidcExchangeError,
  oidcAudience,
  RATE_LIMIT_MAX_WAIT_S,
  resetOidcExchanges,
  retryAfterSeconds
} from "./oidc.js";

const TARGET = "https://oxy.test";
/**
 * The account every exchange here names, by its id: one that names none is
 * never sent, and `acme/deployer` is only what the deployment answers with.
 */
const ACCOUNT = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";

const MINTED = {
  token: "oxy_ci_minted",
  token_id: "tok-1",
  expires_at: "2099-01-01T00:00:00Z",
  service_account: "acme/deployer",
  grants: [
    {
      id: "g1",
      kind: "workspace",
      org_id: "o1",
      org_name: "Acme",
      workspace_id: null,
      workspace_name: null,
      role_ceiling: "member",
      app_id: null,
      app_name: null,
      revoked_at: null
    }
  ]
};

/** Run the exchange against a server that refuses it, and hand back the error. */
async function refusal(status: number, body: unknown): Promise<OidcExchangeError> {
  stubFetch(TARGET, {
    ...GITHUB_ID_TOKEN_ROUTES,
    "POST /api/auth/oidc/exchange": () => ({ status, body })
  });
  // No real wait: a 429 is retried once after its Retry-After.
  const error = await exchangeOidc(TARGET, {
    serviceAccount: ACCOUNT,
    sleep: async () => {}
  }).catch((e: unknown) => e);
  expect(error).toBeInstanceOf(OidcExchangeError);
  return error as OidcExchangeError;
}

beforeEach(() => {
  resetOidcExchanges();
  inGithubActions(TARGET);
});

afterEach(async () => {
  await runExitRevokes();
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
});

describe("githubOidcAvailable", () => {
  it("needs both variables — one without the other cannot mint", () => {
    expect(githubOidcAvailable({})).toBe(false);
    expect(githubOidcAvailable({ ACTIONS_ID_TOKEN_REQUEST_URL: "https://x" })).toBe(false);
    expect(
      githubOidcAvailable({
        ACTIONS_ID_TOKEN_REQUEST_URL: "https://x",
        ACTIONS_ID_TOKEN_REQUEST_TOKEN: "t"
      })
    ).toBe(true);
  });
});

describe("exchangeOidc", () => {
  it("asks GitHub for a token for the host it is about to post to, and asks the deployment nothing", async () => {
    const calls = stubFetch(TARGET, {
      ...GITHUB_ID_TOKEN_ROUTES,
      // A deployment that WOULD name an audience — another deployment's — to
      // show it is never asked, and so cannot.
      "GET /api/auth/oidc/audience": () => ({
        status: 200,
        body: { audience: "oxy:app.oxygen-hq.com" }
      }),
      "POST /api/auth/oidc/exchange": () => ({ status: 200, body: MINTED })
    });

    const credential = await exchangeOidc(TARGET, { serviceAccount: ACCOUNT });

    // Two requests, and the token is for the target's own host.
    expect(oidcAudience(TARGET)).toBe("oxy:oxy.test");
    expect(calls.map((c) => `${c.method} ${c.path}`)).toEqual([
      "GET /__gh/token?api-version=2.0&audience=oxy%3Aoxy.test",
      "POST /api/auth/oidc/exchange"
    ]);
    expect(calls.some((c) => c.path.startsWith("/api/auth/oidc/audience"))).toBe(false);
    // GitHub's request token authenticates the mint; the id token travels in
    // the BODY of the exchange, which is public and takes no credential.
    expect(calls[0]?.headers.authorization).toBe("bearer gh-request-token");
    expect(calls[1]?.headers.authorization).toBeUndefined();
    expect(JSON.parse(calls[1]?.body ?? "{}")).toEqual({
      token: "gh-jwt-oxy",
      service_account: ACCOUNT
    });
    expect(credential).toEqual({
      token: "oxy_ci_minted",
      tokenId: "tok-1",
      expiresAt: "2099-01-01T00:00:00Z",
      serviceAccount: "acme/deployer",
      grants: MINTED.grants
    });
  });

  /**
   * URL → audience. THE SAME LIST is in `crates/auth/src/github_oidc/audience.rs`
   * (the server deriving its own, from the URL it calls itself by) and in
   * `sdk/setup-oxyc/test/main.test.mjs`. The three must never disagree: a
   * client that derived anything else would be refused by the very deployment
   * it was pointed at. Change one, change all.
   */
  const AUDIENCE_CASES: Array<[string, string]> = [
    // The three deployments, as `oxyc --env` and `OXY_API_URL` name them.
    ["https://app.oxygen-hq.com", "oxy:app.oxygen-hq.com"],
    ["https://aip.dev.oxy.tech", "oxy:aip.dev.oxy.tech"],
    ["https://aip.staging.oxy.tech", "oxy:aip.staging.oxy.tech"],
    // The scheme's default port is dropped…
    ["https://app.oxygen-hq.com:443", "oxy:app.oxygen-hq.com"],
    ["http://app.oxygen-hq.com:80", "oxy:app.oxygen-hq.com"],
    // …and any other port is kept: it is part of which deployment it is.
    ["http://localhost:3000", "oxy:localhost:3000"],
    ["https://app.oxygen-hq.com:8443", "oxy:app.oxygen-hq.com:8443"],
    ["http://localhost:443", "oxy:localhost:443"],
    // The host is lowercased.
    ["https://App.Oxygen-HQ.com", "oxy:app.oxygen-hq.com"],
    // Only the address counts: not a path such as `/api`, nor a slash.
    ["https://app.oxygen-hq.com/api", "oxy:app.oxygen-hq.com"],
    ["https://app.oxygen-hq.com/", "oxy:app.oxygen-hq.com"],
    ["https://app.oxygen-hq.com/api/", "oxy:app.oxygen-hq.com"],
    ["https://app.oxygen-hq.com:443/api?x=1#y", "oxy:app.oxygen-hq.com"],
    // An IPv6 literal keeps its brackets.
    ["http://[::1]:3000", "oxy:[::1]:3000"],
    ["https://[2001:db8::1]", "oxy:[2001:db8::1]"],
    ["https://[2001:db8::1]:443/api", "oxy:[2001:db8::1]"],
    // Credentials in a URL are no part of the address.
    ["https://user:secret@app.oxygen-hq.com", "oxy:app.oxygen-hq.com"]
  ];

  it("derives the audience from the target exactly as the server derives its own", () => {
    for (const [url, audience] of AUDIENCE_CASES) {
      expect(oidcAudience(url), url).toBe(audience);
      // Never the plain one: that is a deployment with no public URL, and a
      // token for it would be good at every such deployment.
      expect(oidcAudience(url)).not.toBe("oxy");
    }
    for (const notAUrl of ["", "app.oxygen-hq.com", "localhost:3000", "not a url"]) {
      expect(() => oidcAudience(notAUrl), notAUrl).toThrow(CliError);
    }
  });

  /**
   * NO FALLBACK. A server that answers 404 to everything — an old deployment,
   * or one pretending to be — is still only ever sent a token for its own
   * host. There is no shared audience for it to be given instead.
   */
  it("has no fallback audience: a deployment with no exchange is still only sent a token for its own host", async () => {
    const calls = stubFetch(TARGET, GITHUB_ID_TOKEN_ROUTES);
    const error = (await exchangeOidc(TARGET, { serviceAccount: ACCOUNT }).catch(
      (e: unknown) => e
    )) as OidcExchangeError;
    expect(error.oidcCode).toBe("unsupported");
    expect(calls.map((c) => `${c.method} ${c.path}`)).toEqual([
      "GET /__gh/token?api-version=2.0&audience=oxy%3Aoxy.test",
      "POST /api/auth/oidc/exchange"
    ]);
  });

  it("with no service account named, asks nobody: no id token is minted and nothing is posted", async () => {
    // A stranger can register a trust policy that names this repository, so
    // "whichever policy matches" is not a question oxyc ever puts to a server.
    const calls = stubFetch(TARGET, {
      ...GITHUB_ID_TOKEN_ROUTES,
      "POST /api/auth/oidc/exchange": () => ({ status: 200, body: MINTED })
    });
    for (const serviceAccount of [undefined, "", "   "]) {
      const error = (await exchangeOidc(TARGET, { serviceAccount }).catch(
        (e: unknown) => e
      )) as OidcExchangeError;
      expect(error).toBeInstanceOf(OidcExchangeError);
      expect(error.oidcCode).toBe("no_service_account");
      expect(error.status).toBe(0);
      expect(error.code).toBe(ExitCode.AUTH);
      expect(error.message).toBe(`not authenticated for ${TARGET}`);
      expect(error.hint).toContain("OXY_SERVICE_ACCOUNT");
      expect(error.hint).toContain("--service-account");
      expect(error.hint).toContain("OXY_TOKEN");
    }
    expect(calls).toEqual([]);
  });

  it("keeps a target's path prefix", async () => {
    const calls = stubFetch(TARGET, {
      ...GITHUB_ID_TOKEN_ROUTES,
      "POST /oxy/api/auth/oidc/exchange": () => ({ status: 200, body: MINTED })
    });
    await exchangeOidc(`${TARGET}/oxy/`, { serviceAccount: ACCOUNT });
    // The audience is the HOST: a path on the target is no part of it.
    expect(calls[0]?.path).toBe("/__gh/token?api-version=2.0&audience=oxy%3Aoxy.test");
    expect(calls[1]?.path).toBe("/oxy/api/auth/oidc/exchange");
  });

  it("reports GitHub refusing to mint as its own error, with the permission to add", async () => {
    stubFetch(TARGET, {
      "GET /__gh/token?api-version=2.0&audience=oxy%3Aoxy.test": () => ({ status: 403, body: {} })
    });
    const error = (await exchangeOidc(TARGET, { serviceAccount: ACCOUNT }).catch(
      (e: unknown) => e
    )) as OidcExchangeError;
    expect(error).toBeInstanceOf(OidcExchangeError);
    expect(error.oidcCode).toBe("github");
    expect(error.code).toBe(ExitCode.AUTH);
    expect(error.hint).toContain("id-token: write");
  });

  it("refuses a 200 that carries no token", async () => {
    stubFetch(TARGET, {
      ...GITHUB_ID_TOKEN_ROUTES,
      "POST /api/auth/oidc/exchange": () => ({ status: 200, body: { expires_at: "later" } })
    });
    await expect(exchangeOidc(TARGET, { serviceAccount: ACCOUNT })).rejects.toThrow(
      /returned no token/
    );
  });
});

describe("each refusal the contract defines", () => {
  /** code → [status, exit code, a word its message must carry, a word its hint must]. */
  const CASES: Array<[string, number, number, RegExp, RegExp]> = [
    ["invalid_token", 401, ExitCode.AUTH, /rejected the GitHub OIDC token/, /signature or issuer/],
    ["wrong_audience", 401, ExitCode.AUTH, /wrong audience/, /calls itself by another/],
    ["expired", 401, ExitCode.AUTH, /expired before it was exchanged/, /re-run the job/],
    ["replayed", 401, ExitCode.AUTH, /already been used/, /single-use/],
    ["pull_request_target", 403, ExitCode.AUTH, /pull_request_target/, /`push`, `pull_request`/],
    ["self_hosted_runner", 403, ExitCode.AUTH, /self-hosted runner/, /GitHub-hosted runners only/],
    [
      "missing_environment",
      403,
      ExitCode.AUTH,
      /environment is required and missing/,
      /environment: <name>[\s\S]*org admin sets it on the policy/
    ],
    [
      "no_matching_policy",
      403,
      ExitCode.AUTH,
      /no trust policy of the named service account matches/,
      /oxyc init-ci/
    ],
    [
      "service_account_required",
      400,
      ExitCode.REQUEST,
      /must be named by its ID/,
      /takes the account's ID \(a UUID\), never <org-slug>\/<name>/
    ]
  ];

  for (const [code, status, exit, message, hint] of CASES) {
    it(`${code} (${status})`, async () => {
      const error = await refusal(status, { error: "the server's own words", code });
      expect(error.oidcCode).toBe(code);
      expect(error.status).toBe(status);
      expect(error.code).toBe(exit);
      expect(error.message).toMatch(message);
      expect(error.hint ?? "").toMatch(hint);
    });
  }

  it("wrong_audience names the address the deployment answers to, and says to point --target at it", async () => {
    // The fix is where oxyc is pointed — never which audience it asks for.
    const error = await refusal(401, { code: "wrong_audience", audience: "oxy:app.oxygen-hq.com" });
    expect(error.message).toMatch(/answers to another address/);
    expect(error.hint).toContain("this deployment answers to app.oxygen-hq.com");
    expect(error.hint).toContain("(here oxy.test)");
    expect(error.hint).toContain("Point --target (or --env) at https://app.oxygen-hq.com");
  });

  it("wrong_audience from a deployment with no public URL says GitHub sign-in is not available there", async () => {
    // Plain `oxy` is what a deployment with no OXY_API_URL takes. oxyc never
    // asks for it: a token for it would be good at every such deployment.
    const error = await refusal(401, { code: "wrong_audience", audience: "oxy" });
    expect(error.message).toBe("GitHub sign-in is not available on this deployment");
    expect(error.hint).toContain("no public URL configured (OXY_API_URL)");
    expect(error.hint).toContain("OXY_TOKEN");
    expect(error.code).toBe(ExitCode.AUTH);
  });

  it("no_matching_policy says where to register one, and what this run looks like", async () => {
    vi.stubEnv("GITHUB_REPOSITORY", "acme-co/acme-apps");
    vi.stubEnv(
      "GITHUB_WORKFLOW_REF",
      "acme-co/acme-apps/.github/workflows/release.yml@refs/heads/main"
    );
    const error = await refusal(403, { error: "nope", code: "no_matching_policy" });
    expect(error.hint).toContain("Service accounts");
    expect(error.hint).toContain("environment:");
    // The three things a policy matches on, as GitHub reports them for THIS run.
    expect(error.detail).toContain("acme-co/acme-apps");
    expect(error.detail).toContain(".github/workflows/release.yml");
  });

  it("`ambiguous` is not a code any more: a 409 is just an unknown refusal", async () => {
    // The run names its account, so two accounts trusting one run is not a
    // conflict the server can report — and nothing here lists candidates.
    const error = await refusal(409, {
      error: "several policies match",
      code: "ambiguous",
      candidates: ["acme/deployer", "acme/ci"]
    });
    expect(error.oidcCode).toBe("unknown");
    expect(error).not.toHaveProperty("candidates");
  });

  it("404 is a deployment with no exchange: the old 'not authenticated', with the reason", async () => {
    const error = await refusal(404, { error: "not found" });
    expect(error.oidcCode).toBe("unsupported");
    // The same message and exit code a missing credential always produced.
    expect(error.message).toBe(`not authenticated for ${TARGET}`);
    expect(error.code).toBe(ExitCode.AUTH);
    expect(error.detail).toContain("/api/auth/oidc/exchange");
    expect(error.hint).toContain("OXY_TOKEN");
  });

  it("an unknown code keeps the server's words rather than inventing a reason", async () => {
    const error = await refusal(403, { error: "policy disabled by the org", code: "brand_new" });
    expect(error.oidcCode).toBe("unknown");
    expect(error.detail).toContain("policy disabled by the org");
  });

  it("a 400 with no code is an unreadable request", async () => {
    const error = await refusal(400, { error: "'service_account' must be '<org_slug>/<name>'" });
    expect(error.oidcCode).toBe("malformed");
    expect(error.code).toBe(ExitCode.REQUEST);
    expect(error.detail).toContain("'service_account' must be");
    expect(error.hint).toContain("rewriting request bodies");
  });

  it("only no account named, no exchange or no matching policy leaves the app's publisher worth trying", async () => {
    const tried: Record<string, boolean> = {};
    for (const [code, status] of [
      ...CASES.map(([c, s]) => [c, s] as const),
      ["rate_limited", 429] as const
    ]) {
      tried[code] = fallsBackToPublisher(await refusal(status, { code }));
    }
    tried.malformed = fallsBackToPublisher(await refusal(400, {}));
    tried.unsupported = fallsBackToPublisher(await refusal(404, {}));
    // Not `service_account_required`: a workflow that names its account badly
    // — by name, say — has a line to fix, not a second door to try.
    expect(Object.keys(tried).filter((code) => tried[code])).toEqual([
      "no_matching_policy",
      "unsupported"
    ]);
    // And the refusal oxyc raises on its own, having asked nobody.
    const unnamed = (await exchangeOidc(TARGET).catch((e: unknown) => e)) as OidcExchangeError;
    expect(unnamed.oidcCode).toBe("no_service_account");
    expect(fallsBackToPublisher(unnamed)).toBe(true);
  });
});

describe("a rate-limited exchange (429)", () => {
  const LIMITED = { error: "too many requests; try again later", code: "rate_limited" };

  it("reads Retry-After as whole seconds, capped, and a missing one as a second", () => {
    expect(retryAfterSeconds("7")).toBe(7);
    expect(retryAfterSeconds("600")).toBe(RATE_LIMIT_MAX_WAIT_S);
    expect(retryAfterSeconds(null)).toBe(1);
    expect(retryAfterSeconds("0")).toBe(1);
    expect(retryAfterSeconds("Wed, 21 Oct 2026 07:28:00 GMT")).toBe(1);
  });

  it("waits the server's Retry-After once, then retries with a fresh id token", async () => {
    let posts = 0;
    const calls = stubFetch(TARGET, {
      ...GITHUB_ID_TOKEN_ROUTES,
      "POST /api/auth/oidc/exchange": () =>
        ++posts === 1
          ? { status: 429, body: LIMITED, headers: { "retry-after": "7" } }
          : { status: 200, body: MINTED }
    });
    const waits: number[] = [];
    const credential = await exchangeOidc(TARGET, {
      serviceAccount: ACCOUNT,
      sleep: async (ms) => {
        waits.push(ms);
      }
    });
    expect(credential.token).toBe("oxy_ci_minted");
    expect(waits).toEqual([7_000]);
    // Two id tokens, one per attempt: a jti is never offered twice.
    expect(calls.map((c) => `${c.method} ${c.path}`)).toEqual([
      "GET /__gh/token?api-version=2.0&audience=oxy%3Aoxy.test",
      "POST /api/auth/oidc/exchange",
      "GET /__gh/token?api-version=2.0&audience=oxy%3Aoxy.test",
      "POST /api/auth/oidc/exchange"
    ]);
  });

  it("retries once only, caps the wait, and then says it is rate-limited — retryably", async () => {
    const calls = stubFetch(TARGET, {
      ...GITHUB_ID_TOKEN_ROUTES,
      "POST /api/auth/oidc/exchange": () => ({
        status: 429,
        body: LIMITED,
        headers: { "retry-after": "3600" }
      })
    });
    const waits: number[] = [];
    const error = (await exchangeOidc(TARGET, {
      serviceAccount: ACCOUNT,
      sleep: async (ms) => {
        waits.push(ms);
      }
    }).catch((e: unknown) => e)) as OidcExchangeError;
    expect(error).toBeInstanceOf(OidcExchangeError);
    expect(waits).toEqual([RATE_LIMIT_MAX_WAIT_S * 1000]);
    expect(calls.filter((c) => c.method === "POST")).toHaveLength(2);
    expect(error.oidcCode).toBe("rate_limited");
    expect(error.status).toBe(429);
    expect(error.code).toBe(ExitCode.UNAVAILABLE);
    expect(error.message).toMatch(/rate-limiting/);
    expect(error.hint).toContain("oxyc token");
  });
});

describe("exchangeOidcOnce", () => {
  it("exchanges once per process, however many times the bearer is asked for", async () => {
    const calls = stubFetch(TARGET, {
      ...GITHUB_ID_TOKEN_ROUTES,
      "POST /api/auth/oidc/exchange": () => ({ status: 200, body: MINTED }),
      "DELETE /api/auth/token": () => ({ status: 204 })
    });

    const [a, b] = await Promise.all([
      exchangeOidcOnce(TARGET, ACCOUNT),
      exchangeOidcOnce(TARGET, ACCOUNT)
    ]);
    const c = await exchangeOidcOnce(TARGET, ACCOUNT);

    expect(a.token).toBe("oxy_ci_minted");
    expect(b).toBe(a);
    expect(c).toBe(a);
    // One id token minted, one exchange: the id token is single-use.
    expect(calls.filter((x) => x.path.includes("/__gh/token"))).toHaveLength(1);
    expect(calls.filter((x) => x.path === "/api/auth/oidc/exchange")).toHaveLength(1);
  });

  it("queues what it minted for revocation on exit", async () => {
    stubFetch(TARGET, {
      ...GITHUB_ID_TOKEN_ROUTES,
      "POST /api/auth/oidc/exchange": () => ({ status: 200, body: MINTED }),
      "DELETE /api/auth/token": () => ({ status: 204 })
    });
    await exchangeOidcOnce(TARGET, ACCOUNT);
    expect(pendingRevokes()).toEqual(["oxy_ci_minted"]);
  });

  it("remembers a refusal too, rather than spending a second id token on the same answer", async () => {
    const calls = stubFetch(TARGET, {
      ...GITHUB_ID_TOKEN_ROUTES,
      "POST /api/auth/oidc/exchange": () => ({
        status: 403,
        body: { code: "no_matching_policy" }
      })
    });
    await expect(exchangeOidcOnce(TARGET, ACCOUNT)).rejects.toThrow(/no trust policy/);
    await expect(exchangeOidcOnce(TARGET, ACCOUNT)).rejects.toThrow(/no trust policy/);
    expect(calls.filter((x) => x.path === "/api/auth/oidc/exchange")).toHaveLength(1);
    expect(pendingRevokes()).toEqual([]);
  });

  it("treats a different service account as a different exchange", async () => {
    const calls = stubFetch(TARGET, {
      ...GITHUB_ID_TOKEN_ROUTES,
      "POST /api/auth/oidc/exchange": () => ({ status: 200, body: MINTED }),
      "DELETE /api/auth/token": () => ({ status: 204 })
    });
    await exchangeOidcOnce(TARGET, ACCOUNT);
    await exchangeOidcOnce(TARGET, "9b2c1d3e-5f60-4a7b-8c9d-0e1f2a3b4c5d");
    expect(calls.filter((x) => x.path === "/api/auth/oidc/exchange")).toHaveLength(2);
  });
});
