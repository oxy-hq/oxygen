/**
 * `oxyc oltp`, driven through the real binary against a fake deployment.
 *
 * A LOCAL SERVER, for `assume.test.ts`'s reason: what is under test is the wire
 * contract with the admin OLTP routes — the paths, the body `provision` sends,
 * and that nothing is sent before the writers and the confirmation are settled.
 */

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { ExitCode } from "../util/errors.js";
import { parseWriter } from "./oltp.js";

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");
const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

const ORG_ID = "11111111-2222-3333-4444-555555555555";

/** `ConnectionInfoResponse` for a provisioned org. */
function store(schemas: Array<{ schema: string; kind: string; writer: string }>) {
  return {
    is_provisioned: true,
    host: "ep-x.neon.tech",
    database: "oxy_org_acme",
    provider: "neon",
    project_name: "oxy-org-11111111",
    console_url: "https://console.neon.tech/app/projects/x",
    region: "aws-us-east-2",
    status: "active",
    analyst_role: "oxy_analyst_ro",
    analyst_ready: true,
    platform_schema_version: 3,
    expected_platform_schema_version: 3,
    schemas: schemas.map((s) => ({
      schema: s.schema,
      kind: s.kind,
      writer_name: s.writer,
      role: `${s.schema}_rw`,
      analytics_visible: s.kind === "pipeline"
    }))
  };
}

const UNPROVISIONED = {
  is_provisioned: false,
  host: "",
  database: "",
  provider: "",
  project_name: "",
  console_url: null,
  region: "",
  status: "",
  analyst_role: "oxy_analyst_ro",
  analyst_ready: false,
  platform_schema_version: 0,
  expected_platform_schema_version: 3,
  schemas: []
};

const TENANTS = [
  {
    org_id: ORG_ID,
    org_name: "Acme",
    database: "oxy_org_acme",
    host: "ep-x.neon.tech",
    provider: "neon",
    region: "aws-us-east-2",
    status: "active",
    schemas: [
      { schema: "app_store_ops", kind: "app", analytics_visible: false },
      { schema: "raw_toast", kind: "pipeline", analytics_visible: true }
    ],
    analyst_ready: true,
    platform_drift: false
  },
  {
    org_id: "22222222-2222-3333-4444-555555555555",
    org_name: "Beta",
    database: "",
    host: "",
    provider: "",
    region: "",
    status: "none",
    schemas: [],
    analyst_ready: false,
    platform_drift: false
  }
];

let server: Server;
let base: string;
/** Every request the fake deployment received, in order. */
let requests: Array<{ method: string; url: string; body: string }> = [];
let provisioned = true;
let provisionStatus = 200;
let provisionBody = "";

beforeEach(() => {
  requests = [];
  provisioned = true;
  provisionStatus = 200;
  provisionBody = "";
});

function json(res: import("node:http").ServerResponse, value: unknown): void {
  res.writeHead(200, { "content-type": "application/json" });
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
      if (url.startsWith("/api/orgs")) return json(res, [{ slug: "acme", id: ORG_ID }]);
      if (method === "GET" && url === "/api/admin/oltp") return json(res, TENANTS);
      if (method === "GET" && url === `/api/admin/orgs/${ORG_ID}/oltp`) {
        return json(
          res,
          provisioned
            ? store([{ schema: "app_store_ops", kind: "app", writer: "store_ops" }])
            : UNPROVISIONED
        );
      }
      if (method === "POST" && url === `/api/admin/orgs/${ORG_ID}/oltp/provision`) {
        if (provisionStatus !== 200) {
          res.writeHead(provisionStatus, { "content-type": "text/plain" });
          return res.end(provisionBody);
        }
        return json(
          res,
          store([
            { schema: "app_store_ops", kind: "app", writer: "store_ops" },
            { schema: "raw_toast", kind: "pipeline", writer: "toast" }
          ])
        );
      }
      res.writeHead(404);
      res.end("{}");
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(() => server.close());

/** Async for `assume.test.ts`'s reason: `spawnSync` would block the server above. */
function oxyc(...args: string[]): Promise<{ status: number; stdout: string; stderr: string }> {
  if (!existsSync(BIN)) throw new Error(`${BIN} missing — run \`pnpm build\``);
  const home = mkdtempSync(join(tmpdir(), "oxyc-oltp-"));
  SCRATCH.push(home);
  return new Promise((done) => {
    // stdin is a pipe, not a terminal — which is what `provision` without
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

describe("writer specs", () => {
  it("derives an app's writer from its slug, as the platform does", () => {
    expect(parseWriter("app:store-ops")).toEqual({
      spec: "app:store_ops",
      typed: "app:store-ops",
      schema: "app_store_ops"
    });
    // The server's own messages print the derived name; it is accepted as-is.
    expect(parseWriter("app:store_ops").spec).toBe("app:store_ops");
    expect(parseWriter("pipeline:toast")).toMatchObject({
      spec: "pipeline:toast",
      schema: "raw_toast"
    });
  });

  it.each(["bogus:x", "store-ops", "app:", "app:Store-Ops", "app:my_app-x", "pipeline:toast-pos"])(
    "refuses %j",
    (typed) => {
      expect(() => parseWriter(typed)).toThrow();
    }
  );
});

describe("oltp status", () => {
  it("lists every org, with a database or without", async () => {
    const r = await oxyc("oltp", "status");
    expect(r.status, r.stderr).toBe(0);
    expect(requests.map((q) => `${q.method} ${q.url}`)).toEqual(["GET /api/admin/oltp"]);
    for (const part of [
      "Acme",
      "oxy_org_acme",
      "app_store_ops",
      "raw_toast (analytics)",
      "Beta",
      "none"
    ]) {
      expect(r.stdout).toContain(part);
    }
    expect(r.stderr).toMatch(/1 of 2 org\(s\) have an OLTP database/);
  });

  it("shows one org's store and writers, resolving a slug", async () => {
    const r = await oxyc("oltp", "status", "--org", "acme");
    expect(r.status, r.stderr).toBe(0);
    expect(requests.at(-1)?.url).toBe(`/api/admin/orgs/${ORG_ID}/oltp`);
    expect(r.stdout).toMatch(/database\s+oxy_org_acme/);
    expect(r.stdout).toMatch(/analyst\s+oxy_analyst_ro \(ready\)/);
    for (const part of ["app_store_ops", "store_ops", "app_store_ops_rw", "hidden"]) {
      expect(r.stdout).toContain(part);
    }
  });

  it("emits the server's document with --json", async () => {
    const r = await oxyc("oltp", "status", "--org", ORG_ID, "--json");
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(r.stdout)).toMatchObject({ is_provisioned: true, database: "oxy_org_acme" });
  });

  it("says when an org has no database, and how to make one", async () => {
    provisioned = false;
    const r = await oxyc("oltp", "status", "--org", "acme");
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain("no OLTP database for org acme");
    expect(r.stderr).toContain("oxyc oltp provision --org acme --writer app:<slug>");
  });
});

describe("oltp provision", () => {
  it("prints the plan, then posts the derived writers once each", async () => {
    const r = await oxyc(
      "oltp",
      "provision",
      "--org",
      "acme",
      "--writer",
      "app:store-ops",
      "--writer",
      "pipeline:toast",
      "--writer",
      "app:store_ops",
      "--yes"
    );
    expect(r.status, r.stderr).toBe(0);
    const post = requests.find((q) => q.method === "POST");
    expect(post?.url).toBe(`/api/admin/orgs/${ORG_ID}/oltp/provision`);
    expect(JSON.parse(post?.body ?? "{}")).toEqual({
      writers: ["app:store_ops", "pipeline:toast"]
    });

    expect(r.stderr).toContain(`for org acme (${ORG_ID})`);
    expect(r.stderr).toContain("database  oxy_org_acme on ep-x.neon.tech — exists, reconciled");
    expect(r.stderr).toContain(
      "writer    app:store_ops → app_store_ops (from app:store-ops) — exists, reconciled"
    );
    expect(r.stderr).toContain(
      "writer    pipeline:toast → raw_toast — new schema and read-write role"
    );
    expect(r.stdout).toContain("provisioned oxy_org_acme on ep-x.neon.tech");
    expect(r.stdout).toContain("raw_toast");
  });

  it("calls a new database billable in the plan", async () => {
    provisioned = false;
    const r = await oxyc(
      "oltp",
      "provision",
      "--org",
      "acme",
      "--writer",
      "app:store-ops",
      "--yes"
    );
    expect(r.status, r.stderr).toBe(0);
    expect(r.stderr).toMatch(
      /a NEW database at the deployment's OLTP provider — a billable resource/
    );
  });

  it("refuses without --yes when stdin is not a terminal, and posts nothing", async () => {
    const r = await oxyc("oltp", "provision", "--org", "acme", "--writer", "app:store-ops");
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(r.stderr).toMatch(/provisioning needs --yes when stdin is not a terminal/);
    expect(requests.some((q) => q.method === "POST")).toBe(false);
  });

  it.each([
    [["--writer", "bogus:x"], /must look like app:<slug> or pipeline:<source>/],
    [["--writer", "app:Store-Ops"], /invalid app writer/],
    [["--writer", "pipeline:toast-pos"], /invalid pipeline source/],
    [[], /name at least one --writer/]
  ])("refuses %j before any request", async (writerArgs, message) => {
    const r = await oxyc(
      "oltp",
      "provision",
      "--org",
      "acme",
      ...(writerArgs as string[]),
      "--yes"
    );
    expect(r.status).toBe(ExitCode.USAGE);
    expect(r.stderr).toMatch(message as RegExp);
    expect(requests).toEqual([]);
  });

  it("explains a deployment with no OLTP provider", async () => {
    provisionStatus = 503;
    provisionBody = "OLTP is not configured on this deployment";
    const r = await oxyc(
      "oltp",
      "provision",
      "--org",
      "acme",
      "--writer",
      "app:store-ops",
      "--yes"
    );
    expect(r.status).toBe(ExitCode.UNAVAILABLE);
    expect(r.stderr).toMatch(/no OLTP provider configured/);
    expect(r.stderr).toContain("server: OLTP is not configured on this deployment");
  });

  it("carries a conflict's reason through from the server", async () => {
    provisionStatus = 409;
    provisionBody = "app:store_ops: [OXY02] schema app_store_ops is owned by another role";
    const r = await oxyc(
      "oltp",
      "provision",
      "--org",
      "acme",
      "--writer",
      "app:store-ops",
      "--yes"
    );
    expect(r.status).toBe(ExitCode.REQUEST);
    expect(r.stderr).toContain("[OXY02] schema app_store_ops is owned by another role");
  });
});
