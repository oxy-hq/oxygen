/**
 * `oxyc looker test`, driven through the real binary against a fake deployment
 * (`airway.test.ts`'s harness, for its reason): the wire contract with
 * `POST …/integrations/looker/{name}/test` and the exit codes a script
 * branches on, pinned case by case.
 */

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { createServer, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { ExitCode } from "../util/errors.js";
import type { LookerConnectionTest } from "./looker.js";

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");
const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

const WS = "11111111-2222-3333-4444-555555555555";
const TEST = `/api/${WS}/integrations/looker/my_looker/test`;

/** The route's body, as `the_response_shape_is_pinned` (Rust) pins it. */
const PASSED: LookerConnectionTest = {
  ok: true,
  stage: null,
  error: null,
  integration: "my_looker",
  base_url: "https://acme.looker.com:19999",
  models: [
    { name: "ecommerce", explores: 2 },
    { name: "finance", explores: 0 },
    { name: "m3", explores: 1 },
    { name: "m4", explores: 1 },
    { name: "m5", explores: 1 },
    { name: "m6", explores: 1 }
  ],
  elapsed_ms: 40
};

const FAILED: LookerConnectionTest = {
  ok: false,
  stage: "auth",
  error: "Authentication failed: Invalid credentials: Not found",
  integration: "my_looker",
  base_url: "https://acme.looker.com:19999",
  models: [],
  elapsed_ms: 12
};

let server: Server;
let base: string;
let requests: Array<{ method: string; url: string }> = [];
let answer: { status: number; body: unknown } = { status: 200, body: PASSED };

beforeEach(() => {
  requests = [];
  answer = { status: 200, body: PASSED };
});

function json(res: ServerResponse, value: unknown, status = 200): void {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(value));
}

beforeAll(async () => {
  server = createServer((req, res) => {
    req.resume();
    req.on("end", () => {
      const url = req.url ?? "";
      const method = req.method ?? "";
      requests.push({ method, url });
      if (method === "POST" && url === TEST) return json(res, answer.body, answer.status);
      res.writeHead(404);
      res.end("{}");
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(() => server.close());

/** Async for `oltp.test.ts`'s reason: `spawnSync` would block the server above. */
function oxyc(...args: string[]): Promise<{ status: number; stdout: string; stderr: string }> {
  if (!existsSync(BIN)) throw new Error(`${BIN} missing — run \`pnpm build\``);
  const home = mkdtempSync(join(tmpdir(), "oxyc-looker-"));
  SCRATCH.push(home);
  return new Promise((done) => {
    const child = spawn(process.execPath, [BIN, ...args, "--target", base], {
      env: {
        ...process.env,
        HOME: home,
        OXY_CREDENTIALS_PATH: join(home, "credentials.json"),
        OXYC_CACHE_DIR: join(home, "cache"),
        OXY_TOKEN: "test-token",
        NO_COLOR: "1"
      }
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (d) => {
      stdout += d;
    });
    child.stderr.on("data", (d) => {
      stderr += d;
    });
    child.on("close", (status) => done({ status: status ?? -1, stdout, stderr }));
  });
}

const sent = () => requests.map((q) => `${q.method} ${q.url}`);

describe("looker test", () => {
  it("posts to the integration's test route and prints the first five models", async () => {
    const r = await oxyc("looker", "test", "my_looker", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`POST ${TEST}`]);
    expect(r.stdout).toContain("Connection successful");
    expect(r.stdout).toContain("Available models: 6");
    expect(r.stdout).toContain("- ecommerce (2 explores)");
    expect(r.stdout).toContain("... and 1 more");
    expect(r.stdout).not.toContain("m6");
  });

  it("--json prints the server's body as is", async () => {
    const r = await oxyc("looker", "test", "my_looker", "--workspace", WS, "--json");
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(r.stdout)).toEqual(PASSED);
  });

  it("a check that ran and failed exits 9, naming the stage", async () => {
    answer = { status: 200, body: FAILED };
    const r = await oxyc("looker", "test", "my_looker", "--workspace", WS);
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(r.stderr).toContain("failed its check (auth)");
    expect(r.stderr).toContain("Invalid credentials");
  });

  it("--json still prints the body when the check fails, and still exits 9", async () => {
    answer = { status: 200, body: FAILED };
    const r = await oxyc("looker", "test", "my_looker", "--workspace", WS, "--json");
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(JSON.parse(r.stdout)).toEqual(FAILED);
  });

  it("an unknown integration exits 5", async () => {
    answer = { status: 404, body: "no Looker integration named 'my_looker' in the served config" };
    const r = await oxyc("looker", "test", "my_looker", "--workspace", WS);
    expect(r.status).toBe(ExitCode.NOT_FOUND);
    expect(r.stderr).toContain("no Looker integration by that name");
  });

  it("a caller who is not a workspace admin exits 4", async () => {
    answer = { status: 403, body: { error: "forbidden" } };
    const r = await oxyc("looker", "test", "my_looker", "--workspace", WS);
    expect(r.status).toBe(ExitCode.AUTH);
    expect(r.stderr).toContain("workspace admin");
  });

  it("a deployment that cannot serve the config yet exits 7", async () => {
    answer = { status: 503, body: { error: "needs_recompile" } };
    const r = await oxyc("looker", "test", "my_looker", "--workspace", WS);
    expect(r.status).toBe(ExitCode.UNAVAILABLE);
  });

  it("encodes the name as one path segment", async () => {
    const r = await oxyc("looker", "test", "a/b c", "--workspace", WS);
    expect(r.status).toBe(ExitCode.NOT_FOUND);
    expect(sent()).toEqual([`POST /api/${WS}/integrations/looker/a%2Fb%20c/test`]);
  });

  it("needs --workspace, before any request", async () => {
    const r = await oxyc("looker", "test", "my_looker");
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });
});
