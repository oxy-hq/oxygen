/**
 * The user agent: `oxyc/<version>[ agent/<label>][ mcp]`, on every request to
 * a deployment — the generic client, and each request that builds its own
 * `fetch` (the PKCE exchange, the OIDC exchange, a function call, a publish).
 *
 * The environment is stubbed in every case: these tests are themselves often
 * run by an agent, whose harness sets the very marker being detected.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { runExitRevokes } from "../auth/exit-revoke.js";
import { exchangeCliCode } from "../auth/login.js";
import { exchangeOidcOnce, resetOidcExchanges } from "../auth/oidc.js";
import { VERSION } from "../generated/version.js";
import { fetchProject, uploadBundle } from "../publish/server.js";
import {
  GITHUB_ID_TOKEN_ROUTES,
  inGithubActions,
  SERVICE_ACCOUNT_ID,
  stubFetch
} from "../testing/stub-fetch.js";
import { request } from "./request.js";
import {
  agentLabel,
  markMcpSession,
  sanitizeAgentLabel,
  userAgent,
  withUserAgent
} from "./user-agent.js";

const TARGET = "https://oxy.test";
const PLAIN = `oxyc/${VERSION}`;

beforeEach(() => {
  vi.stubEnv("OXY_AGENT", "");
  vi.stubEnv("CLAUDECODE", "");
  markMcpSession(false);
  resetOidcExchanges();
});

afterEach(async () => {
  // An OIDC token minted by a case is revoked while its stub still answers.
  await runExitRevokes();
  markMcpSession(false);
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
});

describe("the string", () => {
  it("is oxyc/<version> and nothing else for a person at a terminal", () => {
    expect(userAgent({})).toBe(PLAIN);
    expect(PLAIN).toMatch(/^oxyc\/\d+\.\d+\.\d+$/);
  });

  it("appends agent/<label> from OXY_AGENT", () => {
    expect(userAgent({ OXY_AGENT: "release-bot" })).toBe(`${PLAIN} agent/release-bot`);
  });

  it("detects Claude Code by the variable it exports, with no OXY_AGENT", () => {
    expect(userAgent({ CLAUDECODE: "1" })).toBe(`${PLAIN} agent/claude-code`);
    // Set-but-blank is not set: a harness that clears it for a child is not driving it.
    expect(userAgent({ CLAUDECODE: " " })).toBe(PLAIN);
  });

  it("lets OXY_AGENT name the agent even inside a detected harness", () => {
    expect(agentLabel({ OXY_AGENT: "store-ops-agent", CLAUDECODE: "1" })).toBe("store-ops-agent");
  });

  it("falls back to the detected harness when OXY_AGENT has nothing usable in it", () => {
    expect(agentLabel({ OXY_AGENT: "!!!", CLAUDECODE: "1" })).toBe("claude-code");
    expect(agentLabel({ OXY_AGENT: "!!!" })).toBeUndefined();
  });

  it("appends mcp last, for a session served by `oxyc mcp`", () => {
    markMcpSession();
    expect(userAgent({})).toBe(`${PLAIN} mcp`);
    expect(userAgent({ OXY_AGENT: "codex" })).toBe(`${PLAIN} agent/codex mcp`);
  });
});

describe("sanitising the label", () => {
  it.each([
    ["claude-code", "claude-code"],
    ["v1.2_beta-3", "v1.2_beta-3"],
    ["Claude Code", "claude-code"],
    ["Claude Code/2.1", "claude-code-2.1"],
    ["  spaced  ", "spaced"],
    ["a b\tc\nd", "a-b-c-d"],
    ["agent/../../etc", "agent-..-..-etc"],
    ["naïve-agent", "na-ve-agent"],
    ["--edges--", "edges"]
  ])("%j → %j", (raw, expected) => {
    expect(sanitizeAgentLabel(raw)).toBe(expected);
  });

  it("keeps only lowercase letters, digits, dot, underscore and hyphen", () => {
    const label = sanitizeAgentLabel('My "Agent" <v2> (prod); DROP TABLE\r\nX-Injected: 1') ?? "";
    expect(label).toMatch(/^[a-z0-9._-]+$/);
  });

  it("stops at 32 characters, and never ends on the separator it inserted", () => {
    expect(sanitizeAgentLabel("a".repeat(80))).toBe("a".repeat(32));
    expect(sanitizeAgentLabel(`${"a".repeat(31)} bcd`)).toBe("a".repeat(31));
  });

  it("is no label at all when nothing usable is left", () => {
    for (const raw of ["", "   ", "!!!", "—", "///"]) {
      expect(sanitizeAgentLabel(raw)).toBeUndefined();
    }
  });
});

describe("on the wire", () => {
  const ok = () => ({ status: 200, body: {} });

  it("the generic client sends it — plain, then with OXY_AGENT, then detected", async () => {
    const calls = stubFetch(TARGET, { "GET /api/orgs": ok });
    const send = () => request({ target: TARGET, path: "/api/orgs", method: "GET" });

    await send();
    vi.stubEnv("OXY_AGENT", "Release Bot");
    await send();
    vi.stubEnv("OXY_AGENT", "");
    vi.stubEnv("CLAUDECODE", "1");
    await send();

    expect(calls.map((c) => c.headers["user-agent"])).toEqual([
      PLAIN,
      `${PLAIN} agent/release-bot`,
      `${PLAIN} agent/claude-code`
    ]);
  });

  it("a caller's own -H user-agent still wins", async () => {
    const calls = stubFetch(TARGET, { "GET /api/orgs": ok });
    await request({
      target: TARGET,
      path: "/api/orgs",
      method: "GET",
      headers: { "User-Agent": "curl/8" }
    });
    expect(calls[0]?.headers["user-agent"]).toBe("curl/8");
    expect(withUserAgent({ "User-Agent": "curl/8" })).toEqual({ "User-Agent": "curl/8" });
  });

  it("the PKCE exchange sends it", async () => {
    vi.stubEnv("OXY_AGENT", "codex");
    const calls = stubFetch(TARGET, {
      "POST /api/auth/cli/exchange": () => ({
        status: 200,
        body: { token: { id: "t" }, secret: "oxy_pat_x" }
      })
    });
    await exchangeCliCode(TARGET, "code", "verifier");
    expect(calls[0]?.headers["user-agent"]).toBe(`${PLAIN} agent/codex`);
  });

  it("the OIDC exchange sends it to the deployment, and not to GitHub", async () => {
    vi.stubEnv("OXY_AGENT", "ci-agent");
    const calls = stubFetch(TARGET, {
      ...GITHUB_ID_TOKEN_ROUTES,
      "POST /api/auth/oidc/exchange": () => ({
        status: 200,
        body: { token: "oxy_ci_minted", token_id: "t", expires_at: "2099-01-01T00:00:00Z" }
      }),
      "DELETE /api/auth/token": () => ({ status: 204 })
    });
    inGithubActions(TARGET);
    await exchangeOidcOnce(TARGET, SERVICE_ACCOUNT_ID);

    const exchange = calls.find((c) => c.path === "/api/auth/oidc/exchange");
    expect(exchange?.headers["user-agent"]).toBe(`${PLAIN} agent/ci-agent`);
    const github = calls.find((c) => c.path.startsWith("/__gh/token"));
    expect(github?.headers["user-agent"]).toBeUndefined();
  });

  it("a publish sends it on the public lookup and on the upload", async () => {
    vi.stubEnv("CLAUDECODE", "1");
    const calls = stubFetch(TARGET, {
      "GET /api/apps/acme/store/build-config": () => ({ status: 200, body: { project_id: "p" } }),
      "POST /api/customer-apps/publish": () => ({
        status: 200,
        body: { app_id: "a", build_id: "b", url: "/x", channel: "draft" }
      })
    });
    await fetchProject(TARGET, "acme", "store");
    await uploadBundle({ target: TARGET, token: "tok", fields: [], tarball: Buffer.from("x") });
    expect(calls.map((c) => c.headers["user-agent"])).toEqual([
      `${PLAIN} agent/claude-code`,
      `${PLAIN} agent/claude-code`
    ]);
    // The upload still authenticates: the header was added, not swapped in.
    expect(calls[1]?.headers.authorization).toBe("Bearer tok");
  });
});
