/**
 * What the BUILT BINARY prints when a deployment refuses a token in the
 * variable — the last thing an agent reads before it decides what to do next.
 *
 * `auth/agent-token.test.ts` pins the swap itself. This pins that `main.ts`
 * applies it: the generic "try `oxyc login` again" is written far from where a
 * failure is rendered, and a hint that is right in a unit and never shown is
 * the failure this file exists to catch. It runs the binary against a
 * deployment that answers 401, with `spawn` rather than `spawnSync`, since the
 * deployment lives in this process and has to keep answering.
 */

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { ExitCode } from "../util/errors.js";

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");
const AGENT_TOKEN = "oxy_pat_0123456789abcdefghijABCDEFGHIJ012345";

let server: Server;
let target: string;
/** What the deployment answers every request with. */
let status = 401;
let bearers: (string | undefined)[] = [];

beforeAll(async () => {
  server = createServer((req, res) => {
    bearers.push(req.headers.authorization);
    res.writeHead(status, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: "unauthorized" }));
  });
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  target = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});

afterAll(() => {
  server.close();
});

interface Run {
  status: number;
  stdout: string;
  stderr: string;
}

function oxyc(args: string[], env: Record<string, string>): Promise<Run> {
  if (!existsSync(BIN)) {
    throw new Error(
      `${BIN} is missing — run \`pnpm build\` (or \`pnpm test\`, which builds first)`
    );
  }
  bearers = [];
  return new Promise((done, fail) => {
    const child = spawn(process.execPath, [BIN, ...args, "--target", target], {
      env: {
        ...process.env,
        // Nothing here may read a developer's real credentials.
        OXY_CREDENTIALS_PATH: join(BIN, "..", "__no_such_credentials__.json"),
        OXYC_CACHE_DIR: join(BIN, "..", "__no_such_cache__"),
        OXY_TOKEN: "",
        NO_COLOR: "1",
        ...env
      }
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += String(chunk);
    });
    child.stderr.on("data", (chunk) => {
      stderr += String(chunk);
    });
    child.on("error", fail);
    child.on("close", (code) => done({ status: code ?? -1, stdout, stderr }));
  });
}

describe("a request the deployment refuses, under a token in the variable", () => {
  it.each([401, 403])(
    "exits 4 and tells an agent to stop and report, never to log in (%i)",
    async (answer) => {
      status = answer;
      const r = await oxyc(["api", "/api/orgs"], { OXY_TOKEN: AGENT_TOKEN });

      expect(r.status).toBe(ExitCode.AUTH);
      expect(r.stdout).toBe("");
      // The token was the one sent: this is its refusal, not a missing credential's.
      expect(bearers).toContain(`Bearer ${AGENT_TOKEN}`);
      expect(r.stderr).toContain("an agent: stop and report this to your operator");
      expect(r.stderr).toContain("a login cannot fix that: OXY_TOKEN wins over the login cache");
      expect(r.stderr).not.toContain("try `oxyc login` again");
      expect(r.stderr).not.toContain(AGENT_TOKEN);
    }
  );

  it("says the same with the variable named, where a login was never a fallback", async () => {
    status = 401;
    const r = await oxyc(["api", "/api/orgs", "--token-env", "OXY_TOKEN"], {
      OXY_TOKEN: AGENT_TOKEN
    });
    expect(r.status).toBe(ExitCode.AUTH);
    expect(r.stderr).toContain("an agent: stop and report this to your operator");
    expect(r.stderr).not.toContain("try `oxyc login` again");
  });

  it("whoami says so too, and names no login as the fix", async () => {
    status = 401;
    const r = await oxyc(["whoami"], { OXY_TOKEN: AGENT_TOKEN });
    expect(r.status).toBe(ExitCode.AUTH);
    expect(r.stdout).toBe("");
    expect(r.stderr).toContain("the token in OXY_TOKEN is no longer accepted");
    expect(r.stderr).toContain("an agent: stop and report this to your operator");
    expect(r.stderr).not.toMatch(/→ oxyc login/);
  });

  it("exits 4 without sending anything when the named variable was lost", async () => {
    // What `--token-env OXY_TOKEN` is for: no silent fall back to a cached login.
    const r = await oxyc(["api", "/api/orgs", "--token-env", "OXY_TOKEN"], { OXY_TOKEN: "" });
    expect(r.status).toBe(ExitCode.AUTH);
    expect(r.stderr).toContain("OXY_TOKEN is not set");
    expect(bearers).toEqual([]);
  });
});
