/**
 * `oxyc experiments`, driven through the real binary against a fake deployment.
 *
 * A LOCAL SERVER, for `assume.test.ts`'s reason: what is under test is the wire
 * contract with the `/{workspace}/experiments*` routes — the paths, the bodies,
 * the hash a write carries, and that nothing irreversible is sent unconfirmed.
 */

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { parse as parseYaml } from "yaml";
import { ExitCode } from "../util/errors.js";

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");
const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

const WS = "e7293e2a-9477-454d-be60-7ddb1ce41faa";
const BASE = `/api/${WS}/experiments`;

const CONFIG = {
  experiment: "promo",
  hypothesis: "A promo lifts orders",
  owner: "ops",
  for_each: "store",
  lever: {
    on: { via: "promo_on" },
    off: { via: "promo_off" },
    read_back: { when_on: "{{x}} > 0", when_off: "{{x}} = 0", grace_days: 2 }
  },
  design: { timezone: "UTC", time_dimension: "orders.order_date" },
  outcome: { target: "orders.total", direction: "increase" }
};

function design(hash: string, frozen = false) {
  return {
    id: "d1",
    content_hash: hash,
    frozen_at: frozen ? "2026-10-01T00:00:00Z" : null,
    from_proposal_id: null,
    created_by: "u1",
    created_at: "2026-10-01T00:00:00Z",
    updated_by: "u1",
    updated_at: "2026-10-01T00:00:00Z",
    config: CONFIG
  };
}

function proposal(id: string, status: string, extra: Record<string, unknown> = {}) {
  return {
    id,
    request_id: "r1",
    status,
    created_at: "2026-10-01T00:00:00Z",
    finished_at: null,
    request: { shape: "waves", wave_sizes: [4, 2], first_switch_on: "2026-11-02" },
    mde: status === "done" ? 12.5 : null,
    mde_unit_day: status === "done" ? 12.5 : null,
    mde_relative: status === "done" ? 0.052 : null,
    alpha: 0.05,
    warnings: [],
    refusal: status === "refused" ? "window coverage below the floor" : null,
    stale: false,
    ...extra
  };
}

const RUN = {
  name: "promo",
  status: "live",
  registration_refusal: null,
  retrospective_ready: false,
  retrospective_refusal: null,
  lever_revisions: [],
  run: {
    state: "live",
    design: "waves",
    periods: null,
    assign_by: null,
    group_membership: null,
    design_id: "d1",
    registered_by: "u1",
    registered_at: "2026-10-01T00:00:00Z",
    retrospective: false,
    next_switch_on: "2026-11-16",
    flips: { pending: 2, sent: 4, confirmed: 3, unconfirmed: 0, failed: 1, cancelled: 0 },
    failed_flips: [{ unit: "store_9", state: "failed", reason: "automation errored" }],
    compliance: null,
    coverage: { last_window: 0.97, floor: 0.9 },
    guardrails: [{ criteria: "refunds < 5%", status: "ok", detail: null, units: [] }],
    guardrail_breach: null,
    abort_reason: null,
    abort_levers: null,
    aborted_on: null,
    units_on: ["store_1"],
    checked_at: "2026-10-14T00:00:00Z",
    problems: [],
    next_check_at: "2026-10-16T00:00:00Z",
    not_advancing: false,
    estimate: { state: "none" }
  }
};

const INTERIM = {
  experiment: "promo",
  run_id: "run1",
  state: "live",
  design_id: "d1",
  running: false,
  requested_at: "2026-10-20T00:00:00Z",
  error: null,
  reading: {
    computed_at: "2026-10-20T00:05:00Z",
    through: "2026-10-19",
    report: {
      label:
        "INTERIM — not the final result: the registered estimator over the days read through 2026-10-19, while the run is still in progress",
      refusal: null,
      headline: { verdict: "significant at α = 0.05", text: "+$3.20 per store-day" },
      assumptions: ["no carryover between periods"]
    },
    effect: {},
    compliance: {},
    series: null,
    alpha: 0.05,
    directional: false
  }
};

let server: Server;
let base: string;
let requests: Array<{ method: string; url: string; body: string }> = [];
/** How many times `GET /proposals` answers with the priced rows still pending. */
let pendingPolls = 0;
let resultStatus = 200;
let resultBody: unknown = {};
let interimStatus = 200;
let interimBody: unknown = {};
/** How many times `GET /interim` answers that a reading is still being computed. */
let interimPolls = 0;

beforeEach(() => {
  requests = [];
  pendingPolls = 0;
  resultStatus = 200;
  resultBody = {};
  interimStatus = 200;
  interimBody = INTERIM;
  interimPolls = 0;
});

function json(res: ServerResponse, value: unknown, status = 200): void {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(value));
}

function route(method: string, url: string, res: ServerResponse) {
  if (method === "GET" && url === BASE) {
    return json(res, [
      { name: "promo", status: "draft", owner: "ops", proposals_count: 2, latest_proposal: null },
      {
        name: "promo_live",
        status: "live",
        owner: "ops",
        proposals_count: 1,
        next_switch_on: "2026-11-16"
      }
    ]);
  }
  if (method === "POST" && url === BASE) return json(res, design("h0"), 201);
  if (method === "GET" && url === `${BASE}/promo/design`) return json(res, design("h1"));
  if (method === "PUT" && url === `${BASE}/promo/design`) return json(res, design("h2"));
  if (method === "POST" && url === `${BASE}/promo/duplicate`) return json(res, design("h3"), 201);
  if (method === "POST" && url === `${BASE}/promo/price`) {
    return json(res, { request_id: "r1", proposal_ids: ["p1", "p2"] }, 202);
  }
  if (method === "GET" && url === `${BASE}/promo/proposals`) {
    if (pendingPolls > 0) {
      pendingPolls -= 1;
      return json(res, [proposal("p1", "pending"), proposal("p2", "pending")]);
    }
    return json(res, [proposal("p1", "done"), proposal("p2", "refused"), proposal("p0", "done")]);
  }
  if (method === "POST" && url === `${BASE}/promo/proposals/p1/use`) return json(res, design("h4"));
  if (
    method === "POST" &&
    (url === `${BASE}/promo/register` || url === `${BASE}/promo/retrospective`)
  ) {
    return json(
      res,
      {
        run_id: "run1",
        design_id: "d1",
        registered_at: "2026-10-15T00:00:00Z",
        frozen_at: "2026-10-15T00:00:00Z"
      },
      201
    );
  }
  if (method === "GET" && url === `${BASE}/promo/run`) return json(res, RUN);
  if (method === "POST" && url === `${BASE}/promo/abort`) return json(res, RUN, 202);
  if (method === "GET" && url === `${BASE}/promo/result`)
    return json(res, resultBody, resultStatus);
  if (url === `${BASE}/promo/interim`) {
    if (method === "POST") return json(res, { ...INTERIM, running: true }, 202);
    if (interimPolls > 0) {
      interimPolls -= 1;
      return json(res, { ...INTERIM, running: true });
    }
    return json(res, interimBody, interimStatus);
  }
  if (method === "POST" && url === `${BASE}/promo/write-back/apply`) {
    return json(res, { branch: "experiments/promo-write-back", file_path: "models/world.yml" });
  }
  if (url.startsWith(`${BASE}/missing`)) {
    res.writeHead(404, { "content-type": "text/plain" });
    res.end("no experiment named `missing`");
    return;
  }
  res.writeHead(404);
  res.end("{}");
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
      route(method, url, res);
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(() => server.close());

function scratch(): string {
  const dir = mkdtempSync(join(tmpdir(), "oxyc-exp-"));
  SCRATCH.push(dir);
  return dir;
}

/** Async for `assume.test.ts`'s reason: `spawnSync` would block the server above. */
function oxyc(...args: string[]): Promise<{ status: number; stdout: string; stderr: string }> {
  if (!existsSync(BIN)) throw new Error(`${BIN} missing — run \`pnpm build\``);
  const home = scratch();
  return new Promise((done) => {
    // stdin is a pipe, not a terminal — which is what an unconfirmed register refuses on.
    const child = spawn(process.execPath, [BIN, ...args, "--target", base, "--workspace", WS], {
      env: {
        ...process.env,
        HOME: home,
        OXY_CREDENTIALS_PATH: join(home, "credentials.json"),
        OXYC_CACHE_DIR: join(home, "cache"),
        OXY_TOKEN: "test-token",
        OXYC_POLL_MS: "10",
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

describe("experiments list", () => {
  it("prints each experiment with its status", async () => {
    const r = await oxyc("experiments", "list");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`GET ${BASE}`]);
    expect(r.stdout).toMatch(/promo[\s|]+draft/);
    expect(r.stdout).toMatch(/promo_live[\s|]+live/);
  });

  it("--json is the server's array", async () => {
    const r = await oxyc("experiments", "list", "--json");
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(r.stdout)).toHaveLength(2);
  });

  it("needs a workspace", async () => {
    const child = await new Promise<{ status: number; stderr: string }>((done) => {
      const home = scratch();
      const c = spawn(process.execPath, [BIN, "experiments", "list", "--target", base], {
        env: { ...process.env, HOME: home, OXY_TOKEN: "t", NO_COLOR: "1" }
      });
      let stderr = "";
      c.stderr.on("data", (d) => {
        stderr += d;
      });
      c.on("close", (status) => done({ status: status ?? -1, stderr }));
    });
    expect(child.status).toBe(ExitCode.USAGE);
    expect(child.stderr).toContain("--workspace");
    expect(requests).toEqual([]);
  });
});

describe("experiments design / create / edit / duplicate", () => {
  it("design prints the config as YAML a later edit reads back", async () => {
    const r = await oxyc("experiments", "design", "promo");
    expect(r.status, r.stderr).toBe(0);
    expect(parseYaml(r.stdout)).toEqual(CONFIG);
  });

  it("create sends the file's config, YAML or JSON", async () => {
    const dir = scratch();
    const file = join(dir, "promo.experiment.yml");
    writeFileSync(file, "experiment: promo\nhypothesis: lifts\nowner: ops\n");
    const r = await oxyc("experiments", "create", "--file", file);
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`POST ${BASE}`]);
    expect(JSON.parse(requests[0]?.body ?? "")).toEqual({
      config: { experiment: "promo", hypothesis: "lifts", owner: "ops" }
    });
  });

  it("edit carries the hash it read, so an edit made since is a 409, not a silent overwrite", async () => {
    const dir = scratch();
    const file = join(dir, "promo.json");
    writeFileSync(file, JSON.stringify(CONFIG));
    const r = await oxyc("experiments", "edit", "promo", "--file", file);
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`GET ${BASE}/promo/design`, `PUT ${BASE}/promo/design`]);
    expect(JSON.parse(requests[1]?.body ?? "")).toEqual({ config: CONFIG, base_hash: "h1" });
  });

  it("edit --base-hash sends the caller's hash without reading", async () => {
    const dir = scratch();
    const file = join(dir, "promo.json");
    writeFileSync(file, JSON.stringify(CONFIG));
    const r = await oxyc("experiments", "edit", "promo", "--file", file, "--base-hash", "hX");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`PUT ${BASE}/promo/design`]);
    expect(JSON.parse(requests[0]?.body ?? "").base_hash).toBe("hX");
  });

  it("duplicate names the new experiment", async () => {
    const r = await oxyc("experiments", "duplicate", "promo", "promo_v2");
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(requests[0]?.body ?? "")).toEqual({ name: "promo_v2" });
  });

  it("a 404 is NOT_FOUND with the server's words", async () => {
    const r = await oxyc("experiments", "design", "missing");
    expect(r.status).toBe(ExitCode.NOT_FOUND);
    expect(r.stderr).toContain("no experiment named `missing`");
  });
});

describe("experiments price / proposals / use", () => {
  it("sends every --candidate, YAML or JSON, in order", async () => {
    const r = await oxyc(
      "experiments",
      "price",
      "promo",
      "--candidate",
      "{shape: waves, wave_sizes: [4, 2], first_switch_on: 2026-11-02, spacing_days: 14}",
      "--candidate",
      '{"shape":"switchback","first_period_on":"2026-11-02","period_days":7,"pairs":6}'
    );
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(requests[0]?.body ?? "")).toEqual({
      candidates: [
        { shape: "waves", wave_sizes: [4, 2], first_switch_on: "2026-11-02", spacing_days: 14 },
        { shape: "switchback", first_period_on: "2026-11-02", period_days: 7, pairs: 6 }
      ]
    });
    expect(r.stdout).toContain("p1");
  });

  it("refuses with no candidate before sending anything", async () => {
    const r = await oxyc("experiments", "price", "promo");
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });

  it("--wait polls until this request's proposals settle, then shows only them", async () => {
    pendingPolls = 2;
    const r = await oxyc(
      "experiments",
      "price",
      "promo",
      "--candidate",
      "{shape: waves, wave_sizes: [4], first_switch_on: 2026-11-02}",
      "--wait"
    );
    expect(r.status, r.stderr).toBe(0);
    expect(sent().filter((s) => s.endsWith("/proposals"))).toHaveLength(3);
    expect(r.stdout).toMatch(/p1[\s|]+done/);
    expect(r.stdout).toMatch(/p2[\s|]+refused/);
    expect(r.stdout).toContain("window coverage below the floor");
    expect(r.stdout).not.toContain("p0");
  });

  it("use adopts the proposal onto the draft", async () => {
    const r = await oxyc("experiments", "use", "promo", "p1");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`POST ${BASE}/promo/proposals/p1/use`]);
  });
});

describe("experiments register / retrospective", () => {
  it("refuses without --yes when nobody can be asked, and sends nothing irreversible", async () => {
    const r = await oxyc("experiments", "register", "promo");
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(sent()).not.toContain(`POST ${BASE}/promo/register`);
  });

  it("registers the draft as it was read: its hash goes with the request", async () => {
    const r = await oxyc("experiments", "register", "promo", "--yes");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`GET ${BASE}/promo/design`, `POST ${BASE}/promo/register`]);
    expect(JSON.parse(requests[1]?.body ?? "")).toEqual({ content_hash: "h1" });
    expect(r.stdout).toContain("run1");
  });

  it("retrospective takes the same confirmation and hash", async () => {
    const r = await oxyc("experiments", "retrospective", "promo", "--yes");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`GET ${BASE}/promo/design`, `POST ${BASE}/promo/retrospective`]);
    expect(JSON.parse(requests[1]?.body ?? "")).toEqual({ content_hash: "h1" });
  });
});

describe("experiments status / abort", () => {
  it("status shows the run's state, flips and guardrails", async () => {
    const r = await oxyc("experiments", "status", "promo");
    expect(r.status, r.stderr).toBe(0);
    for (const part of [
      "live",
      "2026-11-16",
      "confirmed 3",
      "failed 1",
      "store_9",
      "refunds < 5%"
    ]) {
      expect(r.stdout).toContain(part);
    }
  });

  it("abort needs a reason", async () => {
    const r = await oxyc("experiments", "abort", "promo", "--yes");
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });

  it("abort refuses a levers value the server would not take", async () => {
    const r = await oxyc(
      "experiments",
      "abort",
      "promo",
      "--reason",
      "x",
      "--levers",
      "on",
      "--yes"
    );
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });

  it("abort refuses without --yes when nobody can be asked", async () => {
    const r = await oxyc("experiments", "abort", "promo", "--reason", "guardrail");
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(requests).toEqual([]);
  });

  it("abort sends the reason and the levers choice", async () => {
    const r = await oxyc(
      "experiments",
      "abort",
      "promo",
      "--reason",
      "refunds spiked",
      "--levers",
      "keep",
      "--yes"
    );
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`POST ${BASE}/promo/abort`]);
    expect(JSON.parse(requests[0]?.body ?? "")).toEqual({
      reason: "refunds spiked",
      levers: "keep"
    });
  });
});

describe("experiments result / write-back", () => {
  it("a sealed result is a state, not a failure", async () => {
    resultStatus = 409;
    resultBody = { sealed: true, state: "live" };
    const r = await oxyc("experiments", "result", "promo");
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain("sealed");
    expect(r.stdout).toContain("live");
  });

  it("a finished run whose report is not written yet says so", async () => {
    resultStatus = 409;
    resultBody = { sealed: false, pending: true, state: "reported" };
    const r = await oxyc("experiments", "result", "promo");
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain("being prepared");
  });

  it("a report shows its label, verdict and every assumption", async () => {
    resultBody = {
      experiment: "promo",
      state: "reported",
      report: {
        label: "Registered result",
        refusal: null,
        headline: { verdict: "significant at α = 0.05", text: "+$4.10 per store-day" },
        assumptions: ["no carryover between periods"]
      },
      write_back: { available: true, reason: null, edit: null }
    };
    const r = await oxyc("experiments", "result", "promo");
    expect(r.status, r.stderr).toBe(0);
    for (const part of [
      "Registered result",
      "significant at α = 0.05",
      "+$4.10 per store-day",
      "no carryover between periods",
      "write-back"
    ]) {
      expect(r.stdout).toContain(part);
    }
  });

  it("write-back reports the branch it opened", async () => {
    const r = await oxyc("experiments", "write-back", "promo");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`POST ${BASE}/promo/write-back/apply`]);
    expect(r.stdout).toContain("experiments/promo-write-back");
  });
});

describe("experiments interim", () => {
  it("reads the latest interim: labelled not final, read through a date, every assumption", async () => {
    const r = await oxyc("experiments", "interim", "promo");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`GET ${BASE}/promo/interim`]);
    for (const part of [
      "INTERIM — not the final result",
      "2026-10-19",
      "+$3.20 per store-day",
      "no carryover between periods"
    ]) {
      expect(r.stdout).toContain(part);
    }
    expect(r.stdout).not.toContain("write-back");
  });

  it("--json is the server's body", async () => {
    const r = await oxyc("experiments", "interim", "promo", "--json");
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(r.stdout)).toEqual(INTERIM);
  });

  it("no reading yet says when the first one comes", async () => {
    interimBody = { ...INTERIM, reading: null };
    const r = await oxyc("experiments", "interim", "promo");
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain("no interim yet");
  });

  it("a failed attempt shows its reason and keeps the previous reading", async () => {
    interimBody = { ...INTERIM, error: "the warehouse timed out" };
    const r = await oxyc("experiments", "interim", "promo");
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain("last attempt");
    expect(r.stdout).toContain("the warehouse timed out");
    expect(r.stdout).toContain("+$3.20 per store-day");
  });

  it("a run not in progress is a state, not a failure, and points at the report", async () => {
    interimStatus = 409;
    interimBody = {
      in_progress: false,
      state: "reported",
      error: "an interim reads only a live or closing run"
    };
    const r = await oxyc("experiments", "interim", "promo");
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain("reported");
    expect(r.stdout).toContain("oxyc experiments result promo");
  });

  it("--refresh asks for a fresh reading, and --wait polls until it is computed", async () => {
    interimPolls = 2;
    const r = await oxyc("experiments", "interim", "promo", "--refresh", "--wait");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([
      `POST ${BASE}/promo/interim`,
      `GET ${BASE}/promo/interim`,
      `GET ${BASE}/promo/interim`,
      `GET ${BASE}/promo/interim`
    ]);
    expect(r.stdout).toContain("+$3.20 per store-day");
  });

  it("a poll that fails after --refresh names the GET that failed, not the POST", async () => {
    interimPolls = 1;
    interimStatus = 500;
    interimBody = {};
    const r = await oxyc("experiments", "interim", "promo", "--refresh", "--wait");
    expect(r.status).not.toBe(0);
    expect(r.stderr).toContain(`GET ${BASE}/promo/interim failed (500)`);
    expect(r.stderr).not.toContain("POST");
  });

  it("--refresh without --wait says a reading is being computed", async () => {
    const r = await oxyc("experiments", "interim", "promo", "--refresh");
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`POST ${BASE}/promo/interim`]);
    expect(r.stdout).toContain("being computed");
  });

  it("a sealed result points at the interim", async () => {
    resultStatus = 409;
    resultBody = { sealed: true, state: "live" };
    const r = await oxyc("experiments", "result", "promo");
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain("oxyc experiments interim promo");
  });
});

describe("experiments at hour grain", () => {
  it("a candidate's local whole hour reaches the server as written", async () => {
    const r = await oxyc(
      "experiments",
      "price",
      "promo",
      "--candidate",
      "{shape: switchback, first_period_on: 2026-10-26T14:00, period_days: 4, pairs: 6}"
    );
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(requests[0]?.body ?? "").candidates[0].first_period_on).toBe(
      "2026-10-26T14:00"
    );
  });
});
