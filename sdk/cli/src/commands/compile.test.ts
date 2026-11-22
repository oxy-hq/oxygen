/**
 * `oxyc compile <branch>`, driven through the real binary against a fake
 * deployment — `oltp-audit.test.ts`'s shape, for the verb that replaced the
 * customer template's `oxy compile --skip-migrations --json`.
 *
 * What is pinned: the two paths it sends, the `--json` report a CI script
 * parses (the `failures` array of `oxy compile --json`: `{path, kind, message}`,
 * always an array), and the exit codes — 0 compiled, 9 compiled with file
 * failures (`--json` included), 1 failed with none, 2 usage before any request,
 * 5 a branch GitHub does not have, 7 `--wait` ran out.
 */

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import type { StagingCompile } from "../publish/semantic-branch.js";
import { ExitCode } from "../util/errors.js";
import { type CompileReport, toReport } from "./compile.js";

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");
const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});
function scratch(prefix: string): string {
  const dir = mkdtempSync(join(tmpdir(), prefix));
  SCRATCH.push(dir);
  return dir;
}

const WS = "11111111-2222-3333-4444-555555555555";
const SHA = "0123456789abcdef0123456789abcdef01234567";
const BASE = `/api/${WS}/compile/staging`;

let server: Server;
let base: string;
let requests: Array<{ method: string; url: string }> = [];
/** Answers, in order: the POST's, then each status read's. */
let answers: Array<{ status: number; body: unknown }> = [];

beforeEach(() => {
  requests = [];
  answers = [];
});

beforeAll(async () => {
  server = createServer((req, res) => {
    req.resume();
    req.on("end", () => {
      requests.push({ method: req.method ?? "", url: req.url ?? "" });
      const next = answers.shift();
      if (!next) {
        res.writeHead(500, { "content-type": "text/plain" });
        return res.end("no more answers");
      }
      const text = typeof next.body === "string" ? next.body : JSON.stringify(next.body);
      res.writeHead(next.status, {
        "content-type": typeof next.body === "string" ? "text/plain" : "application/json"
      });
      res.end(text);
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(() => server.close());

function oxyc(args: string[]): Promise<{ status: number; stdout: string; stderr: string }> {
  if (!existsSync(BIN)) throw new Error(`${BIN} missing — run \`pnpm build\``);
  const home = scratch("oxyc-compile-");
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

const ok = (body: Partial<StagingCompile>) => ({
  status: 200,
  body: { workspace_id: WS, git_sha: SHA, ...body }
});

const FAILURES = [
  { path: "semantics/orders.view.yml", kind: "yaml", message: "mapping values are not allowed" },
  { path: "config.yml", kind: "validation", message: "unknown field `databse`" }
];

const sent = () => requests.map((q) => `${q.method} ${q.url}`);

describe("oxyc compile", () => {
  it("POSTs the branch, polls the status by the commit, and exits 0 once it is ready", async () => {
    answers = [
      ok({ status: "pending", task_id: "t1" }),
      ok({ status: "compiling" }),
      ok({ status: "ready", revision_id: "rev-1" })
    ];
    const r = await oxyc(["compile", "feat/views", "--workspace", WS, "--wait"]);
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    expect(sent()).toEqual([
      `POST ${BASE}?branch=feat%2Fviews`,
      `GET ${BASE}/status?git_sha=${SHA}`,
      `GET ${BASE}/status?git_sha=${SHA}`
    ]);
    expect(r.stdout).toContain("feat/views compiles");
    expect(r.stdout).toContain("rev-1");
  });

  it("exits 9 on file failures and prints each one", async () => {
    answers = [
      ok({ status: "pending", task_id: "t1" }),
      ok({
        status: "failed",
        error: JSON.stringify({ failures: FAILURES }),
        failures: FAILURES,
        file_count_seen: 4,
        file_count_compiled: 2,
        file_count_failed: 2
      })
    ];
    const r = await oxyc(["compile", "main", "--workspace", WS, "--wait"]);
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(r.stdout).toContain("semantics/orders.view.yml [yaml] mapping values are not allowed");
    expect(r.stdout).toContain("4 seen, 2 compiled, 2 failed");
    expect(r.stderr).toContain("recorded 2 compile failure(s)");
  });

  /**
   * THE CONTRACT A CI SCRIPT PARSES. `oxy compile --json` printed a `failures`
   * array of `{path, kind, message}`, plus `status` and the file counts, and
   * the customer template's validate job read exactly those with jq. This pins
   * the keys and the item shape; changing either breaks customer CI.
   */
  it("--json keeps oxy compile's failures array shape, and still exits 9", async () => {
    answers = [
      ok({ status: "pending", task_id: "t1" }),
      ok({
        status: "failed",
        failures: FAILURES,
        file_count_seen: 4,
        file_count_compiled: 2,
        file_count_failed: 2
      })
    ];
    const r = await oxyc(["compile", "main", "--workspace", WS, "--wait", "--json"]);
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    const report = JSON.parse(r.stdout) as CompileReport;
    expect(Object.keys(report).sort()).toEqual(
      [
        "branch",
        "error",
        "failures",
        "file_count_compiled",
        "file_count_failed",
        "file_count_seen",
        "git_sha",
        "revision_id",
        "status",
        "workspace_id"
      ].sort()
    );
    expect(report.status).toBe("failed");
    expect(report.failures).toEqual(FAILURES);
    for (const f of report.failures)
      expect(Object.keys(f).sort()).toEqual(["kind", "message", "path"]);
    expect(report.file_count_seen).toBe(4);
  });

  it("--json on success has an empty failures array, never a missing one", async () => {
    answers = [ok({ status: "ready", revision_id: "rev-2" })];
    const r = await oxyc(["compile", "main", "--workspace", WS, "--wait", "--json"]);
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    const report = JSON.parse(r.stdout) as CompileReport;
    expect(report.failures).toEqual([]);
    expect(report.status).toBe("ready");
    expect(report.revision_id).toBe("rev-2");
    expect(sent()).toEqual([`POST ${BASE}?branch=main`]);
  });

  it("exits 1, not 9, when the compile failed with no file failure", async () => {
    answers = [
      ok({ status: "pending", task_id: "t1" }),
      ok({ status: "failed", error: "the compile failed before it produced a revision" })
    ];
    const r = await oxyc(["compile", "main", "--workspace", WS, "--wait"]);
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stderr).toContain("the compile failed before it produced a revision");
  });

  it("without --wait, queues it and returns 0 with the state", async () => {
    answers = [ok({ status: "pending", task_id: "t1" })];
    const r = await oxyc(["compile", "main", "--workspace", WS, "--json"]);
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    expect(JSON.parse(r.stdout).status).toBe("pending");
    expect(sent()).toEqual([`POST ${BASE}?branch=main`]);
  });

  it("exits 7 when --wait runs out", async () => {
    answers = Array.from({ length: 50 }, () => ok({ status: "compiling", task_id: "t1" }));
    const r = await oxyc(["compile", "main", "--workspace", WS, "--wait", "--timeout", "0.001"]);
    expect(r.status).toBe(ExitCode.UNAVAILABLE);
    expect(r.stderr).toContain("did not finish compiling in time");
  });

  it("exits 5 for a branch GitHub does not have", async () => {
    answers = [{ status: 404, body: 'branch "nope" not found' }];
    const r = await oxyc(["compile", "nope", "--workspace", WS, "--wait"]);
    expect(r.status).toBe(ExitCode.NOT_FOUND);
    expect(r.stderr).toContain("push it first");
  });

  it("exits 4 when the credential may not compile the workspace", async () => {
    answers = [{ status: 403, body: "" }];
    const r = await oxyc(["compile", "main", "--workspace", WS]);
    expect(r.status).toBe(ExitCode.AUTH);
    expect(r.stderr).toContain("trust policy");
  });

  it("refuses a bad --timeout before sending anything", async () => {
    const r = await oxyc(["compile", "main", "--workspace", WS, "--wait", "--timeout", "-3"]);
    expect(r.status).toBe(ExitCode.USAGE);
    expect(sent()).toEqual([]);
  });

  it("names --workspace when it is missing, before sending anything", async () => {
    const r = await oxyc(["compile", "main"]);
    expect(r.status).toBe(ExitCode.USAGE);
    expect(r.stderr).toContain("--workspace");
    expect(sent()).toEqual([]);
  });
});

describe("toReport", () => {
  it("fills what an older server leaves out with null and an empty failures array", () => {
    expect(toReport({ git_sha: SHA, status: "failed", error: "x" }, "b")).toEqual({
      status: "failed",
      workspace_id: null,
      branch: "b",
      git_sha: SHA,
      revision_id: null,
      file_count_seen: null,
      file_count_compiled: null,
      file_count_failed: null,
      failures: [],
      error: "x"
    });
  });
});
