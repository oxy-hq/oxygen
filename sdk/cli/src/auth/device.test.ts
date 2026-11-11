/**
 * Approval by device code (`--no-browser`), against a fake deployment.
 *
 * The deployment is a real loopback server answering the two device-code
 * routes; the "person" is whatever the test scripts the poll to answer next.
 * The waits between polls go through an injected `sleep`, so nothing here
 * waits for real and every wait it asked for is on record.
 */

import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { CliError, ExitCode } from "../util/errors.js";
import { askFromQuery, deviceRound, deviceUrl } from "./device.js";
import { challengeFor, createPkce } from "./pkce.js";

let server: Server;
let target: string;
/** What the start answers. */
let startReply: { status: number; body: unknown };
/** What each poll answers, in order. The last one repeats. */
let pollReplies: Array<{ status: number; body: unknown }>;
let starts: Array<Record<string, unknown>>;
let polls: Array<Record<string, unknown>>;
let waits: number[];

const TOKEN_ROW = { id: "tok-9", name: "agent on build-box", expires_at: "2026-12-30T00:00:00Z" };

async function read(req: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks).toString();
}

beforeAll(async () => {
  server = createServer((req, res) => {
    void (async () => {
      const reply = ({ status, body }: { status: number; body: unknown }) => {
        res.writeHead(status, { "content-type": "application/json" });
        res.end(JSON.stringify(body));
      };
      if (req.url === "/api/auth/cli/device" && req.method === "POST") {
        starts.push(JSON.parse(await read(req)));
        return reply(startReply);
      }
      if (req.url === "/api/auth/cli/device/poll" && req.method === "POST") {
        polls.push(JSON.parse(await read(req)));
        const next = pollReplies.length > 1 ? pollReplies.shift() : pollReplies[0];
        return reply(next ?? { status: 500, body: {} });
      }
      reply({ status: 404, body: { error: `unexpected ${req.url}` } });
    })();
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  target = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});

afterAll(() => {
  server.close();
});

beforeEach(() => {
  vi.stubEnv("OXYC_QUIET", "1");
  startReply = {
    status: 200,
    body: { device_code: "dev-code", user_code: "BCDF-GHJK", expires_in: 600, interval: 5 }
  };
  pollReplies = [];
  starts = [];
  polls = [];
  waits = [];
});

afterEach(() => {
  vi.unstubAllEnvs();
});

const pending = { status: 200, body: { status: "pending", interval: 5 } };
const approved = {
  status: 200,
  body: { status: "approved", token: TOKEN_ROW, secret: "oxy_pat_from_the_phone" }
};

const opts = () => ({
  hostname: "build-box",
  sleep: async (ms: number) => {
    waits.push(ms);
  }
});

const refusalOf = async (promise: Promise<unknown>): Promise<CliError> => {
  const cause = await promise.then(
    () => undefined,
    (e: unknown) => e
  );
  expect(cause).toBeInstanceOf(CliError);
  return cause as CliError;
};

describe("askFromQuery", () => {
  it("sends the /cli-auth parameters as the loopback URL would carry them", () => {
    expect(askFromQuery("kind=sandbox_agent&apps=acme/store,acme/ops&hours=8&name=a%20b")).toEqual({
      kind: "sandbox_agent",
      apps: "acme/store,acme/ops",
      hours: "8",
      name: "a b"
    });
    expect(askFromQuery(undefined)).toEqual({});
  });

  it("points the person at the page with the user code", () => {
    expect(deviceUrl("https://oxy.example/", "BCDF-GHJK")).toBe(
      "https://oxy.example/cli-auth/device?code=BCDF-GHJK"
    );
  });
});

describe("deviceRound", () => {
  it("polls until approved, then returns the token the exchange would have", async () => {
    pollReplies = [pending, pending, approved];
    const pkce = createPkce();
    const minted = await deviceRound(target, opts(), pkce, { query: "kind=agent&hours=4" });

    expect(minted).toEqual({
      token: "oxy_pat_from_the_phone",
      tokenId: "tok-9",
      expiresAt: TOKEN_ROW.expires_at,
      described: TOKEN_ROW
    });
    // The challenge goes out with the request; the verifier only with a poll.
    expect(starts).toEqual([
      {
        code_challenge: challengeFor(pkce.verifier),
        hostname: "build-box",
        ask: { kind: "agent", hours: "4" }
      }
    ]);
    expect(polls).toHaveLength(3);
    expect(polls[0]).toEqual({ device_code: "dev-code", code_verifier: pkce.verifier });
    expect(waits).toEqual([5000, 5000, 5000]);
  });

  it("waits longer after a slow_down", async () => {
    pollReplies = [{ status: 200, body: { status: "slow_down", interval: 5 } }, approved];
    await deviceRound(target, opts(), createPkce());
    expect(waits).toEqual([5000, 10_000]);
  });

  it("waits out the per-client limit instead of abandoning the request", async () => {
    pollReplies = [{ status: 429, body: { code: "rate_limited", error: "too many" } }, approved];
    const minted = await deviceRound(target, opts(), createPkce());
    expect(minted?.token).toBe("oxy_pat_from_the_phone");
    // The server sent no Retry-After: the wait is the interval, which stays as it was.
    expect(waits).toEqual([5000, 5000]);
  });

  it("gives up at the request's lifetime when the deployment never says it lapsed", async () => {
    startReply = {
      status: 200,
      body: { device_code: "dev-code", user_code: "BCDF-GHJK", expires_in: 60, interval: 5 }
    };
    pollReplies = [pending];
    const cause = await refusalOf(deviceRound(target, opts(), createPkce()));
    expect(cause.code).toBe(ExitCode.UNAVAILABLE);
    expect(cause.serverCode).toBe("expired_token");
    // 60 s of life plus 30 s of grace, at 5 s a poll.
    expect(waits.reduce((a, b) => a + b, 0)).toBeLessThanOrEqual(90_000);
    expect(polls.length).toBe(18);
  });

  it("prints the URL and the code even under --quiet", async () => {
    pollReplies = [approved];
    let said = "";
    const spy = vi.spyOn(process.stderr, "write").mockImplementation((chunk) => {
      said += String(chunk);
      return true;
    });
    try {
      await deviceRound(target, opts(), createPkce());
    } finally {
      spy.mockRestore();
    }
    expect(process.env.OXYC_QUIET).toBe("1");
    expect(said).toContain(`${target}/cli-auth/device?code=BCDF-GHJK`);
    expect(said).toContain("type this code on that page: BCDF-GHJK");
    expect(said).not.toContain("Waiting for approval");
  });

  it("is undefined on a deployment with no device-code route", async () => {
    startReply = { status: 404, body: { error: "not found" } };
    expect(await deviceRound(target, opts(), createPkce())).toBeUndefined();
    expect(polls).toHaveLength(0);
  });

  it("stops with the reason when the request is refused", async () => {
    pollReplies = [
      pending,
      {
        status: 400,
        body: { code: "access_denied", error: "the request was declined in the browser" }
      }
    ];
    const cause = await refusalOf(deviceRound(target, opts(), createPkce()));
    expect(cause.code).toBe(ExitCode.AUTH);
    expect(cause.serverCode).toBe("access_denied");
    expect(cause.detail).toContain("declined in the browser");
  });

  it("stops when nobody approves in time", async () => {
    pollReplies = [{ status: 400, body: { code: "expired_token", error: "too late" } }];
    const cause = await refusalOf(deviceRound(target, opts(), createPkce()));
    expect(cause.code).toBe(ExitCode.UNAVAILABLE);
    expect(cause.serverCode).toBe("expired_token");
  });

  it("names the caller's own words for a request it cannot collect", async () => {
    pollReplies = [{ status: 400, body: { code: "invalid_code", error: "nope" } }];
    const refused = { message: "the approval was rejected", detail: "d", hint: "h" };
    const cause = await refusalOf(deviceRound(target, opts(), createPkce(), { refused }));
    expect(cause.message).toBe("the approval was rejected");
    expect(cause.code).toBe(ExitCode.AUTH);
  });

  it("refuses a start the deployment rejects, before any poll", async () => {
    startReply = { status: 400, body: { error: "'ask.kind' 'personal' is unknown" } };
    const cause = await refusalOf(deviceRound(target, opts(), createPkce()));
    expect(cause.code).toBe(ExitCode.REQUEST);
    expect(polls).toHaveLength(0);
  });
  describe("--approver", () => {
    /** stderr as written while `run` runs, with quiet mode off. */
    async function stderrOf(run: () => Promise<unknown>): Promise<string> {
      vi.stubEnv("OXYC_QUIET", "");
      const lines: string[] = [];
      const spy = vi.spyOn(process.stderr, "write").mockImplementation((chunk) => {
        lines.push(String(chunk));
        return true;
      });
      try {
        await run();
      } finally {
        spy.mockRestore();
      }
      return lines.join("");
    }

    it("sends the approver in the start body, beside the ask", async () => {
      pollReplies = [approved];
      await deviceRound(target, { ...opts(), approver: "ada@example.com" }, createPkce(), {
        query: "kind=agent&hours=4"
      });
      expect(starts).toHaveLength(1);
      expect(starts[0]).toMatchObject({
        approver: "ada@example.com",
        ask: { kind: "agent", hours: "4" }
      });
      // Only the start names the approver: a poll carries the device code and the verifier.
      expect(Object.keys(polls[0] ?? {}).sort()).toEqual(["code_verifier", "device_code"]);
    });

    it("sends no approver key when none was named (or only whitespace)", async () => {
      pollReplies = [approved];
      await deviceRound(target, opts(), createPkce());
      pollReplies = [approved];
      await deviceRound(target, { ...opts(), approver: "  " }, createPkce());
      expect(starts).toHaveLength(2);
      for (const start of starts) expect(start).not.toHaveProperty("approver");
    });

    it("says who was asked and keeps every existing line, never printing the device code", async () => {
      pollReplies = [approved];
      const said = await stderrOf(() =>
        deviceRound(target, { ...opts(), approver: "ada@example.com" }, createPkce())
      );
      const lines = said.split("\n").filter(Boolean);
      expect(lines).toContain(`  ${target}/cli-auth/device?code=BCDF-GHJK`);
      expect(said).toContain("and type this code on that page: BCDF-GHJK");
      expect(said).toContain("Waiting for approval (the request lapses in 10 min)");
      expect(said).toContain(
        `Approval request sent to ada@example.com — they type the code BCDF-GHJK on ${target}/cli-auth/device?code=BCDF-GHJK.`
      );
      expect(said).not.toContain("dev-code");
    });

    it("says nothing about an approver when none was named", async () => {
      pollReplies = [approved];
      const said = await stderrOf(() => deviceRound(target, opts(), createPkce()));
      expect(said).toContain("and type this code on that page: BCDF-GHJK");
      expect(said).not.toContain("Approval request sent");
    });

    it("strips control characters from the address it prints", async () => {
      pollReplies = [approved];
      const said = await stderrOf(() =>
        deviceRound(target, { ...opts(), approver: "ada@example.com\u001b[2J" }, createPkce())
      );
      expect(said).not.toContain("\u001b[2J");
    });
  });
});
