/**
 * `oxyc agentic`, driven through the real binary against a fake deployment.
 *
 * THE `--json` CONTRACT. `fixtures/agentic/run.rust.jsonl` is what the Rust
 * `oxy agentic run … --json` printed for one analytics run:
 * `println!("{}", json!({ "seq", "event_type", "payload" }))` per UI event, with
 * serde_json's `preserve_order` on, each payload from
 * `agentic_core::ui_stream::serialize_ui_block` or (domain events) serde over
 * `AnalyticsEvent` with its tag removed. It is DERIVED from that code, not
 * captured: the Rust command needed a database and an LLM key, and it was
 * deleted in the PR that ported it. Its floats read `1.0`, as serde writes
 * them — a JavaScript re-serialization would print `1`, which is why
 * `agentic-stream.ts` keeps the frame's raw `data`.
 *
 * The fake server sends those events as `GET …/runs/{id}/events` does (`id` =
 * seq, `event` = type, `data` = the payload with `"attempt":0` appended), and
 * the test holds oxyc's stdout to the transcript, line for line.
 *
 * A LOCAL SERVER and the built binary, for `airway.test.ts`'s reason: the
 * contract is the wire and the exit code a script branches on.
 */

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createServer, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { ExitCode } from "../util/errors.js";
import { currentStage, delegationInfo, mapPool, TREE_READS_IN_FLIGHT } from "./agentic-runs.js";
import { isFailureEvent, isTerminalEvent, jsonLine, SUSPEND_QUIET_MS } from "./agentic-stream.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const BIN = resolve(HERE, "..", "..", "dist", "main.mjs");
const TRANSCRIPT = readFileSync(
  resolve(HERE, "..", "..", "fixtures", "agentic", "run.rust.jsonl"),
  "utf8"
)
  .split("\n")
  .filter(Boolean);
const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

const WS = "11111111-2222-3333-4444-555555555555";
const BASE = `/api/${WS}/analytics`;

interface Frame {
  id: number;
  event: string;
  data: string;
}

/** A transcript line's parts, its payload kept as the raw text Rust printed. */
function parts(line: string): Frame {
  const m = /^\{"seq":(-?\d+),"event_type":("(?:[^"\\]|\\.)*"),"payload":(.*)\}$/.exec(line);
  if (!m) throw new Error(`not a transcript line: ${line}`);
  return { id: Number(m[1]), event: JSON.parse(m[2] as string), data: m[3] as string };
}

/** The payload as the route sends it: the run's `attempt` appended, as `stream_events` inserts it. */
function withAttempt(payload: string, attempt = 0): string {
  return payload === "{}"
    ? `{"attempt":${attempt}}`
    : `${payload.slice(0, -1)},"attempt":${attempt}}`;
}

/** A Rust transcript line as the route's frame makes it: what oxyc must print. */
const expectedLine = (line: string) => {
  const f = parts(line);
  return `{"seq":${f.id},"event_type":${JSON.stringify(f.event)},"payload":${withAttempt(f.data)}}`;
};

const serverFrames = (lines: string[]): Frame[] =>
  lines.map((l) => {
    const f = parts(l);
    return { ...f, data: withAttempt(f.data) };
  });

let server: Server;
let base: string;
let requests: Array<{ method: string; url: string; body: string; lastEventId?: string }> = [];
let start: { status: number; body: unknown } = { status: 200, body: { run_id: "run-9" } };
let frames: Frame[] = [];
/** Leave the SSE response open after the frames, as the route does for a suspended run. */
let holdOpen = false;
/** Frames sent after a pause, on the same response. */
let tail: { frames: Frame[]; afterMs: number } = { frames: [], afterMs: 0 };
let reply: { status: number; body: unknown } = { status: 200, body: { ok: true } };
let coordinator: { status: number } = { status: 200 };
const open: ServerResponse[] = [];

const RUNS = [
  {
    run_id: "run-9",
    status: "done",
    question: "Revenue by region?",
    agent_id: "agents/sales.agentic.yml",
    source_type: "analytics",
    created_at: "2026-11-24T10:00:00+00:00"
  },
  {
    run_id: "run-8",
    status: "running",
    question: "Churn last month?",
    agent_id: "agents/sales.agentic.yml",
    source_type: "",
    created_at: "2026-11-24T09:00:00+00:00"
  }
];

const TREE = (id: string) => ({
  root_id: id,
  nodes: [
    {
      ...RUNS.find((r) => r.run_id === id),
      parent_run_id: null,
      answer: "West $1.00",
      error_message: null,
      attempt: 0,
      task_status: id === "run-9" ? "done" : "delegating",
      trigger: "manual",
      updated_at: "2026-11-24T10:01:00+00:00",
      event_log: [
        { seq: 1, event_type: "state_enter", payload: { state: "clarifying" } },
        { seq: 3, event_type: "state_enter", payload: { state: "solving" } },
        { seq: 4, event_type: "delegation_started", payload: { target: "workflow:etl" } },
        ...(id === "run-9" ? [{ seq: 6, event_type: "delegation_completed", payload: {} }] : [])
      ]
    }
  ]
});

beforeEach(() => {
  requests = [];
  start = { status: 200, body: { run_id: "run-9", thread_id: "t-1" } };
  frames = serverFrames(TRANSCRIPT);
  holdOpen = false;
  tail = { frames: [], afterMs: 0 };
  reply = { status: 200, body: { ok: true, resumed: true } };
  coordinator = { status: 200 };
});

function json(res: ServerResponse, value: unknown, status = 200): void {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(value));
}

const writeFrame = (res: ServerResponse, f: Frame) =>
  res.write(`id: ${f.id}\nevent: ${f.event}\ndata: ${f.data}\n\n`);

function sse(res: ServerResponse): void {
  res.writeHead(200, { "content-type": "text/event-stream" });
  res.write(": keep-alive\n\n");
  for (const f of frames) writeFrame(res, f);
  if (tail.frames.length > 0) {
    // Later frames, after the stream has been quiet longer than oxyc waits
    // after a question.
    setTimeout(() => {
      for (const f of tail.frames) writeFrame(res, f);
      res.end();
    }, tail.afterMs);
    return;
  }
  if (holdOpen) open.push(res);
  else res.end();
}

/** The coordinator reads' answer: a status and a JSON body. */
function coordinatorAnswer(url: string): [number, unknown] {
  if (coordinator.status !== 200) {
    return [
      coordinator.status,
      { error: "factory_access_required", code: "factory_access_required" }
    ];
  }
  if (url.startsWith(`${BASE}/coordinator/runs?`)) return [200, { runs: RUNS, total: 2 }];
  if (url === `${BASE}/coordinator/active-runs`) return [200, { runs: [RUNS[1]], total: 1 }];
  const tree = /\/coordinator\/runs\/([^/]+)\/tree$/.exec(url);
  if (tree && RUNS.some((r) => r.run_id === tree[1])) return [200, TREE(tree[1] as string)];
  return [404, "run not found"];
}

/** The fake deployment's routes, first match wins. */
const ROUTES: Array<[string, RegExp, (res: ServerResponse, url: string) => void]> = [
  ["POST", /\/analytics\/runs$/, (res) => json(res, start.body, start.status)],
  ["GET", /\/runs\/run-9\/events$/, (res) => sse(res)],
  ["POST", /\/runs\/[^/]+\/(answer|cancel)$/, (res) => json(res, reply.body, reply.status)],
  [
    "GET",
    /\/analytics\/coordinator\//,
    (res, url) => {
      const [status, value] = coordinatorAnswer(url);
      json(res, value, status);
    }
  ]
];

function route(res: ServerResponse, method: string, url: string): void {
  const hit = ROUTES.find(([m, re]) => m === method && url.startsWith(BASE) && re.test(url));
  if (hit) hit[2](res, url);
  else json(res, {}, 404);
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
      const lastEventId = req.headers["last-event-id"] as string | undefined;
      requests.push({ method, url, body, lastEventId });
      route(res, method, url);
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(() => {
  for (const res of open) res.end();
  server.close();
});

function oxyc(...args: string[]): Promise<{ status: number; stdout: string; stderr: string }> {
  if (!existsSync(BIN)) throw new Error(`${BIN} missing — run \`pnpm build\``);
  const home = mkdtempSync(join(tmpdir(), "oxyc-agentic-"));
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

/** For the cases that wait out the quiet window after a question, plus a spawn. */
const TIMED = { timeout: 15_000 };
const sent = () => requests.map((q) => `${q.method} ${q.url}`);
const lines = (s: string) => s.split("\n").filter(Boolean);
const RUN = ["agentic", "run", "agents/sales.agentic.yml", "-q", "Revenue by region?"];

describe("the --json line", () => {
  it("rebuilds every transcript line byte for byte from the route's frame (attempt aside)", () => {
    for (const line of TRANSCRIPT) {
      const f = parts(line);
      expect(jsonLine({ id: String(f.id), event: f.event, data: f.data })).toBe(line);
    }
  });

  it("keeps serde's number spelling, which a re-serialization would lose", () => {
    const line = TRANSCRIPT.find((l) => l.includes('"query_executed"')) as string;
    const f = parts(line);
    expect(jsonLine({ id: String(f.id), event: f.event, data: f.data })).toContain('["west",1.0]');
    expect(JSON.stringify(JSON.parse(f.data))).not.toContain("1.0]");
  });

  it("ends on agentic-http's analytics terminal events, and fails on two of them", () => {
    for (const e of ["done", "error", "cancelled"]) expect(isTerminalEvent(e)).toBe(true);
    for (const e of ["awaiting_input", "subrun_completed", "load_completed", "recovery_resumed"]) {
      expect(isTerminalEvent(e)).toBe(false);
    }
    expect(isFailureEvent("done")).toBe(false);
    expect(isFailureEvent("error")).toBe(true);
    expect(isFailureEvent("cancelled")).toBe(true);
  });
});

describe("agentic run", () => {
  it("--json prints the Rust command's lines, each payload with the route's attempt, and exits 0", async () => {
    const r = await oxyc(...RUN, "--workspace", WS, "--json");
    expect(r.status, r.stderr).toBe(0);
    expect(lines(r.stdout)).toEqual(TRANSCRIPT.map(expectedLine));
    // Stripped of `attempt`, each line is the Rust one as a value too.
    const stripped = lines(r.stdout).map((l) => {
      const v = JSON.parse(l);
      delete v.payload.attempt;
      return v;
    });
    expect(stripped).toEqual(TRANSCRIPT.map((l) => JSON.parse(l)));
    expect(r.stderr).not.toContain("Run started");
  });

  it("starts the deployed agent by its path, with the question and thinking mode", async () => {
    await oxyc(...RUN, "--workspace", WS, "--thinking-mode", "extended_thinking");
    expect(sent()).toEqual([`POST ${BASE}/runs`, `GET ${BASE}/runs/run-9/events`]);
    expect(JSON.parse(requests[0]?.body ?? "{}")).toEqual({
      agent_id: "agents/sales.agentic.yml",
      question: "Revenue by region?",
      thinking_mode: "extended_thinking"
    });
  });

  it("the fields the oxy-evals skill reads are where it says", async () => {
    const r = await oxyc(...RUN, "--workspace", WS, "--json");
    const events = lines(r.stdout).map((l) => JSON.parse(l));
    const executed = events.find((e) => e.event_type === "query_executed");
    expect(Object.keys(executed)).toEqual(["seq", "event_type", "payload"]);
    expect(executed.payload).toMatchObject({ source: "llm", row_count: 2, error: null });
    expect(executed.payload.query).toContain("SELECT");
    expect(executed.payload.rows).toEqual([
      ["west", 1],
      ["east", 1234.5]
    ]);
    const bounced = events.find((e) => e.event_type === "analytics_validation_failed");
    expect(bounced.payload).toMatchObject({ state: "solving" });
    expect(typeof bounced.payload.reason).toBe("string");
  });

  it("renders the run for a person, as the Rust command did", async () => {
    const r = await oxyc(...RUN, "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stderr).toContain("Run started: run-9");
    expect(r.stdout).toContain("⠋ Clarifying\n");
    expect(r.stdout).toContain("⠋ Solving — revenue by region\n");
    expect(r.stdout).toContain("✓ Solving\n");
    expect(r.stdout).toContain('  → search_catalog({"query":"revenue by region"})\n');
    expect(r.stdout).toContain("West: $1.00; east: $1,234.50 — «ünïcode»");
    expect(r.stdout).toContain("  [query_executed]\n");
    expect(r.stdout).toContain("✓ Done\n");
  });

  it("an error event exits 1 (the Rust command exited 0)", async () => {
    frames = serverFrames([
      TRANSCRIPT[0] as string,
      '{"seq":3,"event_type":"error","payload":{"message":"no such measure"}}'
    ]);
    const r = await oxyc(...RUN, "--workspace", WS);
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stdout).toContain("✗ Error: no such measure");
  });

  it("a cancelled run exits 1", async () => {
    frames = serverFrames(['{"seq":2,"event_type":"cancelled","payload":{"trace_id":"run-9"}}']);
    const r = await oxyc(...RUN, "--workspace", WS, "--json");
    expect(r.status).toBe(ExitCode.FAILURE);
  });

  it("a terminal event the route synthesized from the run row ends it like any other", async () => {
    frames = serverFrames([
      TRANSCRIPT[0] as string,
      '{"seq":2,"event_type":"done","payload":{"answer":"West $1.00","trace_id":"run-9"}}'
    ]);
    const r = await oxyc(...RUN, "--workspace", WS, "--json");
    expect(r.status, r.stderr).toBe(0);
    expect(lines(r.stdout)).toHaveLength(2);
  });

  it(
    "stops at a question the stream leaves open, exits 0 and says how to answer",
    TIMED,
    async () => {
      holdOpen = true;
      frames = serverFrames([
        TRANSCRIPT[0] as string,
        '{"seq":3,"event_type":"awaiting_input","payload":{"questions":[{"prompt":"Which quarter?","suggestions":["Q3"]}]}}'
      ]);
      const r = await oxyc(...RUN, "--workspace", WS, "--json");
      expect(r.status, r.stderr).toBe(0);
      expect(lines(r.stdout).map((l) => JSON.parse(l).event_type)).toEqual([
        "step_start",
        "awaiting_input"
      ]);
      expect(r.stderr).toContain('oxyc agentic answer run-9 --answer "..."');
    }
  );

  // One case per spawn: under the full suite's load a spawn of the binary can
  // take a second, and a loop of six would cross the 5 s test timeout.
  it("stays suspended through what the run writes after its question", TIMED, async () => {
    holdOpen = true;
    frames = serverFrames([
      '{"seq":3,"event_type":"awaiting_input","payload":{"questions":[{"prompt":"Which quarter?","suggestions":[]}]}}',
      '{"seq":4,"event_type":"step_end","payload":{"label":"Clarifying","outcome":"suspended","sub_spec_index":null}}'
    ]);
    const r = await oxyc(...RUN, "--workspace", WS, "--json");
    expect(r.status, r.stderr).toBe(0);
    expect(lines(r.stdout)).toHaveLength(2);
    expect(r.stderr).toContain("oxyc agentic answer run-9");
  });

  it(
    "a suspension that delegates is followed to the end, however long the child runs",
    TIMED,
    async () => {
      frames = serverFrames([
        '{"seq":3,"event_type":"awaiting_input","payload":{"questions":[{"prompt":"run refresh_orders","suggestions":[]}]}}',
        '{"seq":4,"event_type":"delegation_started","payload":{"target":"workflow:refresh_orders"}}'
      ]);
      tail = {
        frames: serverFrames(['{"seq":9,"event_type":"done","payload":{}}']),
        afterMs: SUSPEND_QUIET_MS + 700
      };
      const r = await oxyc(...RUN, "--workspace", WS, "--json");
      expect(r.status, r.stderr).toBe(0);
      expect(lines(r.stdout).map((l) => JSON.parse(l).event_type)).toEqual([
        "awaiting_input",
        "delegation_started",
        "done"
      ]);
      expect(r.stderr).not.toContain("oxyc agentic answer");
    }
  );

  it(
    "an answered question does not end a replay, however long the run then takes",
    TIMED,
    async () => {
      frames = serverFrames([
        '{"seq":3,"event_type":"awaiting_input","payload":{"questions":[]}}',
        '{"seq":5,"event_type":"input_resolved","payload":{"answer":"Q3","trace_id":"run-9"}}'
      ]);
      tail = {
        frames: serverFrames(['{"seq":9,"event_type":"done","payload":{}}']),
        afterMs: SUSPEND_QUIET_MS + 700
      };
      const r = await oxyc("agentic", "events", "run-9", "--json", "--workspace", WS);
      expect(r.status, r.stderr).toBe(0);
      expect(lines(r.stdout).map((l) => JSON.parse(l).event_type)).toEqual([
        "awaiting_input",
        "input_resolved",
        "done"
      ]);
    }
  );

  it.each([
    [400, ExitCode.REQUEST],
    [410, ExitCode.REQUEST],
    [401, ExitCode.AUTH],
    [403, ExitCode.AUTH],
    [404, ExitCode.NOT_FOUND],
    [503, ExitCode.UNAVAILABLE]
  ])("a start refused with %i exits %i", async (status, code) => {
    start = { status, body: { error: "Your project configuration is invalid" } };
    const r = await oxyc(...RUN, "--workspace", WS);
    expect(r.status, r.stderr).toBe(code);
  });

  it.each([
    [
      "an absolute path",
      ["agentic", "run", "/home/me/ws/a.agentic.yml", "-q", "q", "--workspace", WS]
    ],
    ["a bad thinking mode", [...RUN, "--workspace", WS, "--thinking-mode", "deep"]],
    ["no workspace", RUN]
  ])("refuses %s with exit 2, before any request", async (_why, args) => {
    const r = await oxyc(...args);
    expect(r.status, r.stderr).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });
});

describe("agentic events", () => {
  it("replays from --after-seq through Last-Event-ID, in the run's --json lines", async () => {
    const r = await oxyc(
      "agentic",
      "events",
      "run-9",
      "--after-seq",
      "3",
      "--json",
      "--workspace",
      WS
    );
    expect(r.status, r.stderr).toBe(0);
    expect(requests.map((q) => q.lastEventId)).toEqual(["3"]);
    expect(lines(r.stdout)).toEqual(TRANSCRIPT.map(expectedLine));
  });

  it("reports a failed run without failing itself", async () => {
    frames = serverFrames(['{"seq":3,"event_type":"error","payload":{"message":"boom"}}']);
    const r = await oxyc("agentic", "events", "run-9", "--workspace", WS);
    expect(r.status).toBe(0);
    expect(requests[0]?.lastEventId).toBeUndefined();
  });

  it("an unknown run is exit 5", async () => {
    const r = await oxyc("agentic", "events", "nope", "--workspace", WS);
    expect(r.status).toBe(ExitCode.NOT_FOUND);
  });
});

describe("agentic answer and cancel", () => {
  it("answer posts the text to the run", async () => {
    const r = await oxyc("agentic", "answer", "run-9", "--answer", "Q3", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`POST ${BASE}/runs/run-9/answer`]);
    expect(JSON.parse(requests[0]?.body ?? "{}")).toEqual({ answer: "Q3" });
    expect(r.stdout).toContain("Answer submitted for run run-9.");
  });

  it.each([
    [409, ExitCode.REQUEST],
    [410, ExitCode.REQUEST],
    [503, ExitCode.UNAVAILABLE]
  ])("an answer refused with %i exits %i", async (status, code) => {
    reply = { status, body: { error: "not suspended" } };
    const r = await oxyc("agentic", "answer", "run-9", "-a", "Q3", "--workspace", WS);
    expect(r.status, r.stderr).toBe(code);
  });

  it("cancel posts to the run", async () => {
    const r = await oxyc("agentic", "cancel", "run-9", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`POST ${BASE}/runs/run-9/cancel`]);
    expect(r.stdout).toContain("Run run-9 cancelled.");
  });

  it("cancelling an unknown run is exit 5", async () => {
    reply = { status: 404, body: { error: "run not found" } };
    const r = await oxyc("agentic", "cancel", "gone", "--workspace", WS);
    expect(r.status).toBe(ExitCode.NOT_FOUND);
  });
});

describe("agentic list, inspect and status", () => {
  it("list --json keeps the Rust keys", async () => {
    const r = await oxyc("agentic", "list", "--limit", "5", "--json", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`GET ${BASE}/coordinator/runs?limit=5`]);
    const items = JSON.parse(r.stdout);
    expect(items.map((i: object) => Object.keys(i))).toEqual([
      ["run_id", "status", "source_type", "question", "created_at"],
      ["run_id", "status", "source_type", "question", "created_at"]
    ]);
    expect(items[1].source_type).toBeNull();
  });

  it("list refuses a limit the route would clamp", async () => {
    const r = await oxyc("agentic", "list", "--limit", "500", "--workspace", WS);
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });

  it("inspect --json reads the run's tree", async () => {
    const r = await oxyc("agentic", "inspect", "run-9", "--json", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`GET ${BASE}/coordinator/runs/run-9/tree`]);
    expect(JSON.parse(r.stdout)).toMatchObject({
      run_id: "run-9",
      status: "done",
      source_type: "analytics",
      question: "Revenue by region?",
      answer: "West $1.00",
      error_message: null,
      agent_id: "agents/sales.agentic.yml",
      task_status: "done"
    });
  });

  it("inspecting an unknown run is exit 5", async () => {
    const r = await oxyc("agentic", "inspect", "nope", "--workspace", WS);
    expect(r.status).toBe(ExitCode.NOT_FOUND);
  });

  it("a caller the Orchestrator Dashboard refuses is exit 4", async () => {
    coordinator = { status: 403 };
    const r = await oxyc("agentic", "inspect", "run-9", "--workspace", WS);
    expect(r.status).toBe(ExitCode.AUTH);
    expect(r.stderr).toContain("factory_access_required");
  });

  it("status works out each run's stage and delegations, as Rust did", async () => {
    const r = await oxyc("agentic", "status", "--json", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    const rows = JSON.parse(r.stdout);
    expect(rows[0]).toMatchObject({
      run_id: "run-9",
      task_status: "done",
      current_stage: "solving",
      delegations: "1 started, 1 done"
    });
    expect(rows[1]).toMatchObject({
      run_id: "run-8",
      delegations: "1 started, 0 done, active: workflow:etl"
    });
  });

  it("status --active reads the active runs", async () => {
    const r = await oxyc("agentic", "status", "--active", "--json", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(sent()[0]).toBe(`GET ${BASE}/coordinator/active-runs`);
    expect(JSON.parse(r.stdout).map((x: { run_id: string }) => x.run_id)).toEqual(["run-8"]);
  });

  it("status refuses a watch faster than every 2 s, before any request", async () => {
    const r = await oxyc("agentic", "status", "--watch", "1", "--workspace", WS);
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });

  it("status reads the trees a few at a time, in the list's order", async () => {
    let inFlight = 0;
    let peak = 0;
    const order = await mapPool([30, 5, 20, 1, 10, 2, 15], TREE_READS_IN_FLIGHT, async (ms) => {
      inFlight += 1;
      peak = Math.max(peak, inFlight);
      await new Promise((r) => setTimeout(r, ms));
      inFlight -= 1;
      return ms;
    });
    expect(peak).toBe(TREE_READS_IN_FLIGHT);
    expect(order).toEqual([30, 5, 20, 1, 10, 2, 15]);
  });

  it("the stage and delegation helpers match Rust's on an empty log", () => {
    expect(currentStage([])).toBe("—");
    expect(delegationInfo([])).toBe("none");
  });
});
