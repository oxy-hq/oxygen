/**
 * `oxyc login`, against a fake deployment and a fake browser.
 *
 * The fake deployment is a real loopback server, because the login's own
 * loopback is one and the two have to talk over real sockets. The fake browser
 * is the `open` hook: handed the `/cli-auth` URL, it does what the page would —
 * redirect to `127.0.0.1:<port>/callback` with whatever that deployment's page
 * hands back.
 *
 * Three deployments matter, and each is a login somebody will actually make:
 *   new   issues a code, exchanges it for an `oxy_pat_`
 *   old   ignores the PKCE parameters and hands back the session token
 *   mid   issues a code but has no exchange route yet (a deploy in progress)
 */

import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { CliError, ExitCode } from "../util/errors.js";
import { hostKey } from "./credentials.js";
import { cliAuthUrl, login } from "./login.js";
import { challengeFor } from "./pkce.js";

type Deployment = "new" | "old" | "mid";

let server: Server;
let target: string;
let deployment: Deployment;
/** What the exchange answers on a "new" deployment. */
let exchangeReply: { status: number; body: unknown };
let exchangeBodies: Array<{ code: string; code_verifier: string }>;
let userBearers: string[];
let scratch: string;

const TOKEN_ROW = {
  id: "tok-1",
  name: "oxyc on test-laptop",
  kind: "personal",
  expires_at: "2026-12-30T00:00:00Z"
};

async function read(req: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks).toString();
}

beforeAll(async () => {
  // `node:http` ignores the listener's return value; `void` says so. A rejection
  // still surfaces, as vitest's unhandled-rejection error.
  server = createServer((req, res) => {
    void (async () => {
      const reply = (status: number, value: unknown) => {
        res.writeHead(status, { "content-type": "application/json" });
        res.end(JSON.stringify(value));
      };
      if (req.url === "/api/auth/cli/exchange" && req.method === "POST") {
        if (deployment !== "new") return reply(404, { error: "not found" });
        exchangeBodies.push(JSON.parse(await read(req)));
        return reply(exchangeReply.status, exchangeReply.body);
      }
      if (req.url === "/api/user") {
        userBearers.push(req.headers.authorization ?? "");
        return reply(200, { email: "dev@acme.test", is_app_admin: true });
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
  scratch = mkdtempSync(join(tmpdir(), "oxyc-login-"));
  vi.stubEnv("OXY_CREDENTIALS_PATH", join(scratch, "credentials.json"));
  vi.stubEnv("OXYC_QUIET", "1");
  deployment = "new";
  exchangeReply = { status: 200, body: { token: TOKEN_ROW, secret: "oxy_pat_secret" } };
  exchangeBodies = [];
  userBearers = [];
});

afterEach(() => {
  vi.unstubAllEnvs();
  rmSync(scratch, { recursive: true, force: true });
});

/** Every `/cli-auth` URL the login opened, in order. */
let opened: URL[];

/**
 * The page. A deployment that knows PKCE issues a code when it is handed a
 * challenge and a token when it is not (which is how an OLDER oxyc still logs
 * in to it); one that does not know PKCE ignores the challenge entirely.
 */
function browser(url: string): void {
  const page = new URL(url);
  opened.push(page);
  const port = page.searchParams.get("port");
  const state = page.searchParams.get("state") ?? "";
  const issuesCode = deployment !== "old" && page.searchParams.has("code_challenge");
  const handed = issuesCode ? "code=one-time-code" : "token=eyJ.session.jwt";
  void fetch(`http://127.0.0.1:${port}/callback?${handed}&state=${encodeURIComponent(state)}`);
}

const stored = () =>
  JSON.parse(readFileSync(join(scratch, "credentials.json"), "utf8"))[hostKey(target)];

beforeEach(() => {
  opened = [];
});

describe("cliAuthUrl", () => {
  it("adds the challenge and the hostname to the original two parameters", () => {
    const url = new URL(
      cliAuthUrl(
        "https://app.oxygen-hq.com/",
        4242,
        "st ate",
        { verifier: "v", challenge: "ch-allenge_" },
        "my laptop"
      )
    );
    expect(url.pathname).toBe("/cli-auth");
    expect(Object.fromEntries(url.searchParams)).toEqual({
      port: "4242",
      state: "st ate",
      code_challenge: "ch-allenge_",
      hostname: "my laptop"
    });
  });

  it("without PKCE, is the URL an older oxyc opens — byte for byte", () => {
    expect(cliAuthUrl("https://app.oxygen-hq.com", 4242, "abc", undefined, "x")).toBe(
      "https://app.oxygen-hq.com/cli-auth?port=4242&state=abc"
    );
  });
});

describe("login", () => {
  it("exchanges the one-time code, with the verifier, for a revocable token", async () => {
    const { token, user } = await login(target, { open: browser, hostname: "test-laptop" });

    expect(token).toBe("oxy_pat_secret");
    expect(opened).toHaveLength(1);
    expect(opened[0]?.searchParams.get("hostname")).toBe("test-laptop");

    // PKCE, end to end: the challenge the browser carried is the S256 of the
    // verifier that only the exchange ever saw.
    expect(exchangeBodies).toHaveLength(1);
    expect(exchangeBodies[0]?.code).toBe("one-time-code");
    const verifier = exchangeBodies[0]?.code_verifier ?? "";
    expect(opened[0]?.searchParams.get("code_challenge")).toBe(challengeFor(verifier));
    expect(opened[0]?.href).not.toContain(verifier);

    // The new token is what proves the login, and what is cached.
    expect(userBearers).toEqual(["Bearer oxy_pat_secret"]);
    expect(user).toEqual({
      token: "oxy_pat_secret",
      email: "dev@acme.test",
      is_app_admin: true,
      token_id: "tok-1",
      expires_at: "2026-12-30T00:00:00Z"
    });
    expect(stored()).toEqual(user);
  });

  it("omits expires_at for a token that never expires, rather than storing null", async () => {
    exchangeReply = {
      status: 200,
      body: { token: { ...TOKEN_ROW, expires_at: null }, secret: "oxy_pat_secret" }
    };
    await login(target, { open: browser, hostname: "h" });
    expect(Object.keys(stored()).sort()).toEqual(["email", "is_app_admin", "token", "token_id"]);
  });

  /**
   * AN OLDER DEPLOYMENT. Its page does not know `code_challenge` and hands the
   * session token back as it always did. The login must simply work, and the
   * file it writes must be the three fields the Rust binary wrote.
   */
  it("accepts the token an older deployment's page hands back, and stores it as before", async () => {
    deployment = "old";
    const { token } = await login(target, { open: browser, hostname: "h" });

    expect(token).toBe("eyJ.session.jwt");
    expect(opened).toHaveLength(1);
    expect(exchangeBodies).toHaveLength(0);
    expect(userBearers).toEqual(["Bearer eyJ.session.jwt"]);
    expect(stored()).toEqual({
      token: "eyJ.session.jwt",
      email: "dev@acme.test",
      is_app_admin: true
    });
  });

  /**
   * THE 404 FALLBACK. A code was issued and there is nowhere to exchange it —
   * a web app deployed ahead of its server. The login opens the browser once
   * more WITHOUT a challenge, which is the request an older oxyc makes and the
   * one every deployment answers with a token.
   */
  it("falls back to the session-token flow when the exchange route is missing", async () => {
    deployment = "mid";
    vi.stubEnv("OXYC_QUIET", "");
    const stderr = vi.spyOn(process.stderr, "write").mockImplementation(() => true);

    const { token } = await login(target, { open: browser, hostname: "h" });

    const written = stderr.mock.calls.map((c) => String(c[0])).join("");
    stderr.mockRestore();
    expect(token).toBe("eyJ.session.jwt");
    expect(opened).toHaveLength(2);
    expect(opened[0]?.searchParams.has("code_challenge")).toBe(true);
    expect(opened[1]?.searchParams.has("code_challenge")).toBe(false);
    expect(opened[1]?.searchParams.has("hostname")).toBe(false);
    // A fresh state for the second round: the first one's is spent.
    expect(opened[1]?.searchParams.get("state")).not.toBe(opened[0]?.searchParams.get("state"));
    expect(written).toContain("/api/auth/cli/exchange answered 404");
    expect(written).toContain("falling back to the session-token login");
    expect(stored()).toEqual({
      token: "eyJ.session.jwt",
      email: "dev@acme.test",
      is_app_admin: true
    });
  });

  it("reports a rejected code as AUTH and caches nothing", async () => {
    exchangeReply = { status: 400, body: { error: "bad code", code: "invalid_code" } };
    const error = await login(target, { open: browser, hostname: "h" }).catch((e: unknown) => e);

    expect(error).toBeInstanceOf(CliError);
    expect((error as CliError).code).toBe(ExitCode.AUTH);
    expect((error as CliError).message).toContain("login code was rejected");
    expect((error as CliError).hint).toContain("oxyc login");
    // One round only: a rejected code is not a missing route.
    expect(opened).toHaveLength(1);
    expect(() => stored()).toThrow();
  });

  it("ignores a callback carrying the wrong state, and keeps waiting for the right one", async () => {
    const impatient = (url: string) => {
      const page = new URL(url);
      const port = page.searchParams.get("port");
      void fetch(`http://127.0.0.1:${port}/callback?code=stolen&state=not-the-state`).then(
        (response) => {
          expect(response.status).toBe(400);
          browser(url);
        }
      );
    };
    await login(target, { open: impatient, hostname: "h" });
    expect(exchangeBodies.map((b) => b.code)).toEqual(["one-time-code"]);
  });
});
