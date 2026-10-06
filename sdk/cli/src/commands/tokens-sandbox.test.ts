/**
 * `oxyc tokens create --sandbox-agent`, against a fake deployment and a fake
 * browser — the harness `login.test.ts` uses, because this is the same
 * loopback with four more parameters.
 *
 * What is pinned: every usage error lands before a browser opens; the
 * `/cli-auth` URL; stdout is the `export` line and nothing else; the
 * credentials file is never written; and no other token is revoked.
 */

import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { forgetSandboxTokens } from "../apps/sandbox-token.js";
import { loadCredential, saveCredential } from "../auth/credentials.js";
import { challengeFor } from "../auth/pkce.js";
import { createContext } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import {
  parseSandboxMint,
  runTokensCreateSandboxAgent,
  sandboxMintQuery
} from "./tokens-sandbox.js";

const SECRET = "oxy_sbx_0123456789abcdefghijABCDEFGHIJ012345";

/** An RFC 3339 instant `hours` from now — what a token minted for that long carries. */
const inHours = (hours: number) => new Date(Date.now() + hours * 3_600_000).toISOString();

/** The token row the deployment describes: a sandbox agent token, 8 hours long. */
let TOKEN_ROW: Record<string, unknown>;

/** The apps a minted token reaches, as `GET /api/auth/token` lists them. */
const app = (org_slug: string, slug: string) => ({ id: `${org_slug}-${slug}`, org_slug, slug });

let server: Server;
let target: string;
/** What `POST /api/auth/cli/exchange` answers. */
let exchangeReply: { status: number; body: unknown };
/** The `apps` of `GET /api/auth/token` for the minted token; `undefined` is a 404. */
let describedApps: unknown[] | undefined;
/** Fields laid over that description, to make it say something else. */
let describedOver: Record<string, unknown>;
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
      // A string is sent as it is: a body that is not JSON, byte for byte.
      const reply = (status: number, value?: unknown) => {
        res.writeHead(status, { "content-type": "application/json" });
        if (typeof value === "string") return res.end(value);
        res.end(value === undefined ? undefined : JSON.stringify(value));
      };
      if (req.url === "/api/auth/cli/exchange" && req.method === "POST") {
        return reply(exchangeReply.status, exchangeReply.body);
      }
      if (req.url === "/api/auth/token" && req.method === "DELETE") return reply(204);
      if (req.url === "/api/auth/token" && req.method === "GET" && describedApps) {
        return reply(200, { ...TOKEN_ROW, apps: describedApps, ...describedOver });
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
  scratch = mkdtempSync(join(tmpdir(), "oxyc-mint-"));
  credentialsFile = join(scratch, "credentials.json");
  vi.stubEnv("OXY_CREDENTIALS_PATH", credentialsFile);
  vi.stubEnv("OXY_TOKEN", "");
  TOKEN_ROW = {
    id: "tok-sbx-1",
    name: "fix the checkout",
    kind: "sandbox_agent",
    all_access: false,
    expires_at: inHours(8)
  };
  exchangeReply = { status: 200, body: { token: TOKEN_ROW, secret: SECRET } };
  describedApps = [app("acme", "store")];
  describedOver = {};
  forgetSandboxTokens();
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

const mint = (flags: { apps: string[]; hours?: string; name?: string }) =>
  runTokensCreateSandboxAgent(context(), flags, { open: browser, hostname: "test-laptop" });

async function refusal(run: Promise<unknown>): Promise<CliError> {
  const cause = await run.then(
    () => undefined,
    (thrown: unknown) => thrown
  );
  expect(cause).toBeInstanceOf(CliError);
  return cause as CliError;
}

describe("argument validation, before any browser opens", () => {
  const parse = (flags: { apps: string[]; hours?: string; name?: string }) =>
    parseSandboxMint(flags, "test-laptop");

  it("defaults to 8 hours and a name that says where it was minted", () => {
    expect(parse({ apps: ["acme/store"] })).toEqual({
      apps: ["acme/store"],
      hours: 8,
      name: "sandbox agent on test-laptop"
    });
  });

  it("takes one to five apps", () => {
    const five = ["a/1", "a/2", "a/3", "a/4", "a/5"];
    expect(parse({ apps: five }).apps).toEqual(five);
    expect(() => parse({ apps: [] })).toThrow(/at least one --app/);
    expect(() => parse({ apps: [...five, "a/6"] })).toThrow(/at most 5 apps/);
  });

  it.each([
    "store",
    "acme/",
    "/store",
    "acme/store/extra",
    "acme/sto re",
    "acme/store,acme/pos",
    "a1a1a1a1-2222-3333-4444-555555555555"
  ])("refuses %j: each app is <org>/<app>", (app) => {
    expect(() => parse({ apps: [app] })).toThrow(/is not <org>\/<app>/);
  });

  /**
   * `apps=` is written into the /cli-auth URL unescaped, so a slug is the only
   * thing keeping a second parameter, or a second app, out of it. The pattern
   * is anchored at both ends: a match of the FRONT of the string would let
   * `acme/store&hours=168` ask the page for a week.
   */
  it.each([
    "acme/store&hours=168",
    "acme/store,other/app",
    "acme/store&kind=personal",
    "acme/store#",
    "acme/store?x=1",
    "acme/store%2Cother%2Fapp",
    "acme/store\n",
    "acme/store\nother/app",
    " acme/store",
    "acme/store=",
    "acme&x/store"
  ])("refuses %j before a browser opens: nothing may reach the URL but a slug", async (app) => {
    expect(() => parse({ apps: [app] })).toThrow(/is not <org>\/<app>/);
    const cause = await refusal(mint({ apps: [app] }));
    expect(cause.code).toBe(ExitCode.USAGE);
    expect(opened).toHaveLength(0);
    expect(requests).toHaveLength(0);
  });

  it("writes only slugs, commas and slashes into apps=, whatever was accepted", () => {
    const query = sandboxMintQuery(parse({ apps: ["acme/store", "a-b.c_d/e1"] }));
    expect(query.match(/&/g)).toHaveLength(3);
    expect(new URLSearchParams(query).get("apps")).toBe("acme/store,a-b.c_d/e1");
    expect(new URLSearchParams(query).get("hours")).toBe("8");
  });

  it("refuses an app named twice, whatever the case", () => {
    expect(() => parse({ apps: ["acme/store", "Acme/Store"] })).toThrow(/named twice/);
  });

  it("takes 1 to 168 whole hours", () => {
    expect(parse({ apps: ["a/b"], hours: "1" }).hours).toBe(1);
    expect(parse({ apps: ["a/b"], hours: "168" }).hours).toBe(168);
    for (const hours of ["0", "169", "-1", "1.5", "eight", ""]) {
      expect(() => parse({ apps: ["a/b"], hours })).toThrow(/whole number from 1 to 168/);
    }
  });

  it("refuses an empty or over-long name", () => {
    expect(() => parse({ apps: ["a/b"], name: "  " })).toThrow(/--name is empty/);
    expect(() => parse({ apps: ["a/b"], name: "x".repeat(101) })).toThrow(/limit is 100/);
  });

  it("is a usage error (exit 2), and nothing was opened or requested", async () => {
    for (const flags of [
      { apps: [] },
      { apps: ["store"] },
      { apps: ["acme/store"], hours: "200" }
    ]) {
      expect((await refusal(mint(flags))).code).toBe(ExitCode.USAGE);
    }
    expect(opened).toHaveLength(0);
    expect(requests).toHaveLength(0);
  });
});

describe("the /cli-auth URL", () => {
  it("adds kind, apps, hours and name to the four login parameters", () => {
    expect(
      sandboxMintQuery({ apps: ["acme/store", "acme/pos"], hours: 8, name: "fix the checkout" })
    ).toBe("kind=sandbox_agent&apps=acme/store,acme/pos&hours=8&name=fix%20the%20checkout");
  });

  it("opens it on the deployment, with a PKCE challenge and this machine's hostname", async () => {
    describedApps = [app("acme", "store"), app("acme", "pos")];
    await mint({ apps: ["acme/store", "acme/pos"], hours: "24", name: "fix the checkout" });

    expect(opened).toHaveLength(1);
    const page = opened[0] as URL;
    expect(`${page.origin}${page.pathname}`).toBe(`${target}/cli-auth`);
    expect([...page.searchParams.keys()]).toEqual([
      "port",
      "state",
      "code_challenge",
      "hostname",
      "kind",
      "apps",
      "hours",
      "name"
    ]);
    expect(page.searchParams.get("hostname")).toBe("test-laptop");
    expect(page.searchParams.get("kind")).toBe("sandbox_agent");
    expect(page.searchParams.get("apps")).toBe("acme/store,acme/pos");
    expect(page.searchParams.get("hours")).toBe("24");
    expect(page.searchParams.get("name")).toBe("fix the checkout");
    // The separators stay literal in the URL itself, as the contract writes them.
    expect(page.search).toContain("&apps=acme/store,acme/pos&");

    // The verifier that never left the process answers the challenge in the URL.
    const exchange = requests.find((r) => r.url === "/api/auth/cli/exchange");
    const sent = JSON.parse(exchange?.body ?? "{}") as { code: string; code_verifier: string };
    expect(sent.code).toBe("one-time-code");
    expect(challengeFor(sent.code_verifier)).toBe(page.searchParams.get("code_challenge"));
  });
});

describe("a successful mint", () => {
  it("prints `export OXY_TOKEN=…` once on stdout, and everything else on stderr", async () => {
    await mint({ apps: ["acme/store"] });
    expect(stdout).toBe(`export OXY_TOKEN=${SECRET}\n`);
    expect(stderr).toContain("fix the checkout");
    expect(stderr).not.toContain(SECRET);
  });

  it("never writes the credentials file", async () => {
    await mint({ apps: ["acme/store"] });
    expect(existsSync(credentialsFile)).toBe(false);
  });

  it("leaves an existing login in the file byte for byte, and revokes nothing", async () => {
    const login = {
      token: "oxy_pat_the_humans_login",
      email: "luong@oxy.tech",
      is_app_admin: true
    };
    saveCredential(target, login);
    await mint({ apps: ["acme/store"] });

    expect(loadCredential(target)).toEqual(login);
    // The exchange, then the new token asked what it reaches. No DELETE, no /api/user.
    expect(requests.map((r) => `${r.method} ${r.url}`)).toEqual([
      "POST /api/auth/cli/exchange",
      "GET /api/auth/token"
    ]);
  });

  it("needs no credential of its own: the login in the cache is never sent", async () => {
    saveCredential(target, { token: "oxy_pat_the_humans_login", email: "", is_app_admin: false });
    await mint({ apps: ["acme/store"] });
    // The exchange carries none; the read-back carries the token just minted.
    expect(requests.map((r) => r.authorization)).toEqual([undefined, `Bearer ${SECRET}`]);
  });

  it("reads the apps off the exchange itself when it lists them, with no second request", async () => {
    exchangeReply = {
      status: 200,
      body: { token: { ...TOKEN_ROW, apps: [app("acme", "store")] }, secret: SECRET }
    };
    describedApps = undefined;
    await mint({ apps: ["acme/store"] });
    expect(stdout).toBe(`export OXY_TOKEN=${SECRET}\n`);
    expect(requests.map((r) => `${r.method} ${r.url}`)).toEqual(["POST /api/auth/cli/exchange"]);
  });

  it("matches the apps whatever their case or order", async () => {
    describedApps = [app("acme", "pos"), app("Acme", "Store")];
    await mint({ apps: ["acme/store", "ACME/pos"] });
    expect(stdout).toBe(`export OXY_TOKEN=${SECRET}\n`);
  });

  /**
   * THE EXCHANGE AS THE SERVER ANSWERS IT (`user_tokens::handlers::TokenWithSecret`):
   * `token` is the shared token shape, whose reach is in `grants`. It has no
   * `apps` and no `minter` — those are added by `GET /api/auth/token` alone. So
   * which apps the token reaches is always read back, with the new token.
   */
  it("reads the apps back from GET /api/auth/token when the exchange's token lists none", async () => {
    const exchanged = {
      id: "0b8f6c1e-3a52-4e0f-9d7a-5c2e4b1a9f30",
      name: "fix the checkout",
      kind: "sandbox_agent",
      display_prefix: "oxy_sbx_0123",
      last_four: "2345",
      all_access: false,
      platform: true,
      partner: false,
      grants: [
        {
          id: "7d1c2b3a-0000-4000-8000-000000000001",
          kind: "app_sandbox",
          org_id: "11111111-0000-4000-8000-000000000001",
          org_name: "Acme",
          workspace_id: null,
          workspace_name: null,
          role_ceiling: null,
          app_id: "22222222-0000-4000-8000-000000000001",
          app_name: "Store",
          org_slug: "acme",
          app_slug: "store",
          revoked_at: null
        }
      ],
      expires_at: inHours(8),
      last_used_at: null,
      created_at: new Date().toISOString(),
      revoked_at: null,
      status: "active",
      source: "oxyc",
      owner: { type: "user", id: "33333333-0000-4000-8000-000000000001", label: "luong@oxy.tech" },
      blocked_orgs: []
    };
    expect(exchanged).not.toHaveProperty("apps");
    exchangeReply = { status: 200, body: { token: exchanged, secret: SECRET } };
    TOKEN_ROW = exchanged;
    // What A1 adds for this kind: the minter, and the apps by slug.
    describedOver = { minter: { user_id: exchanged.owner.id, email: "luong@oxy.tech" } };
    describedApps = [
      { id: exchanged.grants[0]?.app_id, org_slug: "acme", slug: "store", name: "Store" }
    ];

    await mint({ apps: ["acme/store"] });

    expect(stdout).toBe(`export OXY_TOKEN=${SECRET}\n`);
    expect(requests.map((r) => `${r.method} ${r.url}`)).toEqual([
      "POST /api/auth/cli/exchange",
      "GET /api/auth/token"
    ]);
    // The read-back is made with the token just minted, and with nothing else.
    expect(requests[1]?.authorization).toBe(`Bearer ${SECRET}`);
  });

  it("refuses a token whose read-back names another app, though its grants named the one asked for", async () => {
    // The grants on the exchange are not what is compared: the read-back is.
    exchangeReply = {
      status: 200,
      body: {
        token: {
          ...TOKEN_ROW,
          grants: [{ kind: "app_sandbox", org_slug: "acme", app_slug: "store" }]
        },
        secret: SECRET
      }
    };
    describedApps = [app("acme", "payroll")];
    const cause = await refusal(mint({ apps: ["acme/store"] }));
    expect(cause.code).toBe(ExitCode.REFUSED);
    expect(cause.detail).toContain("acme/payroll");
    expect(stdout).toBe("");
    expect(requests.at(-1)).toMatchObject({ method: "DELETE", url: "/api/auth/token" });
  });
});

/**
 * WHAT THE EXCHANGE RETURNED IS CHECKED BEFORE ANYTHING IS PRINTED. A web app
 * or server that predates this kind ignores the four parameters: its page
 * shows the ordinary login screen, and its exchange returns a ninety-day login
 * token with the approver's whole reach. An agent that asked for a token
 * scoped to one app must never be handed that — or any token that is not
 * exactly the one it asked for.
 */
describe("a result that is not the token asked for", () => {
  /** Refused with exit 8, nothing on stdout, nothing stored, and `secret` revoked. */
  async function refusedAndRevoked(secret: string): Promise<CliError> {
    const cause = await refusal(mint({ apps: ["acme/store"] }));
    expect(cause.code).toBe(ExitCode.REFUSED);
    expect(cause.code).not.toBe(0);
    expect(stdout).toBe("");
    expect(stderr).not.toContain(secret);
    expect(existsSync(credentialsFile)).toBe(false);
    const revokes = requests.filter((r) => r.method === "DELETE" && r.url === "/api/auth/token");
    expect(revokes.map((r) => r.authorization)).toEqual([`Bearer ${secret}`]);
    expect(cause.detail).toContain("nothing was kept");
    return cause;
  }

  it("an oxy_pat_ login token: nothing printed, the token revoked, a non-zero exit", async () => {
    exchangeReply = {
      status: 200,
      body: {
        token: { id: "tok-pat", name: "oxyc on test-laptop", kind: "personal" },
        secret: "oxy_pat_the_engineers_full_reach"
      }
    };
    const cause = await refusedAndRevoked("oxy_pat_the_engineers_full_reach");
    expect(cause.message).toContain("does not support sandbox agent tokens yet");
  });

  it("an oxy_sbx_ secret the exchange does not call a sandbox_agent", async () => {
    exchangeReply = {
      status: 200,
      body: { token: { ...TOKEN_ROW, kind: "personal" }, secret: SECRET }
    };
    await refusedAndRevoked(SECRET);
    exchangeReply = { status: 200, body: { token: { id: "tok" }, secret: SECRET } };
    requests = [];
    await refusedAndRevoked(SECRET);
  });

  describe("a secret that is not a whole token", () => {
    /** Refused with exit 8, and neither printed nor quoted back. */
    async function refused(secret: string): Promise<void> {
      exchangeReply = { status: 200, body: { token: TOKEN_ROW, secret } };
      const cause = await refusal(mint({ apps: ["acme/store"] }));
      expect(cause.code).toBe(ExitCode.REFUSED);
      expect(stdout).toBe("");
      expect(`${stderr}${cause.message}${cause.detail}`).not.toContain("pwned");
      expect(existsSync(credentialsFile)).toBe(false);
      expect(cause.detail).toContain("nothing was kept");
    }

    it.each([
      ["a second command after the token", `${SECRET}; touch /tmp/pwned`],
      ["a command substitution", "oxy_sbx_$(touch /tmp/pwned)0123456789abcdef"],
      ["a second line", `${SECRET}\nexport PATH=/tmp/pwned`],
      ["a quote", `${SECRET}'pwned`]
    ])("%s is never printed, and never sent in a header", async (_what, secret) => {
      await refused(secret);
      expect(requests.filter((r) => r.method === "DELETE")).toEqual([]);
      expect(requests.filter((r) => r.url === "/api/auth/token")).toEqual([]);
    });

    it("a token of the wrong length is refused, and revoked", async () => {
      await refused("oxy_sbx_tooShort123");
      const revokes = requests.filter((r) => r.method === "DELETE" && r.url === "/api/auth/token");
      expect(revokes.map((r) => r.authorization)).toEqual(["Bearer oxy_sbx_tooShort123"]);
    });
  });

  it("a sandbox agent token for other apps than the ones named", async () => {
    describedApps = [app("acme", "payroll")];
    const cause = await refusedAndRevoked(SECRET);
    expect(cause.message).toContain("is not the one asked for");
    expect(cause.detail).toContain("acme/payroll");
  });

  it("a sandbox agent token for MORE apps than the ones named", async () => {
    describedApps = [app("acme", "store"), app("acme", "payroll")];
    await refusedAndRevoked(SECRET);
  });

  it("a token whose apps cannot be read back at all", async () => {
    describedApps = undefined;
    const cause = await refusedAndRevoked(SECRET);
    expect(cause.message).toContain("could not be confirmed");
  });

  it("the same check on the exchange's own list, when it carries one", async () => {
    exchangeReply = {
      status: 200,
      body: { token: { ...TOKEN_ROW, apps: [app("acme", "payroll")] }, secret: SECRET }
    };
    await refusedAndRevoked(SECRET);
  });

  /**
   * WHAT CANNOT BE READ IS REFUSED, NOT SKIPPED. Dropping an unreadable entry
   * and comparing the rest would accept a token for the app asked for plus one
   * more the check could not parse.
   */
  describe("an app entry that cannot be read", () => {
    const malformed: Array<[string, unknown]> = [
      ["no slugs at all", { id: "a1a1a1a1-2222-3333-4444-555555555555" }],
      ["an org and no app", { org_slug: "acme" }],
      ["an empty slug", { org_slug: "acme", slug: "" }],
      ["a blank org", { org_slug: "  ", slug: "payroll" }],
      ["slugs that are not strings", { org_slug: 7, slug: ["payroll"] }],
      ["a bare string", "acme/payroll"],
      ["null", null]
    ];

    it.each(malformed)("beside the app asked for, on the exchange: %s", async (_name, extra) => {
      exchangeReply = {
        status: 200,
        body: { token: { ...TOKEN_ROW, apps: [app("acme", "store"), extra] }, secret: SECRET }
      };
      const cause = await refusedAndRevoked(SECRET);
      expect(cause.message).toContain("could not be confirmed");
    });

    it.each(malformed)("beside the app asked for, read back: %s", async (_name, extra) => {
      describedApps = [app("acme", "store"), extra];
      const cause = await refusedAndRevoked(SECRET);
      expect(cause.message).toContain("could not be confirmed");
    });

    it("an `apps` that is not a list at all", async () => {
      describedOver = { apps: "acme/store" };
      await refusedAndRevoked(SECRET);
    });

    it("the app asked for, listed twice — a second grant, not a typo", async () => {
      describedApps = [app("acme", "store"), app("acme", "store")];
      await refusedAndRevoked(SECRET);
    });
  });

  /** The agent asked for a bounded life. A longer one, or none, is not that. */
  describe("a lifetime that is not the one asked for", () => {
    const withExpiry = (expires_at: unknown) => {
      exchangeReply = {
        status: 200,
        body: { token: { ...TOKEN_ROW, expires_at }, secret: SECRET }
      };
    };

    it("an expiry later than asked, naming both", async () => {
      withExpiry(inHours(9));
      const cause = await refusedAndRevoked(SECRET);
      expect(cause.message).toContain("is not the one asked for");
      expect(cause.detail).toContain("8 h was asked for");
      expect(cause.detail).toContain("9 h from now");
    });

    it("ninety days, when one hour was asked for", async () => {
      withExpiry(inHours(24 * 90));
      const cause = await refusal(mint({ apps: ["acme/store"], hours: "1" }));
      expect(cause.code).toBe(ExitCode.REFUSED);
      expect(cause.detail).toContain("1 h was asked for");
      expect(stdout).toBe("");
      expect(requests.filter((r) => r.method === "DELETE")).toHaveLength(1);
    });

    it("no expiry", async () => {
      withExpiry(null);
      const cause = await refusedAndRevoked(SECRET);
      expect(cause.detail).toContain("it has no expiry, and 8 h was asked for");
    });

    it("no expiry on the exchange or on the read-back", async () => {
      const { expires_at: _dropped, ...row } = TOKEN_ROW;
      TOKEN_ROW = row;
      exchangeReply = { status: 200, body: { token: row, secret: SECRET } };
      await refusedAndRevoked(SECRET);
    });

    it.each(["soon", "never", "", 1_900_000_000, true])(
      "an expiry that is not a time: %j",
      async (expires_at) => {
        withExpiry(expires_at);
        await refusedAndRevoked(SECRET);
      }
    );

    it("a longer expiry read back, when the exchange carried none", async () => {
      const { expires_at: _dropped, ...row } = TOKEN_ROW;
      exchangeReply = { status: 200, body: { token: row, secret: SECRET } };
      describedOver = { expires_at: inHours(200) };
      await refusedAndRevoked(SECRET);
    });

    it("accepts the asked lifetime, read back when the exchange carried none", async () => {
      const { expires_at: _dropped, ...row } = TOKEN_ROW;
      exchangeReply = { status: 200, body: { token: row, secret: SECRET } };
      await mint({ apps: ["acme/store"] });
      expect(stdout).toBe(`export OXY_TOKEN=${SECRET}\n`);
    });

    it("allows a few minutes of clock difference, and a shorter life than asked", async () => {
      withExpiry(new Date(Date.now() + 8 * 3_600_000 + 2 * 60_000).toISOString());
      await mint({ apps: ["acme/store"] });
      expect(stdout).toBe(`export OXY_TOKEN=${SECRET}\n`);

      stdout = "";
      withExpiry(inHours(1));
      await mint({ apps: ["acme/store"] });
      expect(stdout).toBe(`export OXY_TOKEN=${SECRET}\n`);
    });
  });

  it("a token described as all-access", async () => {
    exchangeReply = {
      status: 200,
      body: { token: { ...TOKEN_ROW, all_access: true }, secret: SECRET }
    };
    const cause = await refusedAndRevoked(SECRET);
    expect(cause.detail).toContain("all-access");
  });

  it("a token the read-back describes as all-access", async () => {
    describedOver = { all_access: true };
    await refusedAndRevoked(SECRET);
  });
});

describe("text a deployment supplied is printed without control characters", () => {
  const ESC = "\u001b";
  const CLEAR = `${ESC}[2J`;

  it("the name and the expiry of a token that is accepted", async () => {
    // A parenthesised comment is legal in a date string, and holds anything.
    const expiry = `${new Date(Date.now() + 3_600_000).toUTCString()} (${CLEAR})`;
    expect(Number.isNaN(Date.parse(expiry))).toBe(false);
    TOKEN_ROW = { ...TOKEN_ROW, name: `task${CLEAR}`, expires_at: expiry };
    exchangeReply = { status: 200, body: { token: TOKEN_ROW, secret: SECRET } };
    await mint({ apps: ["acme/store"], hours: "8" });
    expect(stdout).toBe(`export OXY_TOKEN=${SECRET}\n`);
    expect(stderr).not.toContain(CLEAR);
    expect(stderr).toContain("task[2J");
  });

  it("the app slugs a refusal quotes back", async () => {
    describedApps = [app("acme", "store"), app(`evil${CLEAR}`, "app")];
    const cause = await refusal(mint({ apps: ["acme/store"] }));
    expect(cause.code).toBe(ExitCode.REFUSED);
    expect(`${cause.message}${cause.detail}`).not.toContain(ESC);
    expect(stdout).toBe("");
  });
});

describe("a deployment that cannot mint one", () => {
  it("refuses a session token in the callback, printing and storing nothing", async () => {
    handsBack = "token";
    const cause = await refusal(mint({ apps: ["acme/store"] }));
    expect(cause.code).toBe(ExitCode.REFUSED);
    expect(stdout).toBe("");
    // Nothing was minted, so there is nothing to revoke — and no request at all.
    expect(requests).toHaveLength(0);
    expect(existsSync(credentialsFile)).toBe(false);
  });

  it("refuses a code it cannot exchange, when the deployment has no exchange route", async () => {
    exchangeReply = { status: 404, body: { error: "not found" } };
    const cause = await refusal(mint({ apps: ["acme/store"] }));
    expect(cause.code).toBe(ExitCode.REFUSED);
    expect(stdout).toBe("");
    expect(requests.filter((r) => r.method === "DELETE")).toHaveLength(0);
  });

  it("reports a rejected code as AUTH, naming this command — not `oxyc login`", async () => {
    exchangeReply = { status: 400, body: { code: "invalid_code" } };
    const cause = await refusal(mint({ apps: ["acme/store"] }));
    expect(cause.code).toBe(ExitCode.AUTH);
    expect(cause.hint).toContain("oxyc tokens create --sandbox-agent");
    expect(stdout).toBe("");
  });

  /**
   * The server answers `400 invalid_code` for a spent code AND for an approved
   * mint it can no longer honour: who may mint, and an organization's lifetime
   * cap, are checked again at the exchange. Running the command again fixes
   * only the first, so the error names the others.
   */
  it("says what else a rejected code can mean for a mint: lost access, or a lifetime cap", async () => {
    exchangeReply = {
      status: 400,
      body: { error: "the login code is invalid or has expired", code: "invalid_code" }
    };
    const cause = await refusal(mint({ apps: ["acme/store"], hours: "168" }));
    expect(cause.code).toBe(ExitCode.AUTH);
    expect(cause.serverCode).toBe("invalid_code");
    expect(cause.message).toBe("the approval code was rejected, and no token was minted");
    expect(cause.message).not.toContain("login");
    expect(cause.detail).toContain("lost access to one of the apps");
    expect(cause.detail).toContain("caps a token's lifetime below the hours asked for");
    expect(cause.hint).toContain("fewer --hours");
    // Nothing was minted, so there is nothing to revoke.
    expect(requests.filter((r) => r.method === "DELETE")).toHaveLength(0);
  });

  it("prints an exchange that failed some other way without its control characters", async () => {
    const ESC = "\u001b";
    // Not JSON, and two lines: what a proxy in front of the deployment might send.
    exchangeReply = { status: 500, body: `upstream failed${ESC}[2J\nsecond${ESC}[1A line` };
    const cause = await refusal(mint({ apps: ["acme/store"] }));
    expect(cause.code).toBe(ExitCode.UNAVAILABLE);
    expect(`${cause.message}${cause.detail}`).not.toContain(ESC);
    expect(cause.detail).toBe("upstream failed[2J\nsecond[1A line");
    expect(stdout).toBe("");
  });
});
