/**
 * `oxyc run`, driven through the real binary against a fake deployment
 * (`looker.test.ts`'s harness): the wire contract with the SQL and automation
 * routes, the output a caller reads, and the exit codes a script branches on.
 *
 * The first block is the Rust `oxy run` integration tests, ported: those tests
 * spawned `oxy run` over `tests/fixtures/oxy_example`, and run here over the
 * same fixture files, so the SQL that reaches the route is the fixture's own.
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
import { classify, parseVariablePairs, renderSql, renderTable, workspaceRef } from "./run.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const BIN = resolve(HERE, "..", "..", "dist", "main.mjs");
/** `oxy_test_utils::oxy_example_fixture_dir()`: the checkout the Rust tests ran in. */
const FIXTURE = resolve(HERE, "..", "..", "..", "..", "tests", "fixtures", "oxy_example");
const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

const WS = "11111111-2222-3333-4444-555555555555";
const API = `/api/${WS}`;
const RUN_ID = "0b6c9f0e-run";
const TABLE_VALUES = "procedures/table_values.automation.yml";
const B64 = Buffer.from(TABLE_VALUES).toString("base64url");

interface Seen {
  method: string;
  url: string;
  body: unknown;
}

let server: Server;
let base: string;
let seen: Seen[] = [];
let databases: Array<{ name: string }>;
let sqlAnswer: { status: number; body: unknown };
let fileAnswer: { status: number; body: unknown };
let snapshots: unknown[];

beforeEach(() => {
  seen = [];
  databases = [{ name: "primary_database" }, { name: "local" }];
  sqlAnswer = { status: 200, body: [["intervals"], ["monthly"], ["weekly"]] };
  fileAnswer = {
    status: 200,
    body: { path: TABLE_VALUES, content: readFileSync(join(FIXTURE, TABLE_VALUES), "utf8") }
  };
  snapshots = [snapshot("done")];
});

function snapshot(status: string, error_message: string | null = null): unknown {
  return {
    run_id: RUN_ID,
    workflow_ref: TABLE_VALUES,
    status,
    error_message,
    answer: null,
    current_step: 2,
    total_steps: 2,
    results: {
      // Deliberately out of the fixture's task order (intervals, schedules,
      // formatter): the output must follow the definition, as `oxy run`'s did.
      formatter: "weekly report",
      schedules: [{ report: "weekly" }],
      intervals: [["intervals"], ["monthly"], ["weekly"]]
    },
    step_hashes: {},
    retry_from_run_id: null,
    cache_enabled: false
  };
}

function json(res: ServerResponse, value: unknown, status = 200): void {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(typeof value === "string" ? value : JSON.stringify(value));
}

/** The fake deployment's routes: `METHOD path` → the answer, read per request. */
const ROUTES: Record<string, () => { status: number; body: unknown }> = {
  [`GET ${API}/databases`]: () => ({ status: 200, body: databases }),
  [`POST ${API}/sql/query`]: () => sqlAnswer,
  [`GET ${API}/agentic-automations/files/${B64}`]: () => fileAnswer,
  [`POST ${API}/agentic-automations/runs`]: () => ({ status: 200, body: { run_id: RUN_ID } }),
  [`GET ${API}/agentic-automations/runs/${RUN_ID}`]: () => {
    const next = snapshots.length > 1 ? snapshots.shift() : snapshots[0];
    const fail = (next as { fail?: number }).fail;
    return fail ? { status: fail, body: "unavailable" } : { status: 200, body: next };
  }
};

beforeAll(async () => {
  server = createServer((req, res) => {
    let raw = "";
    req.on("data", (d) => {
      raw += d;
    });
    req.on("end", () => {
      const key = `${req.method ?? ""} ${req.url ?? ""}`;
      const [method = "", url = ""] = key.split(" ");
      seen.push({ method, url, body: raw ? JSON.parse(raw) : undefined });
      const answer = ROUTES[key]?.() ?? { status: 404, body: "not found" };
      json(res, answer.body, answer.status);
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(() => server.close());

/** Async for `oltp.test.ts`'s reason: `spawnSync` would block the server above. */
function oxyc(...args: string[]): Promise<{ status: number; stdout: string; stderr: string }> {
  if (!existsSync(BIN)) throw new Error(`${BIN} missing — run \`pnpm build\``);
  const home = mkdtempSync(join(tmpdir(), "oxyc-run-"));
  SCRATCH.push(home);
  return new Promise((done) => {
    const child = spawn(process.execPath, [BIN, ...args, "--target", base], {
      cwd: FIXTURE,
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

const sent = () => seen.map((q) => `${q.method} ${q.url}`);
const sqlBody = () =>
  seen.find((q) => q.url === `${API}/sql/query`)?.body as Record<string, unknown>;

describe("ported from the Rust `oxy run` integration tests", () => {
  it("run_failed_if_file_not_exist: a missing SQL file exits 5 and says so, sending nothing", async () => {
    const r = await oxyc("run", "test.sql", "--workspace", WS);
    expect(r.status).toBe(ExitCode.NOT_FOUND);
    expect(r.stderr).toContain("File not found");
    expect(sent()).toEqual([]);
  });

  it("run_example_sql_file_ok: --database sends the fixture's SQL to that database", async () => {
    const r = await oxyc(
      "run",
      "data/example_intervals.sql",
      "--database",
      "primary_database",
      "--workspace",
      WS
    );
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`GET ${API}/databases`, `POST ${API}/sql/query`]);
    expect(sqlBody()).toEqual({
      sql: readFileSync(join(FIXTURE, "data/example_intervals.sql"), "utf8"),
      database: "primary_database"
    });
    expect(r.stdout).toContain("weekly");
  });

  it("run_sql_file_ok_if_database_not_provided: the workspace's only database is used", async () => {
    databases = [{ name: "primary_database" }];
    const r = await oxyc("run", "data/example_intervals.sql", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(sqlBody().database).toBe("primary_database");
  });

  it("…but with several databases and no --database it exits 2 and lists them", async () => {
    const r = await oxyc("run", "data/example_intervals.sql", "--workspace", WS);
    expect(r.status).toBe(ExitCode.USAGE);
    expect(r.stderr).toContain("Database is required");
    expect(r.stderr).toContain("primary_database, local");
    expect(sent()).toEqual([`GET ${API}/databases`]);
  });

  it("run_sql_file_failed_if_database_not_exist: exit 5, nothing run", async () => {
    const r = await oxyc(
      "run",
      "data/example_intervals.sql",
      "--database",
      "test",
      "--workspace",
      WS
    );
    expect(r.status).toBe(ExitCode.NOT_FOUND);
    expect(r.stderr).toContain("Database 'test' not found in config");
    expect(sent()).toEqual([`GET ${API}/databases`]);
  });

  it("run_sql_file_with_variables_ok: -v renders the template before it is sent", async () => {
    sqlAnswer = { status: 200, body: [["testalias"], ["1"]] };
    const r = await oxyc(
      "run",
      "data/example_weekly_rejected.sql",
      "--database",
      "primary_database",
      "-v",
      "variable_a=1",
      "variable_b=testalias",
      "variable_c=*",
      "--workspace",
      WS
    );
    expect(r.status, r.stderr).toBe(0);
    expect(sqlBody().sql).toBe(
      "WITH responses AS (\n    SELECT 1 AS testalias\n)\nSELECT\n    *\nFROM\n    responses;\n"
    );
    expect(r.stdout).toContain("testalias");
  });

  it("run_example_automation_ok: ✓ <file>, then each task's result in task order", async () => {
    const r = await oxyc("run", TABLE_VALUES, "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([
      `GET ${API}/agentic-automations/files/${B64}`,
      `POST ${API}/agentic-automations/runs`,
      `GET ${API}/agentic-automations/runs/${RUN_ID}`
    ]);
    expect(seen[1]?.body).toEqual({ workflow_ref: TABLE_VALUES });
    expect(r.stdout).toContain("✓ table_values.automation.yml");
    expect(r.stdout).toContain("weekly");
    const at = (task: string) => r.stdout.indexOf(`\n  ${task}\n`);
    expect(at("intervals")).toBeGreaterThan(0);
    expect(at("intervals")).toBeLessThan(at("schedules"));
    expect(at("schedules")).toBeLessThan(at("formatter"));
  });
});

describe("oxyc run — SQL", () => {
  it("--json prints the route's rows as is, header row first", async () => {
    const r = await oxyc(
      "run",
      "data/example_intervals.sql",
      "--database",
      "local",
      "--json",
      "--workspace",
      WS
    );
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(r.stdout)).toEqual([["intervals"], ["monthly"], ["weekly"]]);
  });

  it("a failed query exits 6 with the server's message", async () => {
    sqlAnswer = { status: 400, body: { message: 'column "nope" does not exist', code: "42703" } };
    const r = await oxyc(
      "run",
      "data/example_intervals.sql",
      "--database",
      "local",
      "--workspace",
      WS
    );
    expect(r.status).toBe(ExitCode.REQUEST);
    expect(r.stderr).toContain('column "nope" does not exist');
  });

  it("a warehouse that is down exits 7", async () => {
    sqlAnswer = { status: 502, body: { message: "connection error: refused" } };
    const r = await oxyc(
      "run",
      "data/example_intervals.sql",
      "--database",
      "local",
      "--workspace",
      WS
    );
    expect(r.status).toBe(ExitCode.UNAVAILABLE);
  });

  it("an unauthenticated call exits 4", async () => {
    sqlAnswer = { status: 401, body: "" };
    const r = await oxyc(
      "run",
      "data/example_intervals.sql",
      "--database",
      "local",
      "--workspace",
      WS
    );
    expect(r.status).toBe(ExitCode.AUTH);
  });

  it("a template oxyc cannot render exits 2 and sends nothing", async () => {
    const r = await oxyc(
      "run",
      "data/example_weekly_rejected.sql",
      "--database",
      "local",
      "-v",
      "variable_a=1",
      "--workspace",
      WS
    );
    expect(r.status).toBe(ExitCode.USAGE);
    expect(r.stderr).toContain("no value for {{ variable_b }}");
    expect(sent()).toEqual([]);
  });

  it("warns at the route's 10,000-row cap", async () => {
    sqlAnswer = {
      status: 200,
      body: [["n"], ...Array.from({ length: 10_000 }, (_, i) => [String(i)])]
    };
    const r = await oxyc(
      "run",
      "data/example_intervals.sql",
      "--database",
      "local",
      "--workspace",
      WS
    );
    expect(r.status, r.stderr).toBe(0);
    expect(r.stderr).toContain("10000-row cap");
  });
});

describe("oxyc run — automations", () => {
  it("-v becomes the run's variables", async () => {
    const r = await oxyc("run", TABLE_VALUES, "-v", "region=EU", "n=3", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(seen[1]?.body).toEqual({
      workflow_ref: TABLE_VALUES,
      variables: { region: "EU", n: "3" }
    });
  });

  it("repeated -v accumulate, as the Rust stub prints them", async () => {
    const r = await oxyc("run", TABLE_VALUES, "-v", "a=1", "-v", "b=my value", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(seen[1]?.body).toEqual({
      workflow_ref: TABLE_VALUES,
      variables: { a: "1", b: "my value" }
    });
  });

  it("a ./path resolves against the checkout root", async () => {
    const r = await oxyc("run", `./${TABLE_VALUES}`, "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(seen[1]?.body).toEqual({ workflow_ref: TABLE_VALUES });
  });

  it("waits through running, and --json prints the final snapshot", async () => {
    snapshots = [snapshot("running"), snapshot("done")];
    const r = await oxyc("run", TABLE_VALUES, "--json", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    const body = JSON.parse(r.stdout) as { status: string; run_id: string };
    expect([body.status, body.run_id]).toEqual(["done", RUN_ID]);
    expect(sent().filter((s) => s.endsWith(RUN_ID))).toHaveLength(2);
  });

  it.each(["failed", "cancelled", "timed_out"])("a run that ends %s exits 1", async (status) => {
    snapshots = [snapshot(status, "step schedules: boom")];
    const r = await oxyc("run", TABLE_VALUES, "--workspace", WS);
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stderr).toContain(`${RUN_ID} ${status}`);
  });

  it("an automation the deployment does not have exits 5 and starts nothing", async () => {
    fileAnswer = { status: 404, body: "read file: not in the revision" };
    const r = await oxyc("run", TABLE_VALUES, "--workspace", WS);
    expect(r.status).toBe(ExitCode.NOT_FOUND);
    expect(sent()).toEqual([`GET ${API}/agentic-automations/files/${B64}`]);
  });

  it("a workspace that is not compiled yet exits 7", async () => {
    fileAnswer = { status: 503, body: "read file: not compiled" };
    const r = await oxyc("run", TABLE_VALUES, "--workspace", WS);
    expect(r.status).toBe(ExitCode.UNAVAILABLE);
  });

  it("--detach prints the run id and does not wait", async () => {
    const r = await oxyc("run", TABLE_VALUES, "--detach", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout.trim()).toBe(RUN_ID);
    expect(sent()).toHaveLength(2);
  });

  it("--timeout runs out: exit 1, not 7 — re-running would start a second run", async () => {
    snapshots = [snapshot("running")];
    const r = await oxyc("run", TABLE_VALUES, "--timeout", "0.1", "--workspace", WS);
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stderr).toContain("did not finish within 0.1s");
    expect(r.stderr).toContain(RUN_ID);
  });

  it("a status read that fails with a 5xx is retried, not exit 7", async () => {
    snapshots = [{ fail: 503 }, snapshot("done")];
    const r = await oxyc("run", TABLE_VALUES, "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain("✓ table_values.automation.yml");
    expect(sent().filter((s) => s.endsWith(RUN_ID))).toHaveLength(2);
  });
});

describe("oxyc run — arguments", () => {
  it("refuses a YAML file that is not an automation, with oxy run's message", async () => {
    const r = await oxyc("run", "config.yml", "--workspace", WS);
    expect(r.status).toBe(ExitCode.USAGE);
    expect(r.stderr).toContain("Must be either *.automation.yml or *.procedure.yml");
  });

  it("refuses any other extension", () => {
    expect(() => classify("x.csv")).toThrow(/Invalid file extension/);
    expect(classify("a/b.procedure.yaml")).toBe("automation");
    expect(classify("a/b.sql")).toBe("sql");
  });

  it("refuses a -v pair with no =", async () => {
    const r = await oxyc("run", "data/example_intervals.sql", "-v", "oops", "--workspace", WS);
    expect(r.status).toBe(ExitCode.USAGE);
    expect(r.stderr).toContain("Must be in the form of VAR=VALUE");
  });

  it("--dry-run is gone: the query route has no dry run", async () => {
    const r = await oxyc("run", "data/example_intervals.sql", "--dry-run", "--workspace", WS);
    expect(r.status).toBe(ExitCode.USAGE);
    expect(sent()).toEqual([]);
  });

  it("parses pairs, keeping = in values", () => {
    expect(parseVariablePairs(["a=1", "b=x=y", "c="])).toEqual({ a: "1", b: "x=y", c: "" });
  });

  it("renders {{ name }} with any spacing and refuses everything else", () => {
    expect(renderSql("SELECT {{a}}, {{  b  }}", { a: "1", b: "2" })).toBe("SELECT 1, 2");
    expect(() => renderSql("{% if a %}1{% endif %}", { a: "1" })).toThrow(/Jinja blocks/);
    expect(() => renderSql("{{ a | upper }}", { a: "1" })).toThrow(/only \{\{ name \}\}/);
  });

  it("a ref outside a checkout stays as typed, minus ./", () => {
    const dir = mkdtempSync(join(tmpdir(), "oxyc-run-ref-"));
    SCRATCH.push(dir);
    expect(workspaceRef("./procedures/x.automation.yml", dir)).toBe("procedures/x.automation.yml");
    expect(workspaceRef(join(FIXTURE, TABLE_VALUES), dir)).toBe(TABLE_VALUES);
  });

  it("renders rows as an aligned table", () => {
    expect(
      renderTable([
        ["id", "name"],
        ["1", "ada"]
      ])
    ).toBe("id  name\n--  ----\n1   ada");
    expect(renderTable([])).toBe("(no rows)");
  });
});
