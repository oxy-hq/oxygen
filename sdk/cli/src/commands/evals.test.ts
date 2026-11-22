/**
 * `oxyc evals`, driven through the real binary against a fake deployment, plus
 * the pure pieces it ports from `oxy test` (the flag rules, the threshold gate
 * and the results path).
 *
 * What is pinned: the wire contract with `/evals/runs` (paths and bodies), the
 * exit codes case by case, and that the results file is the server's
 * `results_json` BYTE FOR BYTE — `fixtures/evals/results.json` is a copy of
 * the fixture `oxy-api-eval`'s `results_pin_tests` holds the engine to, and it
 * carries `1.0` / `0.0` scores that a JSON round trip would rewrite.
 */

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import {
  CLI_KILL_TIMEOUT_MS,
  CLI_TEST_TIMEOUT_MS,
  cliRunError,
  reapChildrenAfterEach
} from "../testing/cli-child.js";
import { ExitCode } from "../util/errors.js";
import { gate, resultsPath, summaryLines, validateFlags } from "./evals.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const BIN = resolve(HERE, "..", "..", "dist", "main.mjs");
const RESULTS = readFileSync(
  resolve(HERE, "..", "..", "fixtures", "evals", "results.json"),
  "utf8"
);
// Every binary-driven case is one or more cold `node` starts, whose cost is the machine's
// CPU. See `testing/cli-child.ts` for the budget and the kill timeout.
vi.setConfig({ testTimeout: CLI_TEST_TIMEOUT_MS });
const track = reapChildrenAfterEach();
const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

const WS = "11111111-2222-3333-4444-555555555555";
const BASE = `/api/${WS}/evals/runs`;
const FILE = "tests/sales.performance.test.yml";

let server: Server;
let base: string;
let requests: Array<{ method: string; url: string; body: string }> = [];
let created: { status: number; body: unknown } = { status: 202, body: {} };
/** The views `GET …/runs/run-9` answers, in order; the last one repeats. */
let views: unknown[] = [];
let gets = 0;

function view(status: string, extra: Record<string, unknown> = {}): Record<string, unknown> {
  const done = status === "done";
  return {
    run_id: "run-9",
    status,
    test_files: [FILE],
    error: null,
    accuracies: done ? [0.5] : null,
    results: done ? JSON.parse(RESULTS) : null,
    results_json: done ? RESULTS : null,
    ...extra
  };
}

beforeEach(() => {
  requests = [];
  created = { status: 202, body: { run_id: "run-9", test_files: [FILE] } };
  views = [view("running"), view("done")];
  gets = 0;
});

function json(res: ServerResponse, value: unknown, status = 200): void {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(value));
}

beforeAll(async () => {
  server = createServer((req, res) => {
    let body = "";
    req.on("data", (c) => {
      body += c;
    });
    req.on("end", () => {
      const url = req.url ?? "";
      const method = req.method ?? "";
      requests.push({ method, url, body });
      if (method === "POST" && url === BASE) return json(res, created.body, created.status);
      if (method === "GET" && url === `${BASE}/run-9`) {
        const v = views[Math.min(gets, views.length - 1)];
        gets += 1;
        return json(res, v);
      }
      res.writeHead(404);
      res.end("{}");
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(() => server.close());

function oxyc(
  cwd: string,
  ...args: string[]
): Promise<{ status: number; stdout: string; stderr: string }> {
  if (!existsSync(BIN)) throw new Error(`${BIN} missing — run \`pnpm build\``);
  const home = mkdtempSync(join(tmpdir(), "oxyc-evals-"));
  SCRATCH.push(home);
  return new Promise((done, fail) => {
    const child = track(
      spawn(process.execPath, [BIN, ...args, "--target", base], {
        cwd,
        env: {
          ...process.env,
          HOME: home,
          OXY_CREDENTIALS_PATH: join(home, "credentials.json"),
          OXYC_CACHE_DIR: join(home, "cache"),
          OXY_TOKEN: "test-token",
          OXYC_POLL_MS: "10",
          NO_COLOR: "1"
        },
        timeout: CLI_KILL_TIMEOUT_MS
      })
    );
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (d) => {
      stdout += d;
    });
    child.stderr.on("data", (d) => {
      stderr += d;
    });
    child.on("error", (e) => fail(cliRunError(args, e.message)));
    child.on("close", (status, signal) =>
      signal ? fail(cliRunError(args, signal)) : done({ status: status ?? -1, stdout, stderr })
    );
  });
}

function scratch(): string {
  const dir = mkdtempSync(join(tmpdir(), "oxyc-evals-cwd-"));
  SCRATCH.push(dir);
  return dir;
}

const sent = () => requests.map((q) => `${q.method} ${q.url}`);
const posted = () => JSON.parse(requests.find((q) => q.method === "POST")?.body ?? "{}");

// Ported from `oxy test`'s Rust tests (`test_command_tests.rs`): the flag rules,
// the threshold gate and the results path.
describe("the rules oxy test applied", () => {
  const flags = { thresholdMode: "average" as const, json: false };

  it("refuses a bad threshold, a non-test file, and --case / --regrade without a file", () => {
    expect(() => validateFlags(undefined, { ...flags, minAccuracy: 1.5 })).toThrow(
      "min-accuracy must be between 0.0 and 1.0, got: 1.5"
    );
    expect(() => validateFlags(undefined, { ...flags, minAccuracy: -0.1 })).toThrow();
    expect(() => validateFlags("a.agentic.yml", flags)).toThrow("Unsupported test file");
    expect(() => validateFlags(undefined, { ...flags, case: "0" })).toThrow("--case requires");
    expect(() => validateFlags(undefined, { ...flags, regrade: "r.json" })).toThrow(
      "--regrade requires"
    );
    expect(() => validateFlags("a.test.yml", { ...flags, regrade: "r", tag: "t" })).toThrow(
      "cannot be combined"
    );
    expect(() => validateFlags("a.test.yml", { ...flags, minAccuracy: 0 })).not.toThrow();
    expect(() => validateFlags("a.test.yml", { ...flags, minAccuracy: 1 })).not.toThrow();
  });

  it("gates on the average, or on every test, with oxy test's messages", () => {
    expect(gate([0.9, 0.7], 0.8, "average")).toEqual({});
    expect(gate([0.9, 0.6], 0.8, "average").failure).toBe(
      "Average accuracy 0.7500 below threshold 0.8000"
    );
    expect(gate([0.9, 0.85], 0.8, "all")).toEqual({});
    expect(gate([0.9, 0.7, 0.5], 0.8, "all").failure).toBe(
      "2 test(s) below threshold 0.8000: Test 2: 0.7000, Test 3: 0.5000"
    );
    // Exactly at the threshold passes.
    expect(gate([0.8], 0.8, "average")).toEqual({});
    expect(gate([], 0.8, "average").warning).toContain("no accuracy metrics found");
  });

  it("names the results file after the test file, as oxy test did", () => {
    expect(resultsPath("sales.agentic.test.yml")).toBe("sales.agentic.test.results.json");
    expect(resultsPath("tests/x.test.yml")).toBe("tests/x.test.results.json");
    expect(resultsPath(undefined)).toBe("test-results.json");
  });

  it("summarises each test file and lists the runs that did not pass", () => {
    const lines = summaryLines(JSON.parse(RESULTS));
    expect(lines[0]).toBe("sales.performance.test.yml  Correctness 0.5000  answered 1/2");
    expect(lines.slice(1)).toEqual(["  FAIL  What was total revenue across all stores in 2025?"]);
  });
});

describe("oxyc evals run", () => {
  it("queues the file, waits for the run, and writes the server's results byte for byte", async () => {
    const cwd = scratch();
    const r = await oxyc(cwd, "evals", "run", FILE, "--workspace", WS, "--output-json", "out.json");
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    expect(sent()).toEqual([`POST ${BASE}`, `GET ${BASE}/run-9`, `GET ${BASE}/run-9`]);
    expect(posted()).toEqual({ test_file: FILE });
    expect(readFileSync(join(cwd, "out.json"), "utf8")).toBe(RESULTS);
    expect(RESULTS).toContain('"score": 1.0,');
    expect(r.stdout).toContain("Correctness 0.5000");
  });

  it("--json prints the same document, verbatim", async () => {
    const r = await oxyc(scratch(), "evals", "run", FILE, "--workspace", WS, "--json");
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    expect(r.stdout).toBe(RESULTS);
  });

  it("--output-json with no file writes beside the test file's name", async () => {
    const cwd = scratch();
    const r = await oxyc(cwd, "evals", "run", "sales.test.yml", "--workspace", WS, "--output-json");
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    expect(readFileSync(join(cwd, "sales.test.results.json"), "utf8")).toBe(RESULTS);
  });

  it("sends --case, --tag and the regrade file's text", async () => {
    const cwd = scratch();
    writeFileSync(join(cwd, "prev.json"), RESULTS);
    let r = await oxyc(cwd, "evals", "run", FILE, "--workspace", WS, "--case", "2", "--tag", "t");
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    expect(posted()).toEqual({ test_file: FILE, case: "2", tag: "t" });
    requests = [];
    gets = 0;
    r = await oxyc(cwd, "evals", "run", FILE, "--workspace", WS, "--regrade", "prev.json");
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    expect(posted()).toEqual({ test_file: FILE, regrade: RESULTS });
  });

  it("exits 9 under --min-accuracy, after writing the results", async () => {
    const cwd = scratch();
    const r = await oxyc(
      cwd,
      "evals",
      "run",
      FILE,
      "--workspace",
      WS,
      "--min-accuracy",
      "0.8",
      "--output-json",
      "out.json"
    );
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(r.stderr).toContain("Average accuracy 0.5000 below threshold 0.8000");
    expect(readFileSync(join(cwd, "out.json"), "utf8")).toBe(RESULTS);
  });

  it("passes at or above --min-accuracy", async () => {
    const r = await oxyc(
      scratch(),
      "evals",
      "run",
      FILE,
      "--workspace",
      WS,
      "--min-accuracy",
      "0.5"
    );
    expect(r.status, r.stderr).toBe(ExitCode.OK);
  });

  it("exits 1 when the run failed, with its error", async () => {
    views = [view("failed", { error: "No model configured" })];
    const r = await oxyc(scratch(), "evals", "run", FILE, "--workspace", WS);
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stderr).toContain("eval run run-9 failed");
    expect(r.stderr).toContain("No model configured");
  });

  it("exits 7 when --wait runs out, naming the run", async () => {
    views = [view("running")];
    const r = await oxyc(scratch(), "evals", "run", FILE, "--workspace", WS, "--wait", "0.05");
    expect(r.status).toBe(ExitCode.UNAVAILABLE);
    expect(r.stderr).toContain("oxyc evals wait run-9");
  });

  it("a bare --wait waits to the end, as run does without it", async () => {
    const r = await oxyc(scratch(), "evals", "run", FILE, "--workspace", WS, "--wait");
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    expect(sent()).toEqual([`POST ${BASE}`, `GET ${BASE}/run-9`, `GET ${BASE}/run-9`]);
  });

  it("--detach prints the run id and does not wait", async () => {
    const r = await oxyc(scratch(), "evals", "run", FILE, "--workspace", WS, "--detach");
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    expect(r.stdout.trim()).toBe("run-9");
    expect(sent()).toEqual([`POST ${BASE}`]);
  });

  it("a revision that predates compiled test files is retryable (503 → 7)", async () => {
    created = {
      status: 503,
      body: "not_compiled: the served revision was compiled before test files were"
    };
    const r = await oxyc(scratch(), "evals", "run", FILE, "--workspace", WS);
    expect(r.status).toBe(ExitCode.UNAVAILABLE);
    expect(r.stderr).toContain("a compile is queued");
  });

  it("maps the route's refusals: 404 → 5, 400 → 6", async () => {
    created = { status: 404, body: "tests/x.test.yml is not in the served revision" };
    let r = await oxyc(scratch(), "evals", "run", FILE, "--workspace", WS);
    expect(r.status).toBe(ExitCode.NOT_FOUND);
    expect(r.stderr).toContain("push it first");
    created = { status: 400, body: "No test case with name or prompt" };
    r = await oxyc(scratch(), "evals", "run", FILE, "--workspace", WS, "--case", "nope");
    expect(r.status).toBe(ExitCode.REQUEST);
  });

  it("refuses bad flags before sending anything (exit 2)", async () => {
    let r = await oxyc(scratch(), "evals", "run", "--workspace", WS, "--case", "0");
    expect(r.status).toBe(ExitCode.USAGE);
    r = await oxyc(scratch(), "evals", "run", FILE, "--workspace", WS, "--min-accuracy", "2");
    expect(r.status).toBe(ExitCode.USAGE);
    r = await oxyc(scratch(), "evals", "run", FILE, "--workspace", WS, "--threshold-mode", "most");
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });

  it("needs --workspace", async () => {
    const r = await oxyc(scratch(), "evals", "run", FILE);
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });
});

describe("oxyc evals wait", () => {
  it("waits for a queued run and gates it like run", async () => {
    const cwd = scratch();
    const r = await oxyc(
      cwd,
      "evals",
      "wait",
      "run-9",
      "--workspace",
      WS,
      "--min-accuracy",
      "0.9",
      "--threshold-mode",
      "all",
      "--output-json"
    );
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(r.stderr).toContain("1 test(s) below threshold 0.9000: Test 1: 0.5000");
    // One test file in the run: the default is named after it.
    expect(readFileSync(join(cwd, "tests/sales.performance.test.results.json"), "utf8")).toBe(
      RESULTS
    );
  });
});
