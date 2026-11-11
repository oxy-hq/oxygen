/**
 * `oxyc login --no-wait`, then `oxyc login`, against a fake deployment.
 *
 * The first call must come back with the URL without a single poll; the
 * second must collect the same request (same device code, same verifier) and
 * leave a credential behind, without opening a browser.
 */

import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { CliError, ExitCode } from "../util/errors.js";
import { hostKey, loadCredential } from "./credentials.js";
import { login } from "./login.js";
import { loadPending, pendingPath, startPendingLogin } from "./pending.js";
import { challengeFor } from "./pkce.js";

let server: Server;
let target: string;
let startStatus: number;
let pollReplies: Array<{ status: number; body: unknown }>;
let starts: Array<Record<string, unknown>>;
let polls: Array<Record<string, unknown>>;
let scratch: string;
let waits: number[];

async function read(req: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks).toString();
}

beforeAll(async () => {
  server = createServer((req, res) => {
    void (async () => {
      const reply = (status: number, body: unknown) => {
        res.writeHead(status, { "content-type": "application/json" });
        res.end(JSON.stringify(body));
      };
      if (req.url === "/api/auth/cli/device" && req.method === "POST") {
        starts.push(JSON.parse(await read(req)));
        if (startStatus !== 200) return reply(startStatus, {});
        return reply(200, {
          device_code: "dev-code",
          user_code: "BCDF-GHJK",
          expires_in: 600,
          interval: 5
        });
      }
      if (req.url === "/api/auth/cli/device/poll" && req.method === "POST") {
        polls.push(JSON.parse(await read(req)));
        const next = pollReplies.length > 1 ? pollReplies.shift() : pollReplies[0];
        return reply(next?.status ?? 500, next?.body ?? {});
      }
      if (req.url === "/api/user") {
        return reply(200, { email: "dev@acme.test", is_app_admin: false });
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
  scratch = mkdtempSync(join(tmpdir(), "oxyc-pending-"));
  vi.stubEnv("OXY_CREDENTIALS_PATH", join(scratch, "credentials.json"));
  vi.stubEnv("OXYC_QUIET", "1");
  startStatus = 200;
  pollReplies = [];
  starts = [];
  polls = [];
  waits = [];
});

afterEach(() => {
  vi.unstubAllEnvs();
  rmSync(scratch, { recursive: true, force: true });
});

const approved = {
  status: 200,
  body: { status: "approved", token: { id: "tok-7" }, secret: "oxy_pat_from_the_phone" }
};
const pending = { status: 200, body: { status: "pending" } };

const opts = () => ({
  hostname: "remote-box",
  sleep: async (ms: number) => {
    waits.push(ms);
  },
  open: () => {
    throw new Error("no browser may open while a --no-wait request is waiting");
  }
});

describe("startPendingLogin", () => {
  it("returns the approval URL without polling, and records the request owner-only", async () => {
    const url = await startPendingLogin(target, opts(), "oxyc login --env dev");

    expect(url).toBe(`${target}/cli-auth/device?code=BCDF-GHJK`);
    expect(polls).toEqual([]);
    const entry = loadPending(target);
    expect(entry?.device_code).toBe("dev-code");
    // The challenge the start sent is the verifier the file keeps.
    expect(starts[0]?.code_challenge).toBe(challengeFor(entry?.verifier ?? ""));
    if (process.platform !== "win32") {
      expect(statSync(pendingPath()).mode & 0o777).toBe(0o600);
    }
  });

  it("says the deployment predates the device code on a 404, and records nothing", async () => {
    startStatus = 404;
    const cause = await startPendingLogin(target, opts(), "oxyc login").catch((e: unknown) => e);
    expect(cause).toBeInstanceOf(CliError);
    expect((cause as CliError).code).toBe(ExitCode.UNAVAILABLE);
    expect(loadPending(target)).toBeUndefined();
  });
});

describe("startPendingLogin where the request cannot be recorded", () => {
  it("fails before printing a URL nobody could collect", async () => {
    // The credentials "directory" is a file, so nothing can be written under it.
    writeFileSync(join(scratch, "blocker"), "");
    vi.stubEnv("OXY_CREDENTIALS_PATH", join(scratch, "blocker", "credentials.json"));
    let said = "";
    const spy = vi.spyOn(process.stderr, "write").mockImplementation((chunk) => {
      said += String(chunk);
      return true;
    });
    try {
      const cause = await startPendingLogin(target, opts(), "oxyc login").catch((e: unknown) => e);
      expect(cause).toBeInstanceOf(CliError);
      expect((cause as CliError).message).toContain("could not write");
    } finally {
      spy.mockRestore();
    }
    expect(said).not.toContain("/cli-auth/device");
  });
});

describe("login after --no-wait", () => {
  it("collects the same request in one poll, with no wait first, and caches the token", async () => {
    await startPendingLogin(target, opts(), "oxyc login");
    const verifier = loadPending(target)?.verifier;
    pollReplies = [approved];

    const { token } = await login(target, opts());

    expect(token).toBe("oxy_pat_from_the_phone");
    expect(starts).toHaveLength(1);
    expect(polls).toEqual([{ device_code: "dev-code", code_verifier: verifier }]);
    expect(waits).toEqual([]);
    expect(loadCredential(target)?.token_id).toBe("tok-7");
    expect(loadPending(target)).toBeUndefined();
  });

  it("keeps waiting when nobody has approved yet", async () => {
    await startPendingLogin(target, opts(), "oxyc login");
    pollReplies = [pending, approved];
    await login(target, opts());
    expect(polls).toHaveLength(2);
    expect(waits).toEqual([5000]);
  });

  it("forgets a refused request, so the next login starts afresh", async () => {
    await startPendingLogin(target, opts(), "oxyc login");
    pollReplies = [{ status: 400, body: { code: "access_denied" } }];
    const cause = await login(target, opts()).catch((e: unknown) => e);
    expect((cause as CliError).serverCode).toBe("access_denied");
    expect(loadPending(target)).toBeUndefined();
  });

  it("ignores a request that has lapsed, and one --browser skips", async () => {
    writeFileSync(
      pendingPath(),
      JSON.stringify({
        [hostKey(target)]: {
          device_code: "old",
          user_code: "X",
          verifier: "v",
          interval: 5,
          expires_at: new Date(Date.now() - 1000).toISOString()
        }
      })
    );
    expect(loadPending(target)).toBeUndefined();

    await startPendingLogin(target, opts(), "oxyc login");
    let opened = "";
    const forced = login(target, {
      ...opts(),
      forceBrowser: true,
      open: (url) => {
        opened = url;
        throw new Error("stop");
      }
    });
    await expect(forced).rejects.toThrow("stop");
    expect(opened).toContain("/cli-auth?port=");
    expect(polls).toEqual([]);
  });

  it("leaves other deployments' requests alone", async () => {
    await startPendingLogin(target, opts(), "oxyc login");
    const store = JSON.parse(readFileSync(pendingPath(), "utf8"));
    expect(Object.keys(store)).toEqual([hostKey(target)]);
    expect(loadPending("https://elsewhere.example")).toBeUndefined();
  });
});
