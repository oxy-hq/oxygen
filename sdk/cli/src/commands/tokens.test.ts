/**
 * `oxyc tokens list | revoke | create`.
 *
 * The server refuses token management to a token — 403 `session_required` —
 * and `oxyc login` now caches a token, so that refusal is what these commands
 * usually meet. The property worth pinning is that it comes back as a pointer
 * to the page that CAN do it, with the auth exit code, rather than as a bare
 * 403.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createContext } from "../context/resolve.js";
import { stubFetch } from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import { runTokensCreate, runTokensList, runTokensRevoke, tokensPageUrl } from "./tokens.js";

const TARGET = "https://oxy.test";
const PAGE = "https://oxy.test/?settings=account.tokens";

const SESSION_REQUIRED = {
  status: 403,
  body: { error: "this route requires a browser session", code: "session_required" }
};

const LISTED = {
  tokens: [
    {
      id: "tok-1",
      name: "oxyc on laptop",
      kind: "personal",
      display_prefix: "oxy_pat_ab12",
      last_four: "9f3c",
      all_access: true,
      platform: false,
      partner: false,
      grants: [],
      expires_at: null,
      last_used_at: "2026-09-30T10:00:00Z",
      status: "active"
    },
    {
      id: "tok-2",
      name: "ci on staging",
      kind: "personal",
      display_prefix: "oxy_pat_1a2b",
      last_four: "77aa",
      all_access: true,
      platform: false,
      partner: false,
      // `grants` and `blocked_orgs` omitted, as a lenient server might.
      expires_at: null,
      last_used_at: null,
      status: "active"
    }
  ]
};

let scratch: string;
let stdout: string;
let stderr: string;

const context = () => createContext({ env: "production", target: TARGET }, scratch);

async function refusal(run: Promise<unknown>): Promise<CliError> {
  const cause = await run.then(
    () => undefined,
    (thrown: unknown) => thrown
  );
  expect(cause).toBeInstanceOf(CliError);
  return cause as CliError;
}

beforeEach(() => {
  scratch = mkdtempSync(join(tmpdir(), "oxyc-tokens-"));
  vi.stubEnv("OXY_CREDENTIALS_PATH", join(scratch, "credentials.json"));
  vi.stubEnv("OXY_TOKEN", "oxy_pat_secret");
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_URL", "");
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "");
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

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  rmSync(scratch, { recursive: true, force: true });
});

describe("tokensPageUrl", () => {
  it("deep-links Account → Personal access tokens, whatever the target's trailing slash", () => {
    expect(tokensPageUrl(TARGET)).toBe(PAGE);
    expect(tokensPageUrl(`${TARGET}/`)).toBe(PAGE);
  });
});

describe("oxyc tokens list", () => {
  it("prints one row per token the server lists, and the server lists tokens only", async () => {
    const calls = stubFetch(TARGET, {
      "GET /api/user/tokens": () => ({ status: 200, body: LISTED })
    });
    await runTokensList(context(), false);
    expect(calls[0]?.headers.authorization).toBe("Bearer oxy_pat_secret");
    expect(stdout).toContain("tok-1");
    expect(stdout).toContain("oxyc on laptop");
    expect(stdout).toContain("tok-2");
    expect(stdout).toContain("ci on staging");
    expect(stdout).toContain("all access");
    expect(stdout).toContain("never");
    // No client-side legacy handling: a legacy API key is not a token and is never listed.
    expect(stdout).not.toMatch(/legacy/i);
  });

  it("lists an agent token as one, apart from an oxyc login", async () => {
    const [login] = LISTED.tokens;
    stubFetch(TARGET, {
      "GET /api/user/tokens": () => ({
        status: 200,
        body: {
          tokens: [
            { ...login, source: "oxyc_login" },
            { ...login, id: "tok-9", name: "agent on laptop", source: "oxyc_agent", platform: true }
          ]
        }
      })
    });
    await runTokensList(context(), false);
    const row = (id: string) => stdout.split("\n").find((line) => line.includes(id)) ?? "";
    expect(row("tok-9")).toMatch(/\bagent\b/);
    expect(row("tok-9")).not.toMatch(/\bpersonal\b/);
    expect(row("tok-9")).toContain("platform standing");
    expect(row("tok-1")).toMatch(/\bpersonal\b/);
    expect(row("tok-1")).not.toMatch(/\bagent\b/);
  });

  it("--json is the server's response, untouched", async () => {
    stubFetch(TARGET, { "GET /api/user/tokens": () => ({ status: 200, body: LISTED }) });
    await runTokensList(context(), true);
    expect(JSON.parse(stdout)).toEqual(LISTED);
  });

  it("says where to make one when there are none", async () => {
    stubFetch(TARGET, { "GET /api/user/tokens": () => ({ status: 200, body: { tokens: [] } }) });
    await runTokensList(context(), false);
    expect(stdout).toBe("");
    expect(stderr).toContain("oxyc tokens create");
  });

  it("turns 403 session_required into a pointer at the web page, exit code AUTH", async () => {
    stubFetch(TARGET, { "GET /api/user/tokens": () => SESSION_REQUIRED });
    const cause = await refusal(runTokensList(context(), false));
    expect(cause.code).toBe(ExitCode.AUTH);
    expect(cause.message).toContain("needs a browser session");
    expect(cause.hint).toContain(PAGE);
    expect(cause.hint).toContain("Account → Personal access tokens");
    expect(cause.hint).toContain("oxyc logout");
  });

  it("says a deployment without the route predates personal tokens (NOT_FOUND)", async () => {
    stubFetch(TARGET, {});
    const cause = await refusal(runTokensList(context(), false));
    expect(cause.code).toBe(ExitCode.NOT_FOUND);
    expect(cause.message).toContain("no personal access tokens");
    expect(cause.hint).toContain("API Keys");
  });

  it("leaves any other 403 as the server's own refusal", async () => {
    stubFetch(TARGET, {
      "GET /api/user/tokens": () => ({ status: 403, body: { error: "forbidden" } })
    });
    const cause = await refusal(runTokensList(context(), false));
    expect(cause.code).toBe(ExitCode.AUTH);
    expect(cause.message).not.toContain("browser session");
  });
});

describe("oxyc tokens revoke", () => {
  it("deletes the token by id and says so", async () => {
    const calls = stubFetch(TARGET, { "DELETE /api/user/tokens/tok-1": () => ({ status: 204 }) });
    await runTokensRevoke(context(), "tok-1");
    expect(calls).toHaveLength(1);
    expect(stderr).toContain("Revoked tok-1.");
  });

  it("escapes the id, so it can never address another route", async () => {
    const calls = stubFetch(TARGET, {});
    await refusal(runTokensRevoke(context(), "../orgs"));
    expect(calls[0]?.path).toBe("/api/user/tokens/..%2Forgs");
  });

  it("reads 404 as not-yours — the same answer someone else's token gives", async () => {
    stubFetch(TARGET, {});
    const cause = await refusal(runTokensRevoke(context(), "tok-9"));
    expect(cause.code).toBe(ExitCode.NOT_FOUND);
    expect(cause.message).toContain("no token tok-9 of yours");
    // A legacy API key's id answers 404 here too: it is not a token. Say where those live.
    expect(cause.hint).toContain("a legacy API key is not a token");
    expect(cause.hint).toContain("Workspace → Legacy API keys");
  });

  it("turns 403 session_required into the same pointer as list", async () => {
    stubFetch(TARGET, { "DELETE /api/user/tokens/tok-1": () => SESSION_REQUIRED });
    const cause = await refusal(runTokensRevoke(context(), "tok-1"));
    expect(cause.code).toBe(ExitCode.AUTH);
    expect(cause.hint).toContain(PAGE);
  });
});

describe("oxyc tokens create", () => {
  it("opens the settings page and never calls the API", () => {
    const calls = stubFetch(TARGET, {});
    const opened: string[] = [];
    runTokensCreate(context(), (url) => opened.push(url));
    expect(opened).toEqual([PAGE]);
    expect(calls).toHaveLength(0);
    // The URL is the command's stdout, for a terminal with no browser to open.
    expect(stdout).toBe(`${PAGE}\n`);
  });
});
