/**
 * `oxyc tokens create --agent`, against a fake deployment and a fake browser:
 * the harness `tokens-sandbox.test.ts` uses, because this is the same loopback
 * with other parameters.
 *
 * What is pinned: every usage error lands before a browser opens; the
 * `/cli-auth` URL; stdout is the `export` line and nothing else; the
 * credentials file is never written and no other token is revoked; and a token
 * that is not the one asked for is revoked, unprinted, with exit 8. Less than
 * was asked (standing the approver left out) is a success that says so.
 */

import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { loadCredential, saveCredential } from "../auth/credentials.js";
import { challengeFor } from "../auth/pkce.js";
import { createContext } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import {
  type AgentMintFlags,
  agentMintQuery,
  parseAgentMint,
  runTokensCreateAgent
} from "./tokens-agent.js";

const SECRET = "oxy_pat_0123456789abcdefghijABCDEFGHIJ012345";

/** An RFC 3339 instant `hours` from now — what a token minted for that long carries. */
const inHours = (hours: number) => new Date(Date.now() + hours * 3_600_000).toISOString();

/** The token row the deployment describes: an agent token, 8 hours long, with no standing. */
let TOKEN_ROW: Record<string, unknown>;

let server: Server;
let target: string;
/** What `POST /api/auth/cli/exchange` answers. */
let exchangeReply: { status: number; body: unknown };
/** Fields laid over the row `GET /api/auth/token` describes, to make it say something else. */
let describedOver: Record<string, unknown>;
/** What `GET /api/auth/token` answers in place of the row, when it does not describe one. */
let describedInstead: { status: number; body?: unknown } | undefined;
let requests: Array<{ method: string; url: string; authorization?: string; body: string }>;
/** What the fake page hands the loopback: a code, or an old deployment's session token. */
let handsBack: "code" | "token";
let opened: URL[];
let scratch: string;
let credentialsFile: string;
let stdout: string;
let stderr: string;

async function read(req: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks).toString();
}

beforeAll(async () => {
  server = createServer((req, res) => {
    void (async () => {
      const body = await read(req);
      requests.push({
        method: req.method ?? "GET",
        url: req.url ?? "",
        authorization: req.headers.authorization,
        body
      });
      const reply = (status: number, value?: unknown) => {
        res.writeHead(status, { "content-type": "application/json" });
        res.end(value === undefined ? undefined : JSON.stringify(value));
      };
      if (req.url === "/api/auth/cli/exchange" && req.method === "POST") {
        return reply(exchangeReply.status, exchangeReply.body);
      }
      if (req.url === "/api/auth/token" && req.method === "DELETE") return reply(204);
      if (req.url === "/api/auth/token" && req.method === "GET") {
        if (describedInstead) return reply(describedInstead.status, describedInstead.body);
        return reply(200, { ...TOKEN_ROW, ...describedOver });
      }
      reply(404, { error: `unexpected ${req.url}` });
    })();
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  target = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});

afterAll(() => {
  server.close();
});

beforeEach(() => {
  scratch = mkdtempSync(join(tmpdir(), "oxyc-agent-mint-"));
  credentialsFile = join(scratch, "credentials.json");
  vi.stubEnv("OXY_CREDENTIALS_PATH", credentialsFile);
  vi.stubEnv("OXY_TOKEN", "");
  vi.stubEnv("OXYC_QUIET", "");
  TOKEN_ROW = {
    id: "tok-agent-1",
    name: "agent on test-laptop",
    kind: "personal",
    source: "oxyc_agent",
    display_prefix: "oxy_pat_0123",
    last_four: "2345",
    all_access: true,
    platform: false,
    partner: false,
    grants: [],
    expires_at: inHours(8),
    status: "active",
    owner: { type: "user", id: "u-1", label: "luong@oxy.tech" },
    blocked_orgs: []
  };
  exchangeReply = { status: 200, body: { token: TOKEN_ROW, secret: SECRET } };
  describedOver = {};
  describedInstead = undefined;
  requests = [];
  handsBack = "code";
  opened = [];
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
  vi.unstubAllEnvs();
  rmSync(scratch, { recursive: true, force: true });
});

/** The page: the operator approves, and it redirects to the loopback. */
function browser(url: string): void {
  const page = new URL(url);
  opened.push(page);
  const port = page.searchParams.get("port");
  const state = page.searchParams.get("state") ?? "";
  const handed = handsBack === "code" ? "code=one-time-code" : "token=eyJ.session.jwt";
  void fetch(`http://127.0.0.1:${port}/callback?${handed}&state=${encodeURIComponent(state)}`);
}

const context = () => createContext({ env: "production", target }, scratch);

const mint = (flags: AgentMintFlags = {}) =>
  runTokensCreateAgent(context(), flags, { open: browser, hostname: "test-laptop" });

async function failure(run: Promise<unknown>): Promise<CliError> {
  const cause = await run.then(
    () => undefined,
    (thrown: unknown) => thrown
  );
  expect(cause).toBeInstanceOf(CliError);
  return cause as CliError;
}

const sent = () => requests.map((r) => `${r.method} ${r.url}`);
const revokes = () => requests.filter((r) => r.method === "DELETE" && r.url === "/api/auth/token");

/** Both descriptions say it: the exchange's own, and the token's when it is asked. */
const describedAs = (over: Record<string, unknown>) => {
  describedOver = over;
  exchangeReply = { status: 200, body: { token: { ...TOKEN_ROW, ...over }, secret: SECRET } };
};

/**
 * Refused as every mismatch is: exit 8, nothing printed, nothing stored, and
 * the token it minted revoked with that token as the bearer.
 */
async function refused(run: Promise<unknown>, secret = SECRET): Promise<CliError> {
  const cause = await failure(run);
  expect(cause.code).toBe(ExitCode.REFUSED);
  expect(stdout).toBe("");
  expect(existsSync(credentialsFile)).toBe(false);
  expect(revokes().map((r) => r.authorization)).toEqual([`Bearer ${secret}`]);
  expect(cause.detail).toContain("What it minted was revoked, and nothing was kept.");
  expect(`${cause.message}\n${cause.detail}\n${cause.hint}`).not.toContain(secret);
  return cause;
}

describe("argument validation, before any browser opens", () => {
  const parse = (flags: AgentMintFlags) => parseAgentMint(flags, "test-laptop");

  it("defaults to 8 hours, no standing, and a name that says where it was minted", () => {
    expect(parse({})).toEqual({ standing: false, hours: 8, name: "agent on test-laptop" });
    expect(parse({ standing: true, hours: "24", name: "  triage run  " })).toEqual({
      standing: true,
      hours: 24,
      name: "triage run"
    });
  });

  it("takes 1 to 168 whole hours", () => {
    expect(parse({ hours: "1" }).hours).toBe(1);
    expect(parse({ hours: "168" }).hours).toBe(168);
    for (const hours of ["0", "169", "1.5", "-3", "soon", "", "8h"]) {
      expect(() => parse({ hours }), `--hours ${hours}`).toThrow(/is not a whole number from 1/);
    }
  });

  it("refuses an empty or over-long name, and cuts its own default to the limit", () => {
    expect(() => parse({ name: "   " })).toThrow("--name is empty");
    expect(() => parse({ name: "x".repeat(101) })).toThrow("the limit is 100");
    expect(parseAgentMint({}, "h".repeat(200)).name).toHaveLength(100);
  });

  it("is a usage error (exit 2), and nothing was opened or requested", async () => {
    for (const flags of [{ hours: "200" }, { hours: "0", standing: true }, { name: "" }]) {
      expect((await failure(mint(flags))).code).toBe(ExitCode.USAGE);
    }
    expect(opened).toHaveLength(0);
    expect(requests).toHaveLength(0);
    expect(stdout).toBe("");
  });
});

describe("the /cli-auth URL", () => {
  it("adds kind, hours and name, and standing only when it was asked for", () => {
    expect(agentMintQuery({ standing: false, hours: 8, name: "triage run" })).toBe(
      "kind=agent&hours=8&name=triage%20run"
    );
    expect(agentMintQuery({ standing: true, hours: 24, name: "a&b=c" })).toBe(
      "kind=agent&hours=24&standing=1&name=a%26b%3Dc"
    );
  });

  it("opens it on the deployment, with a PKCE challenge and this machine's hostname", async () => {
    describedAs({ platform: true, expires_at: inHours(24), name: "triage run" });
    await mint({ standing: true, hours: "24", name: "triage run" });

    expect(opened).toHaveLength(1);
    const page = opened[0] as URL;
    expect(`${page.origin}${page.pathname}`).toBe(`${target}/cli-auth`);
    expect([...page.searchParams.keys()]).toEqual([
      "port",
      "state",
      "code_challenge",
      "hostname",
      "kind",
      "hours",
      "standing",
      "name"
    ]);
    expect(page.searchParams.get("hostname")).toBe("test-laptop");
    expect(page.searchParams.get("kind")).toBe("agent");
    expect(page.searchParams.get("hours")).toBe("24");
    expect(page.searchParams.get("standing")).toBe("1");
    expect(page.searchParams.get("name")).toBe("triage run");
    // Nothing of the other token's request rides along.
    expect(page.searchParams.has("apps")).toBe(false);

    // The verifier that never left the process answers the challenge in the URL.
    const exchange = requests.find((r) => r.url === "/api/auth/cli/exchange");
    const body = JSON.parse(exchange?.body ?? "{}") as { code: string; code_verifier: string };
    expect(body.code).toBe("one-time-code");
    expect(challengeFor(body.code_verifier)).toBe(page.searchParams.get("code_challenge"));
  });

  it("leaves standing out of the URL altogether when it was not asked for", async () => {
    await mint();
    const page = opened[0] as URL;
    expect(page.searchParams.has("standing")).toBe(false);
    expect(page.search).not.toContain("standing");
    expect(page.searchParams.get("hours")).toBe("8");
    expect(page.searchParams.get("name")).toBe("agent on test-laptop");
  });
});

describe("a successful mint", () => {
  it("prints `export OXY_TOKEN=…` once on stdout, and everything else on stderr", async () => {
    await mint();
    expect(stdout).toBe(`export OXY_TOKEN=${SECRET}\n`);
    expect(stderr).not.toContain(SECRET);
  });

  it("says what it is, when it expires, that it was shown once, and how to end it", async () => {
    await mint();
    expect(stderr).toContain(
      'Minted agent token "agent on test-laptop", acting as luong@oxy.tech.'
    );
    expect(stderr).toContain("it reaches what luong@oxy.tech's organization memberships do");
    expect(stderr).toMatch(/it expires \S+ \(in 8 h\), and cannot be extended or renewed/);
    expect(stderr).toContain("shown once and not stored on this machine");
    expect(stderr).toContain("to end it: `oxyc tokens revoke --current`");
    // The one habit that keeps a later command off the person's cached login.
    expect(stderr).toContain("pass --token-env OXY_TOKEN to each command");
  });

  it("never writes the credentials file", async () => {
    await mint();
    expect(existsSync(credentialsFile)).toBe(false);
  });

  it("leaves an existing login in the file byte for byte, and revokes nothing", async () => {
    const login = {
      token: "oxy_pat_the_humans_login",
      email: "luong@oxy.tech",
      is_app_admin: true
    };
    saveCredential(target, login);
    await mint();

    expect(loadCredential(target)).toEqual(login);
    // The exchange, then the new token asked what it is. No DELETE, no /api/user.
    expect(sent()).toEqual(["POST /api/auth/cli/exchange", "GET /api/auth/token"]);
  });

  it("needs no credential of its own: neither the cached login nor OXY_TOKEN is ever sent", async () => {
    saveCredential(target, { token: "oxy_pat_the_humans_login", email: "", is_app_admin: false });
    vi.stubEnv("OXY_TOKEN", "oxy_pat_someone_elses_token");
    await mint();
    // The exchange carries none; the read-back carries the token just minted.
    expect(requests.map((r) => r.authorization)).toEqual([undefined, `Bearer ${SECRET}`]);
  });

  it("asks the token what it is even when the exchange already described it", async () => {
    await mint();
    expect(sent()).toContain("GET /api/auth/token");
  });

  it("accepts a token the exchange described in less detail, as long as the token itself is whole", async () => {
    exchangeReply = { status: 200, body: { token: { id: "tok-agent-1" }, secret: SECRET } };
    await mint();
    expect(stdout).toBe(`export OXY_TOKEN=${SECRET}\n`);
  });

  it("allows a few minutes of clock difference, and a shorter life than asked", async () => {
    describedAs({ expires_at: new Date(Date.now() + 8 * 3_600_000 + 4 * 60_000).toISOString() });
    await mint({ hours: "8" });
    expect(stdout).toBe(`export OXY_TOKEN=${SECRET}\n`);

    stdout = "";
    describedAs({ expires_at: inHours(2) });
    await mint({ hours: "8" });
    expect(stdout).toBe(`export OXY_TOKEN=${SECRET}\n`);
  });
});

describe("standing", () => {
  it("accepts staff access that was asked for and approved, and says the token carries it", async () => {
    describedAs({ platform: true });
    await mint({ standing: true });
    expect(stdout).toBe(`export OXY_TOKEN=${SECRET}\n`);
    expect(stderr).toContain("it reaches everything luong@oxy.tech does, staff access included");
    expect(revokes()).toHaveLength(0);
  });

  it("names partner access, and both, by what the token carries", async () => {
    describedAs({ partner: true });
    await mint({ standing: true });
    expect(stderr).toContain("partner access included");

    stderr = "";
    describedAs({ platform: true, partner: true });
    await mint({ standing: true });
    expect(stderr).toContain("staff and partner access included");
  });

  it("accepts a token with less than was asked: the approver left standing out, and stderr says so", async () => {
    // --standing was passed; the page's box starts empty and the approver left it so.
    await mint({ standing: true });

    expect(stdout).toBe(`export OXY_TOKEN=${SECRET}\n`);
    expect(revokes()).toHaveLength(0);
    const said = stderr.split("\n").filter((line) => line.includes("--standing"));
    expect(said).toHaveLength(1);
    expect(said[0]).toContain(
      "--standing was asked for and the approver did not include it: the token reaches only what luong@oxy.tech's organization memberships do"
    );
    expect(stderr).not.toContain("access included");
  });

  it("says so under --quiet too: an agent that planned on staff access has to hear it", async () => {
    vi.stubEnv("OXYC_QUIET", "1");
    await mint({ standing: true });
    expect(stdout).toBe(`export OXY_TOKEN=${SECRET}\n`);
    expect(stderr).toContain("the approver did not include it");
  });

  it("refuses staff access nobody asked for", async () => {
    describedAs({ platform: true });
    const cause = await refused(mint());
    expect(cause.message).toContain("minted a token that is not the one asked for");
    expect(cause.detail).toContain(
      "it carries staff standing, and none was asked for (--standing was not passed)"
    );
  });

  it("refuses partner access nobody asked for", async () => {
    describedAs({ partner: true });
    const cause = await refused(mint());
    expect(cause.detail).toContain("it carries partner standing, and none was asked for");
  });

  it("refuses it when only the token itself admits to it, whatever the exchange said", async () => {
    describedOver = { platform: true };
    const cause = await refused(mint());
    expect(cause.detail).toContain("it carries staff standing");
  });

  it("refuses a standing flag it cannot read, with --standing or without", async () => {
    for (const flags of [{}, { standing: true }]) {
      requests = [];
      describedOver = { platform: "yes" };
      const cause = await refused(mint(flags));
      expect(cause.message).toContain("minted a token that could not be confirmed");
      expect(cause.detail).toContain('whether it carries staff standing cannot be read ("yes")');
    }
    requests = [];
    describedOver = { partner: undefined };
    TOKEN_ROW = Object.fromEntries(Object.entries(TOKEN_ROW).filter(([key]) => key !== "partner"));
    exchangeReply = { status: 200, body: { token: TOKEN_ROW, secret: SECRET } };
    const cause = await refused(mint());
    expect(cause.detail).toContain("whether it carries partner standing cannot be read (null)");
  });
});

describe("a result that is not the token asked for", () => {
  it("an ordinary oxyc login token: nothing printed, the token revoked, exit 8", async () => {
    // What a deployment that predates agent tokens hands back: ninety days, as a login.
    describedAs({ source: "oxyc_login", name: "oxyc on test-laptop", expires_at: inHours(2160) });
    const cause = await refused(mint());
    expect(cause.message).toBe(`${target} does not support agent tokens yet`);
    expect(cause.detail).toContain('source as "oxyc_login", not "oxyc_agent"');
    expect(cause.detail).toContain("an ordinary `oxyc login` token");
    expect(cause.hint).toContain("stop and tell your operator");
    expect(cause.hint).toContain("a version with agent tokens");
  });

  it("a token whose own description names another source, though the exchange said oxyc_agent", async () => {
    describedOver = { source: "ui" };
    const cause = await refused(mint());
    expect(cause.detail).toContain('the token described the token\'s source as "ui"');
  });

  it("a token that names no source at all", async () => {
    TOKEN_ROW = Object.fromEntries(Object.entries(TOKEN_ROW).filter(([key]) => key !== "source"));
    exchangeReply = { status: 200, body: { token: TOKEN_ROW, secret: SECRET } };
    const cause = await refused(mint());
    expect(cause.detail).toContain('source as null, not "oxyc_agent"');
  });

  it("a token that is not a personal one, by either description", async () => {
    describedAs({ kind: "service_account" });
    expect((await refused(mint())).detail).toContain(
      'its exchange described the token as kind "service_account", not "personal"'
    );

    requests = [];
    exchangeReply = { status: 200, body: { token: TOKEN_ROW, secret: SECRET } };
    describedOver = { kind: "ci" };
    expect((await refused(mint())).detail).toContain(
      'the token described the token as kind "ci", not "personal"'
    );
  });

  it("a sandbox agent secret, when an agent token was asked for", async () => {
    const sandbox = "oxy_sbx_0123456789abcdefghijABCDEFGHIJ012345";
    exchangeReply = { status: 200, body: { token: TOKEN_ROW, secret: sandbox } };
    const cause = await refused(mint(), sandbox);
    expect(cause.detail).toContain("is not a personal access token");
    // Never asked what it is: the prefix already said it is the wrong thing.
    expect(sent()).not.toContain("GET /api/auth/token");
  });

  describe("a secret that is not a whole token", () => {
    it("a token of the wrong length is refused, and revoked", async () => {
      const short = "oxy_pat_tooShort1234";
      exchangeReply = { status: 200, body: { token: TOKEN_ROW, secret: short } };
      const cause = await refused(mint(), short);
      expect(cause.detail).toContain("is not a whole personal access token");
    });

    it.each([
      "oxy_pat_0123456789abcdefghijABCDEFGHIJ01234; touch /tmp/pwned",
      "oxy_pat_0123456789abcdefghijABCDEFGHIJ01234$(id)",
      "oxy_pat_0123456789abcdefghijABCDEFGHIJ012345\nexport PATH=/tmp"
    ])(
      "never prints, and never sends as a bearer, a secret a shell would act on: %j",
      async (bad) => {
        exchangeReply = { status: 200, body: { token: TOKEN_ROW, secret: bad } };
        const cause = await failure(mint());
        expect(cause.code).toBe(ExitCode.REFUSED);
        expect(stdout).toBe("");
        // Not made of credential characters, so it goes into no header either.
        expect(requests.every((r) => r.authorization === undefined)).toBe(true);
        expect(cause.detail).toContain("What it returned is not a token");
        expect(`${cause.message}${cause.detail}`).not.toContain("touch /tmp");
      }
    );
  });

  describe("a lifetime that is not the one asked for", () => {
    it("an expiry later than asked, naming both", async () => {
      describedAs({ expires_at: inHours(24) });
      const cause = await refused(mint({ hours: "8" }));
      expect(cause.detail).toMatch(/it expires \S+ — 24 h from now — and 8 h was asked for/);
    });

    it("ninety days, when one hour was asked for", async () => {
      describedAs({ expires_at: inHours(2160) });
      const cause = await refused(mint({ hours: "1" }));
      expect(cause.detail).toContain("1 h was asked for");
    });

    it("a later expiry the token itself gives, when the exchange gave the one asked for", async () => {
      describedOver = { expires_at: inHours(72) };
      const cause = await refused(mint({ hours: "8" }));
      expect(cause.detail).toContain("72 h from now");
    });

    it("no expiry", async () => {
      describedAs({ expires_at: null });
      const cause = await refused(mint());
      expect(cause.detail).toContain("it has no expiry, and 8 h was asked for");
    });

    it("an expiry that cannot be read as a time", async () => {
      describedAs({ expires_at: "soon" });
      const cause = await refused(mint());
      expect(cause.message).toContain("could not be confirmed");
      expect(cause.detail).toContain('its expiry "soon" cannot be read as a time');
    });
  });

  describe("a token that will not say what it is", () => {
    it("no introspection route (404)", async () => {
      describedInstead = { status: 404, body: { error: "not found" } };
      const cause = await refused(mint());
      expect(cause.message).toContain("could not be confirmed");
      expect(cause.detail).toContain("GET /api/auth/token answered 404");
    });

    it("a token the deployment already turns away (401)", async () => {
      describedInstead = { status: 401, body: { error: "unauthorized" } };
      const cause = await refused(mint());
      expect(cause.detail).toContain("GET /api/auth/token answered 401");
    });

    it("no token behind the secret (404 no_token)", async () => {
      describedInstead = { status: 404, body: { code: "no_token" } };
      const cause = await refused(mint());
      expect(cause.detail).toContain("there is no token behind the secret it returned");
    });

    it("a description with no id", async () => {
      describedInstead = { status: 200, body: { kind: "personal", source: "oxyc_agent" } };
      const cause = await refused(mint());
      expect(cause.detail).toContain("GET /api/auth/token answered 200");
    });
  });
});

describe("text a deployment supplied is printed without control characters", () => {
  const ESC = String.fromCharCode(27);

  it("the name, the owner and the expiry of a token that is accepted", async () => {
    describedAs({
      name: `agent${ESC}[2J on laptop`,
      owner: { type: "user", id: "u-1", label: `luong${ESC}]0;x@oxy.tech` }
    });
    await mint();
    expect(stdout).toBe(`export OXY_TOKEN=${SECRET}\n`);
    expect(stderr).not.toContain(ESC);
    expect(stderr).toContain("Minted agent token");
  });

  it("what a refusal quotes back", async () => {
    describedAs({ source: `oxyc_login${ESC}[31m` });
    const cause = await refused(mint());
    expect(`${cause.message}\n${cause.detail}`).not.toContain(ESC);
  });
});

describe("a deployment that cannot mint one", () => {
  it("refuses a session token in the callback, printing and storing nothing", async () => {
    handsBack = "token";
    const cause = await failure(mint());
    expect(cause.code).toBe(ExitCode.REFUSED);
    expect(cause.message).toBe(`${target} does not support agent tokens yet`);
    expect(cause.detail).toContain("Nothing was minted, and nothing was kept.");
    expect(stdout).toBe("");
    expect(requests).toHaveLength(0);
    expect(existsSync(credentialsFile)).toBe(false);
    expect(`${cause.message}${cause.detail}${stderr}`).not.toContain("eyJ.session.jwt");
  });

  it("refuses a code it cannot exchange, when the deployment has no exchange route", async () => {
    exchangeReply = { status: 404, body: { error: "not found" } };
    const cause = await failure(mint());
    expect(cause.code).toBe(ExitCode.REFUSED);
    expect(cause.detail).toContain("POST /api/auth/cli/exchange answered 404");
    expect(stdout).toBe("");
    expect(revokes()).toHaveLength(0);
  });

  it("reports a rejected code as AUTH, naming this command and never `oxyc login`", async () => {
    exchangeReply = { status: 400, body: { code: "invalid_code", error: "invalid code" } };
    const cause = await failure(mint());
    expect(cause.code).toBe(ExitCode.AUTH);
    expect(cause.message).toBe("the approval code was rejected, and no token was minted");
    expect(cause.hint).toContain("oxyc tokens create --agent");
    expect(cause.hint).not.toContain("oxyc login");
    expect(cause.detail).toContain("caps a token's lifetime below the hours asked for");
    expect(stdout).toBe("");
  });
});
