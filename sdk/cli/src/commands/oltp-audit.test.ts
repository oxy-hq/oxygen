/**
 * `oxyc oltp audit | rotate`, driven through the real binary against a fake
 * deployment — `oltp-access.test.ts`'s shape, for the verbs that replaced the
 * Rust `oxy oltp audit | rotate`.
 *
 * What is pinned: the path each verb sends, the report a script parses
 * (`--json` is the server's body), the exit codes (9 when the audit finds a
 * violation, `--json` included; 8 for a rotation without `--yes`, with nothing
 * sent), and that a rotation asks for a writer only.
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
import { auditFailure, type ConfinementAudit, renderAudit, rotatePath } from "./oltp-audit.js";

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

const ORG_ID = "11111111-2222-3333-4444-555555555555";
const OLTP = `/api/admin/orgs/${ORG_ID}/oltp`;
let server: Server;
let base: string;
let requests: Array<{ method: string; url: string; body: string }> = [];
let auditStatus = 200;
let report: ConfinementAudit;

beforeEach(() => {
  requests = [];
  auditStatus = 200;
  report = cleanReport();
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
      if (method === "POST" && url === `${OLTP}/audit`) {
        if (auditStatus !== 200) {
          res.writeHead(auditStatus, { "content-type": "text/plain" });
          return res.end("refused");
        }
        return json(res, report);
      }
      if (method === "POST" && url === `${OLTP}/roles/app%3Astore_ops/rotate`) {
        return json(res, {
          writer: "app:store_ops",
          role: "app_store_ops_rw",
          schema: "app_store_ops"
        });
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
function oxyc(
  args: string[],
  extraEnv: Record<string, string> = {}
): Promise<{ status: number; stdout: string; stderr: string }> {
  if (!existsSync(BIN)) throw new Error(`${BIN} missing — run \`pnpm build\``);
  const home = scratch("oxyc-oltp-audit-");
  return new Promise((done) => {
    const child = spawn(process.execPath, [BIN, ...args, "--target", base], {
      env: {
        ...process.env,
        HOME: home,
        OXY_CREDENTIALS_PATH: join(home, "credentials.json"),
        OXYC_CACHE_DIR: join(home, "cache"),
        OXY_TOKEN: "test-token",
        NO_COLOR: "1",
        ...extraEnv
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

/** Requests past the org lookup — what the verb itself sent. */
const sent = () =>
  requests.filter((q) => !q.url.startsWith("/api/orgs")).map((q) => `${q.method} ${q.url}`);

function cleanReport(): ConfinementAudit {
  return {
    org_id: ORG_ID,
    database: "oxy_org_acme",
    owner_role: "oxy_owner",
    roles: [
      {
        role: "app_store_ops_rw",
        login: true,
        risk: "none",
        attributes: [],
        memberships: [],
        confined: true
      },
      {
        role: "oxy_owner",
        login: true,
        risk: "admin",
        attributes: ["createrole"],
        memberships: ["neon_superuser"],
        confined: true
      }
    ],
    grants: [{ role: "app_store_ops_rw", findings: [] }],
    unconfined: 0,
    drifted: 0,
    clean: true
  };
}

function dirtyReport(): ConfinementAudit {
  const r = cleanReport();
  r.roles[0] = {
    role: "app_store_ops_rw",
    login: true,
    risk: "replication",
    attributes: ["replication"],
    memberships: [],
    confined: false
  };
  r.grants.push({ role: "PUBLIC", findings: ["USAGE on schema app_store_ops"] });
  return { ...r, unconfined: 1, drifted: 1, clean: false };
}

describe("oltp audit", () => {
  it("POSTs the org's audit with no body, prints the Rust command's report, and exits 0 when clean", async () => {
    const r = await oxyc(["oltp", "audit", "--org", "acme"]);
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    expect(sent()).toEqual([`POST ${OLTP}/audit`]);
    expect(requests.find((q) => q.url === `${OLTP}/audit`)?.body).toBe("");
    expect(r.stdout).toContain("app_store_ops_rw");
    expect(r.stdout).toContain("all non-admin roles confined");
    expect(r.stdout).toContain("no role holds privileges outside its scope");
    // The owner's authority is shown, and is not a finding.
    expect(r.stdout).toMatch(/oxy_owner\s+yes\s+admin\s+neon_superuser/);
  });

  it("exits 9 when the check finds a violation, naming both halves", async () => {
    report = dirtyReport();
    const r = await oxyc(["oltp", "audit", "--org", ORG_ID]);
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(r.stdout).toContain("1 role(s) hold authority they should not");
    expect(r.stdout).toContain("- USAGE on schema app_store_ops");
    expect(r.stderr).toContain(
      "1 role(s) are not confined and 1 hold privileges outside their scope"
    );
  });

  it("--json is the server's report on stdout, and still exits 9 on a violation", async () => {
    report = dirtyReport();
    const r = await oxyc(["oltp", "audit", "--org", ORG_ID, "--json"]);
    expect(r.status).toBe(ExitCode.CHECK_FAILED);
    expect(JSON.parse(r.stdout)).toEqual(dirtyReport());
  });

  it("--json exits 0 on a clean report", async () => {
    const r = await oxyc(["oltp", "audit", "--org", ORG_ID, "--json"]);
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    expect(JSON.parse(r.stdout).clean).toBe(true);
  });

  it.each([
    [404, ExitCode.NOT_FOUND],
    [403, ExitCode.AUTH],
    [502, ExitCode.UNAVAILABLE],
    [400, ExitCode.REQUEST]
  ])("maps a %i to exit %i, with nothing on stdout", async (status, code) => {
    auditStatus = status;
    const r = await oxyc(["oltp", "audit", "--org", ORG_ID]);
    expect(r.status).toBe(code);
    expect(r.stdout).toBe("");
  });

  it("explains a 502 as an audit that did not run", async () => {
    auditStatus = 502;
    const r = await oxyc(["oltp", "audit", "--org", ORG_ID]);
    expect(r.stderr).toContain("the audit did not run");
  });
});

describe("auditFailure", () => {
  it("is undefined for a clean report, and fails a not-clean one with zero counts", () => {
    expect(auditFailure(cleanReport())).toBeUndefined();
    expect(auditFailure({ ...cleanReport(), clean: false })).toBeDefined();
    expect(auditFailure({ ...cleanReport(), clean: false, unconfined: 2 })).toBe(
      "2 role(s) are not confined"
    );
    expect(auditFailure({ ...cleanReport(), clean: false, drifted: 3 })).toBe(
      "3 role(s) hold privileges outside their scope"
    );
  });

  it("renders a membership-only finding as `member`", () => {
    const r = cleanReport();
    r.roles[0] = {
      ...(r.roles[0] as ConfinementAudit["roles"][number]),
      risk: "",
      memberships: ["pg_read"],
      confined: false
    };
    expect(renderAudit({ ...r, unconfined: 1, clean: false })).toMatch(
      /app_store_ops_rw\s+yes\s+member\s+pg_read/
    );
  });
});

describe("oltp rotate", () => {
  it("posts to the writer's rotate route with no body and prints the role", async () => {
    const r = await oxyc(["oltp", "rotate", "--org", ORG_ID, "--role", "app:store-ops", "--yes"]);
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    expect(sent()).toEqual([`POST ${OLTP}/roles/app%3Astore_ops/rotate`]);
    const post = requests.find((q) => q.method === "POST");
    expect(post?.body).toBe("");
    expect(r.stdout).toContain("rotated app_store_ops_rw (app_store_ops)");
  });

  it("--json is the server's {writer, role, schema}, and carries no credential", async () => {
    const r = await oxyc([
      "oltp",
      "rotate",
      "--org",
      ORG_ID,
      "--role",
      "app:store_ops",
      "--yes",
      "--json"
    ]);
    expect(r.status, r.stderr).toBe(ExitCode.OK);
    const body = JSON.parse(r.stdout);
    expect(body).toEqual({
      writer: "app:store_ops",
      role: "app_store_ops_rw",
      schema: "app_store_ops"
    });
    expect(body).not.toHaveProperty("dsn");
    expect(body).not.toHaveProperty("password");
  });

  it("refuses without --yes with exit 8, and posts nothing", async () => {
    const r = await oxyc(["oltp", "rotate", "--org", ORG_ID, "--role", "app:store-ops"]);
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(sent()).toEqual([]);
    expect(r.stderr).toContain("rotate app:store_ops");
  });

  it.each([[[]], [["--role", "analyst"]], [["--role", "owner"]], [["--role", "bogus"]]])(
    "is a usage error before any request: %j",
    async (extra) => {
      const r = await oxyc(["oltp", "rotate", "--org", ORG_ID, ...extra, "--yes"]);
      expect(r.status).toBe(ExitCode.USAGE);
      expect(requests).toEqual([]);
    }
  );

  it("encodes the writer into one path segment", () => {
    expect(rotatePath(ORG_ID, "pipeline:toast")).toBe(`${OLTP}/roles/pipeline%3Atoast/rotate`);
  });
});
