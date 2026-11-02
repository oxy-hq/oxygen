/**
 * `oxyc airhouse`, driven through the real binary against a fake deployment.
 *
 * A LOCAL SERVER, for `oltp.test.ts`'s reason: what is under test is the wire
 * contract with the admin Airhouse routes — the paths, that `provision` sends
 * no body, and that nothing is posted before the workspace and the
 * confirmation are settled.
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
import { type AirhouseFleetRow, provisionPath, tenantNameFor } from "./airhouse.js";

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");
const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

const WS_WITH = "11111111-2222-3333-4444-555555555555";
const WS_WITHOUT = "22222222-2222-3333-4444-555555555555";
const ORG_ID = "99999999-2222-3333-4444-555555555555";

function row(workspaceId: string, name: string, provisioned: boolean): AirhouseFleetRow {
  if (!provisioned) {
    return {
      workspace_id: workspaceId,
      workspace_name: name,
      org_id: ORG_ID,
      org_name: "Acme",
      status: "none",
      tenant_id: "",
      bucket: "",
      prefix: "",
      service_account_ready: false,
      sa_rotated_at: null,
      created_at: null,
      service_account_id: null,
      sa_created_at: null,
      bearer_max_role: null,
      bearer_max_ttl_secs: null
    };
  }
  return {
    workspace_id: workspaceId,
    workspace_name: name,
    org_id: ORG_ID,
    org_name: "Acme",
    status: "active",
    tenant_id: tenantNameFor(workspaceId),
    bucket: "oxy-airhouse",
    prefix: `tenants/${workspaceId}`,
    service_account_ready: true,
    sa_rotated_at: null,
    created_at: "2026-10-01T00:00:00+00:00",
    service_account_id: "sa-1",
    sa_created_at: "2026-10-01T00:00:00+00:00",
    bearer_max_role: "admin",
    bearer_max_ttl_secs: 86400
  };
}

let server: Server;
let base: string;
/** Every request the fake deployment received, in order. */
let requests: Array<{ method: string; url: string; body: string }> = [];
let provisionStatus = 200;
let provisionBody = "";
let truncatedUnprovisioned = false;

beforeEach(() => {
  requests = [];
  provisionStatus = 200;
  provisionBody = "";
  truncatedUnprovisioned = false;
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
      if (method === "GET" && url === "/api/admin/airhouse") {
        return json(res, {
          rows: [row(WS_WITH, "Analytics", true), row(WS_WITHOUT, "Sandbox", false)],
          truncated: { unprovisioned: truncatedUnprovisioned, provisioned: false }
        });
      }
      for (const ws of [WS_WITH, WS_WITHOUT]) {
        if (method === "POST" && url === provisionPath(ws)) {
          if (provisionStatus !== 200) {
            res.writeHead(provisionStatus, { "content-type": "text/plain" });
            return res.end(provisionBody);
          }
          return json(res, row(ws, ws === WS_WITH ? "Analytics" : "Sandbox", true));
        }
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
  const home = mkdtempSync(join(tmpdir(), "oxyc-airhouse-"));
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

describe("airhouse status", () => {
  it("lists every workspace, with a tenant or without", async () => {
    const r = await oxyc("airhouse", "status");
    expect(r.status, r.stderr).toBe(0);
    expect(requests.map((q) => `${q.method} ${q.url}`)).toEqual(["GET /api/admin/airhouse"]);
    for (const part of ["Analytics", WS_WITH, tenantNameFor(WS_WITH), "Sandbox", "none"]) {
      expect(r.stdout).toContain(part);
    }
    expect(r.stderr).toMatch(/1 of 2 workspace\(s\) have an Airhouse tenant/);
  });

  it("shows one workspace's tenant", async () => {
    const r = await oxyc("airhouse", "status", "--workspace", WS_WITH);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toMatch(new RegExp(`tenant\\s+${tenantNameFor(WS_WITH)}`));
    expect(r.stdout).toMatch(/service\s+ready \(sa-1\)/);
    expect(r.stdout).toContain(`s3://oxy-airhouse/tenants/${WS_WITH}`);
  });

  it("says when a workspace has no tenant, and how to make one", async () => {
    const r = await oxyc("airhouse", "status", "--workspace", WS_WITHOUT);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain(`no Airhouse tenant for workspace Sandbox (${WS_WITHOUT})`);
    expect(r.stderr).toContain(`oxyc airhouse provision --workspace ${WS_WITHOUT}`);
  });

  it("emits the row with --json", async () => {
    const r = await oxyc("airhouse", "status", "--workspace", WS_WITH, "--json");
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(r.stdout)).toMatchObject({ workspace_id: WS_WITH, status: "active" });
  });

  it("does not call a workspace missing from a capped fleet nonexistent", async () => {
    truncatedUnprovisioned = true;
    const absent = "33333333-2222-3333-4444-555555555555";
    const r = await oxyc("airhouse", "status", "--workspace", absent);
    expect(r.status).toBe(ExitCode.NOT_FOUND);
    expect(r.stderr).toContain("is not on the fleet page");
    expect(r.stderr).toContain("may exist without a tenant");
  });

  it("refuses a workspace that is not an id before any request", async () => {
    const r = await oxyc("airhouse", "status", "--workspace", "my-workspace");
    expect(r.status).toBe(ExitCode.USAGE);
    expect(r.stderr).toMatch(/--workspace my-workspace is not a workspace id/);
    expect(requests).toEqual([]);
  });
});

describe("airhouse provision", () => {
  it("prints the plan, then posts to the workspace's route with no body", async () => {
    const r = await oxyc("airhouse", "provision", "--workspace", WS_WITHOUT, "--yes");
    expect(r.status, r.stderr).toBe(0);
    const posts = requests.filter((q) => q.method === "POST");
    expect(posts.map((q) => q.url)).toEqual([provisionPath(WS_WITHOUT)]);
    expect(posts[0]?.body).toBe("");
    expect(r.stderr).toContain(`for workspace Sandbox (${WS_WITHOUT}), org Acme`);
    expect(r.stderr).toContain(
      `tenant    a NEW tenant ${tenantNameFor(WS_WITHOUT)} — the name is global and permanent`
    );
    expect(r.stdout).toContain(`provisioned Airhouse tenant ${tenantNameFor(WS_WITHOUT)}`);
  });

  it("calls an existing tenant reconciled", async () => {
    const r = await oxyc("airhouse", "provision", "--workspace", WS_WITH, "--yes");
    expect(r.status, r.stderr).toBe(0);
    expect(r.stderr).toContain(`tenant    ${tenantNameFor(WS_WITH)} — exists (active), reconciled`);
    expect(r.stdout).toContain(`reconciled Airhouse tenant ${tenantNameFor(WS_WITH)}`);
  });

  it("emits the server's row with --json", async () => {
    const r = await oxyc("airhouse", "provision", "--workspace", WS_WITHOUT, "--yes", "--json");
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(r.stdout)).toMatchObject({
      workspace_id: WS_WITHOUT,
      tenant_id: tenantNameFor(WS_WITHOUT)
    });
  });

  it("refuses without --yes when stdin is not a terminal, and posts nothing", async () => {
    const r = await oxyc("airhouse", "provision", "--workspace", WS_WITHOUT);
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(r.stderr).toMatch(/provisioning needs --yes when stdin is not a terminal/);
    expect(requests.some((q) => q.method === "POST")).toBe(false);
  });

  it.each([
    [[], /name the workspace: --workspace <id>/],
    [["--workspace", "not-a-uuid"], /--workspace not-a-uuid is not a workspace id/]
  ])("refuses %j before any request", async (args, message) => {
    const r = await oxyc("airhouse", "provision", ...(args as string[]), "--yes");
    expect(r.status).toBe(ExitCode.USAGE);
    expect(r.stderr).toMatch(message as RegExp);
    expect(requests).toEqual([]);
  });

  it("explains a deployment with no Airhouse", async () => {
    provisionStatus = 503;
    provisionBody = "Airhouse is not configured on this deployment";
    const r = await oxyc("airhouse", "provision", "--workspace", WS_WITHOUT, "--yes");
    expect(r.status).toBe(ExitCode.UNAVAILABLE);
    expect(r.stderr).toMatch(/no Airhouse configured/);
    expect(r.stderr).toContain("server: Airhouse is not configured on this deployment");
  });

  it("carries a name collision through from the server", async () => {
    provisionStatus = 409;
    provisionBody = `tenant name ${tenantNameFor(WS_WITHOUT)} is taken`;
    const r = await oxyc("airhouse", "provision", "--workspace", WS_WITHOUT, "--yes");
    expect(r.status).toBe(ExitCode.REQUEST);
    expect(r.stderr).toMatch(/tenant name is already taken at Airhouse/);
    expect(r.stderr).toContain(`server: tenant name ${tenantNameFor(WS_WITHOUT)} is taken`);
  });
});
