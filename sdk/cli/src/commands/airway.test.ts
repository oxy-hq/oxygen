/**
 * `oxyc airway`, driven through the real binary against a fake deployment.
 *
 * A LOCAL SERVER, for `oltp.test.ts`'s reason: what is under test is the wire
 * contract with the `/agentic-airway` routes — the paths, the bodies, the SSE
 * stream, and that nothing destructive is sent before the confirmation is
 * settled. The exit codes are pinned case by case, because they are the
 * contract a script branches on.
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
import { type CoverageReport, settled, snapshot } from "./airway-backfill.js";
import { encodeName } from "./airway-leases.js";
import {
  FAILED_STATUSES,
  isFailed,
  isTerminal,
  isTerminalEvent,
  TERMINAL_STATUSES
} from "./airway-run.js";

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");
const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

const WS = "11111111-2222-3333-4444-555555555555";
const BASE = `/api/${WS}/agentic-airway`;
const REF = "pipelines/toast.airway.yml";

const LEASE = {
  pipeline_name: "toast_orders",
  run_id: "run-1",
  acquired_at: "2026-11-21T01:00:00Z",
  expires_at: "2026-11-21T07:00:00Z",
  lapsed: false
};

let server: Server;
let base: string;
let requests: Array<{ method: string; url: string; body: string }> = [];
let leases: unknown[] = [];
let release: { status: number; body: unknown } = { status: 200, body: {} };
let start: { status: number; body: unknown } = { status: 200, body: { run_id: "run-9" } };
/** The SSE frames `GET …/runs/run-9/events` sends. */
let frames: Array<{ event: string; data: unknown }> = [];
let runStatus = "done";
let ranges: unknown[] = [];
let coverageCalls = 0;

beforeEach(() => {
  requests = [];
  leases = [LEASE];
  release = {
    status: 200,
    body: { outcome: "released", pipeline_name: "toast_orders", removed: 1 }
  };
  start = { status: 200, body: { run_id: "run-9" } };
  frames = [
    { event: "load_started", data: {} },
    { event: "extract_completed", data: { table: "orders", rows_extracted: 12 } },
    { event: "load_completed", data: { duration_ms: 40 } }
  ];
  runStatus = "done";
  ranges = [];
  coverageCalls = 0;
});

function json(res: ServerResponse, value: unknown, status = 200): void {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(value));
}

const COVERAGE = {
  pipeline_ref: REF,
  range_id: "range-1",
  chunks: [
    {
      period_start: "2026-01-01T00:00:00+00:00",
      period_end: "2026-02-01T00:00:00+00:00",
      status: "done",
      run_id: "r",
      row_count: 3,
      attempts: 1,
      error: null
    },
    {
      period_start: "2026-02-01T00:00:00+00:00",
      period_end: "2026-03-01T00:00:00+00:00",
      status: "failed",
      run_id: "r2",
      row_count: null,
      attempts: 2,
      error: "429 from upstream"
    }
  ],
  summary: {
    total: 2,
    done: 1,
    loaded_from: "2026-01-01T00:00:00+00:00",
    loaded_to: "2026-02-01T00:00:00+00:00",
    missing: 1
  }
};

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
      if (method === "GET" && url === `${BASE}/leases`) return json(res, leases);
      if (method === "DELETE" && url.startsWith(`${BASE}/leases/`)) {
        return json(res, release.body, release.status);
      }
      if (method === "POST" && url === `${BASE}/runs`) return json(res, start.body, start.status);
      if (method === "GET" && url === `${BASE}/runs/run-9/events`) {
        res.writeHead(200, { "content-type": "text/event-stream" });
        frames.forEach((f, i) => {
          res.write(`id: ${i}\nevent: ${f.event}\ndata: ${JSON.stringify(f.data)}\n\n`);
        });
        return res.end();
      }
      if (method === "GET" && url.startsWith(`${BASE}/runs?`)) {
        return json(res, [{ run_id: "run-9", status: runStatus }]);
      }
      if (method === "POST" && url === `${BASE}/chunked-backfill`) {
        return json(res, { range_id: "range-2", chunk_count: 3 });
      }
      if (method === "POST" && url === `${BASE}/resume-backfill`) {
        return json(res, { range_id: "range-1", chunk_count: 1 });
      }
      if (method === "GET" && url.startsWith(`${BASE}/backfill-ranges?`)) return json(res, ranges);
      if (method === "GET" && url.startsWith(`${BASE}/coverage?`)) {
        // The first read is the pre-resume state; later ones show the failed
        // chunk re-driven (attempts 2 → 3) and failing again.
        coverageCalls += 1;
        if (coverageCalls === 1) return json(res, COVERAGE);
        const [first, second] = COVERAGE.chunks;
        return json(res, { ...COVERAGE, chunks: [first, { ...second, attempts: 3 }] });
      }
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
  const home = mkdtempSync(join(tmpdir(), "oxyc-airway-"));
  SCRATCH.push(home);
  return new Promise((done) => {
    // stdin is a pipe, not a terminal — which is what release-lease without
    // `--yes` has to refuse on.
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

// Ported from `oxy airway run`'s Rust tests, which pinned the same two lists.
describe("run statuses", () => {
  it("every failed status also ends the wait", () => {
    for (const s of FAILED_STATUSES) expect(TERMINAL_STATUSES).toContain(s);
  });

  it("a lost pipeline exits non-zero and a clean one does not", () => {
    for (const s of ["failed", "cancelled", "timed_out"]) {
      expect(isTerminal(s)).toBe(true);
      expect(isFailed(s)).toBe(true);
    }
    expect(isTerminal("done")).toBe(true);
    expect(isFailed("done")).toBe(false);
  });

  it("a running or unknown status keeps waiting", () => {
    for (const s of [undefined, null, "running", "delegating", "awaiting_input", ""]) {
      expect(isTerminal(s)).toBe(false);
    }
  });

  it("ends the stream on agentic-http's airway terminal events, and only those", () => {
    for (const e of [
      "done",
      "error",
      "cancelled",
      "load_completed",
      "pipeline_error",
      "task_failed"
    ]) {
      expect(isTerminalEvent(e)).toBe(true);
    }
    for (const e of ["load_started", "extract_completed", "subrun_completed", "recovery_resumed"]) {
      expect(isTerminalEvent(e)).toBe(false);
    }
  });
});

describe("airway leases", () => {
  it("lists the workspace's leases", async () => {
    const r = await oxyc("airway", "leases", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`GET ${BASE}/leases`]);
    expect(r.stdout).toContain("toast_orders");
    expect(r.stdout).toContain("run-1");
  });

  it("says so when nothing is held", async () => {
    leases = [];
    const r = await oxyc("airway", "leases", "--workspace", WS);
    expect(r.status).toBe(0);
    expect(r.stdout).toContain("No airway leases held.");
  });

  it("needs --workspace, before any request", async () => {
    const r = await oxyc("airway", "leases");
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });
});

describe("airway release-lease", () => {
  const DELETE = `DELETE ${BASE}/leases/${encodeName("toast_orders")}`;

  it("refuses with exit 8 and sends no DELETE when there is no terminal to ask", async () => {
    const r = await oxyc("airway", "release-lease", "toast_orders", "--workspace", WS);
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(sent()).toEqual([`GET ${BASE}/leases`]);
    expect(r.stderr).toContain("held by run run-1");
  });

  it("--yes releases the holder it listed, scoped to that run", async () => {
    const r = await oxyc("airway", "release-lease", "toast_orders", "--workspace", WS, "--yes");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`GET ${BASE}/leases`, `${DELETE}?run_id=run-1`]);
    expect(r.stdout).toContain("Released 1 lease(s) for `toast_orders`");
  });

  it("--force releases whatever holds it, without listing first", async () => {
    const r = await oxyc("airway", "release-lease", "toast_orders", "--workspace", WS, "--force");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`${DELETE}?force=true`]);
  });

  it("nothing held is success, and sends no DELETE", async () => {
    leases = [];
    const r = await oxyc("airway", "release-lease", "toast_orders", "--workspace", WS, "--yes");
    expect(r.status).toBe(0);
    expect(sent()).toEqual([`GET ${BASE}/leases`]);
    expect(r.stdout).toContain("nothing to release");
  });

  it("a different holder by the time of the delete is exit 6, never widened", async () => {
    release = {
      status: 409,
      body: {
        error: "lease_held_by_another_run",
        pipeline_name: "toast_orders",
        holder: { ...LEASE, run_id: "run-2" }
      }
    };
    const r = await oxyc("airway", "release-lease", "toast_orders", "--workspace", WS, "--yes");
    expect(r.status).toBe(ExitCode.REQUEST);
    expect(r.stderr).toContain("run run-2 does now");
    expect(r.stderr).not.toContain("--force");
    expect(sent()).toEqual([`GET ${BASE}/leases`, `${DELETE}?run_id=run-1`]);
  });

  it("the listed run re-acquiring is exit 6 and says it is live", async () => {
    release = {
      status: 409,
      body: { error: "lease_reheld_by_same_run", pipeline_name: "toast_orders" }
    };
    const r = await oxyc("airway", "release-lease", "toast_orders", "--workspace", WS, "--yes");
    expect(r.status).toBe(ExitCode.REQUEST);
    expect(r.stderr).toContain("holds the lease again and is live");
  });

  it("a workspace member is refused with exit 4", async () => {
    release = { status: 403, body: {} };
    const r = await oxyc("airway", "release-lease", "toast_orders", "--workspace", WS, "--force");
    expect(r.status).toBe(ExitCode.AUTH);
  });
});

describe("airway run", () => {
  it("starts the compiled pipeline, streams to the end and exits 0 on a done row", async () => {
    const r = await oxyc("airway", "run", REF, "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(requests[0]).toMatchObject({ method: "POST", url: `${BASE}/runs` });
    expect(JSON.parse(requests[0]?.body ?? "{}")).toEqual({ pipeline_ref: REF, resources: [] });
    expect(sent()).toContain(`GET ${BASE}/runs/run-9/events`);
    expect(r.stdout).toContain("extracted orders (12 rows)");
    expect(r.stdout).toContain("✓ load completed (40 ms)");
  });

  it("--json prints oxy airway run's {seq, event_type, payload} lines", async () => {
    const r = await oxyc("airway", "run", REF, "--workspace", WS, "--json", "--resources", "a,b");
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(requests[0]?.body ?? "{}").resources).toEqual(["a", "b"]);
    const lines = r.stdout
      .trim()
      .split("\n")
      .map((l) => JSON.parse(l));
    expect(lines[1]).toEqual({
      seq: 1,
      event_type: "extract_completed",
      payload: { table: "orders", rows_extracted: 12 }
    });
    expect(lines.map((l) => l.event_type)).toEqual([
      "load_started",
      "extract_completed",
      "load_completed"
    ]);
  });

  it("exits 1 when the run row says failed", async () => {
    frames = [{ event: "pipeline_error", data: { error: "toast 401" } }];
    runStatus = "failed";
    const r = await oxyc("airway", "run", REF, "--workspace", WS);
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stdout).toContain("✗ pipeline error: toast 401");
  });

  it("the row, not the event, decides: a done row exits 0 after an error event", async () => {
    frames = [{ event: "pipeline_error", data: { error: "one resource skipped" } }];
    runStatus = "done";
    const r = await oxyc("airway", "run", REF, "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
  });

  it("a pipeline already running is exit 6", async () => {
    start = {
      status: 409,
      body: { error: "pipeline_already_running", pipeline_name: "toast_orders", run_id: "run-1" }
    };
    const r = await oxyc("airway", "run", REF, "--workspace", WS);
    expect(r.status).toBe(ExitCode.REQUEST);
    expect(r.stderr).toContain("pipeline_already_running");
  });

  it("a ref the deployment cannot serve yet is exit 7", async () => {
    start = { status: 503, body: { error: "needs_recompile" } };
    const r = await oxyc("airway", "run", REF, "--workspace", WS);
    expect(r.status).toBe(ExitCode.UNAVAILABLE);
  });

  it("--variables rides the start request; the shell environment never does", async () => {
    const r = await oxyc("airway", "run", REF, "--workspace", WS, "--variables", '{"BBOX":"1,2"}');
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(requests[0]?.body ?? "{}").variables).toEqual({ BBOX: "1,2" });
  });

  it("--detach prints the run id and does not stream", async () => {
    const r = await oxyc("airway", "run", REF, "--workspace", WS, "--detach");
    expect(r.status).toBe(0);
    expect(r.stdout.trim()).toBe("run-9");
    expect(sent()).toEqual([`POST ${BASE}/runs`]);
  });
});

describe("airway backfill and coverage", () => {
  it("queues a chunked backfill with the window and scope", async () => {
    const r = await oxyc(
      "airway",
      "backfill",
      REF,
      "--workspace",
      WS,
      "--from",
      "2026-01-01T00:00:00Z",
      "--to",
      "2026-04-01T00:00:00Z",
      "--resources",
      "orders"
    );
    expect(r.status, r.stderr).toBe(0);
    const post = requests.find((q) => q.url === `${BASE}/chunked-backfill`);
    expect(JSON.parse(post?.body ?? "{}")).toEqual({
      pipeline_ref: REF,
      from: "2026-01-01T00:00:00.000Z",
      to: "2026-04-01T00:00:00.000Z",
      granularity: "month",
      resources: ["orders"]
    });
    expect(r.stdout).toContain("range range-2, 3 month chunk(s) queued");
  });

  it("names an unfinished range for the same window before starting a new one", async () => {
    ranges = [
      {
        id: "range-1",
        pipeline_ref: REF,
        requested_from: "2026-01-01T00:00:00+00:00",
        requested_to: "2026-04-01T00:00:00+00:00",
        granularity: "month",
        status: "failed",
        total: 3,
        done: 2,
        missing: 1
      }
    ];
    const r = await oxyc(
      "airway",
      "backfill",
      REF,
      "--workspace",
      WS,
      "--from",
      "2026-01-01T00:00:00Z",
      "--to",
      "2026-04-01T00:00:00Z"
    );
    expect(r.status, r.stderr).toBe(0);
    expect(r.stderr).toContain("oxyc airway backfill --resume range-1");
  });

  it("--resume re-drives an existing range", async () => {
    const r = await oxyc("airway", "backfill", "--resume", "range-1", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`POST ${BASE}/resume-backfill`]);
    expect(JSON.parse(requests[0]?.body ?? "{}")).toEqual({ range_id: "range-1" });
  });

  it("refuses an empty window with exit 2 before any request", async () => {
    const r = await oxyc(
      "airway",
      "backfill",
      REF,
      "--workspace",
      WS,
      "--from",
      "2026-04-01T00:00:00Z",
      "--to",
      "2026-01-01T00:00:00Z"
    );
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });

  it("--wait after a resume waits for the re-drive, then exits 1 on chunks still not done", async () => {
    const r = await oxyc(
      "airway",
      "backfill",
      "--resume",
      "range-1",
      "--workspace",
      WS,
      "--wait",
      "30"
    );
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stdout).toContain("1 period(s) not done");
    expect(r.stdout).toContain("(attempts 3)");
    expect(coverageCalls, "the pre-resume snapshot, then the poll").toBe(2);
  }, 20_000);

  it("a range is settled only when every chunk exists, none is in flight, and a resume re-drove the rest", () => {
    const report = COVERAGE as CoverageReport;
    expect(settled(report)).toBe(true);
    expect(settled(report, 3), "a fresh range's checkpoints are written lazily").toBe(false);
    const running = {
      ...report,
      chunks: [{ ...report.chunks[0], status: "running" }]
    } as CoverageReport;
    expect(settled(running)).toBe(false);
    const before = snapshot(report);
    expect(settled(report, 0, before), "the pre-resume failure is not a settle").toBe(false);
    const redriven = {
      ...report,
      chunks: [report.chunks[0], { ...report.chunks[1], attempts: 3 }]
    } as CoverageReport;
    expect(settled(redriven, 0, before)).toBe(true);
  });

  it("coverage --range prints the range's missing periods", async () => {
    const r = await oxyc("airway", "coverage", "--range", "range-1", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain("1/2 chunks done");
    expect(r.stdout).toContain(
      "✗ 2026-02-01 → 2026-03-01 [failed] (attempts 2): 429 from upstream"
    );
  });

  it("coverage <ref> lists the pipeline's ranges", async () => {
    ranges = [
      {
        id: "range-1",
        pipeline_ref: REF,
        requested_from: "2026-01-01T00:00:00+00:00",
        requested_to: "2026-04-01T00:00:00+00:00",
        granularity: "month",
        status: "degraded",
        total: 3,
        done: 2,
        missing: 1
      }
    ];
    const r = await oxyc("airway", "coverage", REF, "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`GET ${BASE}/backfill-ranges?pipeline_ref=${encodeURIComponent(REF)}`]);
    expect(r.stdout).toContain("range-1");
    expect(r.stdout).toContain("2/3");
  });
});
