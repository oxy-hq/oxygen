/**
 * `oxyc oltp dsn | connect | expose | deprovision`, driven through the real
 * binary against a fake deployment — `oltp.test.ts`'s shape, for the verbs that
 * replaced the Rust `oxy oltp dsn | connect | expose | deprovision`.
 *
 * What is pinned: the path and body each verb sends, the shapes a script parses
 * (`dsn`'s bare stdout and `--json`), the exit codes, that the password never
 * reaches `psql`'s argv, and that nothing destructive is sent without `--yes`.
 */

import { spawn } from "node:child_process";
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { ExitCode } from "../util/errors.js";
import { decodeUserinfo, parseRole, splitPassword } from "./oltp-access.js";

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
const ANALYST_DSN =
  "postgresql://oxy_analyst_ro_ab12:p%40ss%2Fw0rd@ep-x.neon.tech:5432/oxy_org_acme?sslmode=require";

function store(provisioned: boolean) {
  return {
    is_provisioned: provisioned,
    host: provisioned ? "ep-x.neon.tech" : "",
    database: provisioned ? "oxy_org_acme" : "",
    provider: provisioned ? "neon" : "",
    project_name: "",
    console_url: null,
    region: "",
    status: provisioned ? "active" : "",
    analyst_role: "oxy_analyst_ro",
    analyst_ready: provisioned,
    platform_schema_version: 3,
    expected_platform_schema_version: 3,
    schemas: []
  };
}

let server: Server;
let base: string;
let requests: Array<{ method: string; url: string; body: string }> = [];
let provisioned = true;
let credentialsStatus = 200;

beforeEach(() => {
  requests = [];
  provisioned = true;
  credentialsStatus = 200;
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
      if (method === "GET" && url === OLTP) return json(res, store(provisioned));
      if (method === "POST" && url === `${OLTP}/credentials`) {
        if (credentialsStatus !== 200) {
          res.writeHead(credentialsStatus, { "content-type": "text/plain" });
          return res.end(credentialsStatus === 404 ? "not found" : "forbidden");
        }
        const role = JSON.parse(body).role as string;
        return json(
          res,
          role === "analyst"
            ? { role: "oxy_analyst_ro_ab12", dsn: ANALYST_DSN, writable: false }
            : {
                role: "app_store_ops_rw",
                dsn: "postgresql://app_store_ops_rw:s3cret@ep-x.neon.tech/oxy_org_acme",
                writable: true
              }
        );
      }
      if (method === "POST" && url === `${OLTP}/visibility`) return json(res, store(true));
      if (method === "POST" && url === `${OLTP}/deprovision-writer`) return json(res, store(true));
      if (method === "DELETE" && url === OLTP) return json(res, store(false));
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
  const home = scratch("oxyc-oltp-access-");
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

describe("--role", () => {
  it("defaults to the analyst, and derives a writer the way provision does", () => {
    expect(parseRole(undefined)).toBe("analyst");
    expect(parseRole("app:store-ops")).toBe("app:store_ops");
    expect(parseRole("pipeline:toast")).toBe("pipeline:toast");
  });

  it.each(["owner", "bogus", "app:Store", ""])("refuses %j", (typed) => {
    expect(() => parseRole(typed)).toThrow();
  });
});

describe("oltp dsn", () => {
  it("prints the bare DSN on stdout and asks for the analyst by default", async () => {
    const r = await oxyc(["oltp", "dsn", "--org", "acme"]);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toBe(`${ANALYST_DSN}\n`);
    const post = requests.find((q) => q.url === `${OLTP}/credentials`);
    expect(JSON.parse(post?.body ?? "{}")).toEqual({ role: "analyst" });
  });

  it("--json is the server's {role, dsn, writable}", async () => {
    const r = await oxyc(["oltp", "dsn", "--org", ORG_ID, "--role", "app:store-ops", "--json"]);
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(r.stdout)).toEqual({
      role: "app_store_ops_rw",
      dsn: "postgresql://app_store_ops_rw:s3cret@ep-x.neon.tech/oxy_org_acme",
      writable: true
    });
    const post = requests.find((q) => q.url === `${OLTP}/credentials`);
    expect(JSON.parse(post?.body ?? "{}")).toEqual({ role: "app:store_ops" });
  });

  it("refuses --role owner before any request", async () => {
    const r = await oxyc(["oltp", "dsn", "--org", ORG_ID, "--role", "owner"]);
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });

  it("maps a 404 (no store, or outside the grant) to exit 5", async () => {
    credentialsStatus = 404;
    const r = await oxyc(["oltp", "dsn", "--org", ORG_ID]);
    expect(r.status).toBe(ExitCode.NOT_FOUND);
    expect(r.stdout).toBe("");
  });

  it("maps a 403 to exit 4", async () => {
    credentialsStatus = 403;
    const r = await oxyc(["oltp", "dsn", "--org", ORG_ID]);
    expect(r.status).toBe(ExitCode.AUTH);
  });
});

describe("oltp connect", () => {
  /** A `psql` on PATH that records what it was handed. */
  function fakePsql(exit = 0, before = ""): { dir: string; args: string; password: string } {
    const dir = scratch("oxyc-fake-psql-");
    const args = join(dir, "args");
    const password = join(dir, "password");
    const script = join(dir, "psql");
    writeFileSync(
      script,
      `#!/bin/sh\n${before}printf '%s' "$*" > '${args}'\nprintf '%s' "\${PGPASSWORD-UNSET}" > '${password}'\nexit ${exit}\n`
    );
    chmodSync(script, 0o755);
    return { dir, args, password };
  }

  it("hands psql the DSN without its password, and the decoded password in PGPASSWORD", async () => {
    const psql = fakePsql();
    const r = await oxyc(["oltp", "connect", "--org", ORG_ID], {
      PATH: `${psql.dir}:/usr/bin:/bin`
    });
    expect(r.status, r.stderr).toBe(0);
    const argv = readFileSync(psql.args, "utf8");
    expect(argv).toBe(
      "postgresql://oxy_analyst_ro_ab12@ep-x.neon.tech:5432/oxy_org_acme?sslmode=require"
    );
    expect(argv).not.toContain("p%40ss");
    expect(readFileSync(psql.password, "utf8")).toBe("p@ss/w0rd");
    expect(r.stderr).toContain("connecting as oxy_analyst_ro_ab12");
  });

  it("survives the ^C psql catches to cancel a query, and keeps psql's exit code", async () => {
    // The terminal sends SIGINT to the whole foreground group; here psql's
    // stand-in sends it to oxyc itself, then sleeps and exits cleanly.
    const psql = fakePsql(0, "kill -INT $PPID; kill -QUIT $PPID; sleep 1\n");
    const r = await oxyc(["oltp", "connect", "--org", ORG_ID], {
      PATH: `${psql.dir}:/usr/bin:/bin`
    });
    expect(r.status, r.stderr).toBe(0);
    expect(readFileSync(psql.args, "utf8")).toContain("oxy_analyst_ro_ab12@");
  });

  it("exits 1 when psql fails", async () => {
    const psql = fakePsql(3);
    const r = await oxyc(["oltp", "connect", "--org", ORG_ID], {
      PATH: `${psql.dir}:/usr/bin:/bin`
    });
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stderr).toContain("psql exited with 3");
  });

  it("exits 7 when psql is not installed", async () => {
    const empty = scratch("oxyc-no-psql-");
    const r = await oxyc(["oltp", "connect", "--org", ORG_ID], { PATH: empty });
    expect(r.status).toBe(ExitCode.UNAVAILABLE);
    expect(r.stderr).toContain("psql is not on PATH");
  });
});

/** Ported from the Rust `oltp.rs` `connect_tests` with the code they covered. */
describe("splitPassword", () => {
  it("takes the password out of the DSN", () => {
    const [dsn, pw] = splitPassword("postgresql://app_bookings_rw:s3cret@db.neon.tech/oxy_org_x");
    expect(dsn).toBe("postgresql://app_bookings_rw@db.neon.tech/oxy_org_x");
    expect(pw).toBe("s3cret");
  });

  it("keeps every other part of the DSN, sslmode included", () => {
    const [dsn] = splitPassword(
      "postgresql://u:p@db.neon.tech:5433/oxy_org_x?sslmode=require&options=-csearch_path%3Dapp_x"
    );
    expect(dsn).toBe(
      "postgresql://u@db.neon.tech:5433/oxy_org_x?sslmode=require&options=-csearch_path%3Dapp_x"
    );
  });

  it("does not split the host on a literal @ in the password", () => {
    expect(splitPassword("postgresql://u:pa@ss@host/db")).toEqual([
      "postgresql://u@host/db",
      "pa@ss"
    ]);
  });

  it("decodes an encoded @", () => {
    expect(splitPassword("postgresql://u:pa%40ss@host/db")).toEqual([
      "postgresql://u@host/db",
      "pa@ss"
    ]);
  });

  it("decodes the password on the way out, and leaves a stray % alone", () => {
    expect(decodeUserinfo("a%2Fb%3Ac%40d")).toBe("a/b:c@d");
    expect(decodeUserinfo("plain")).toBe("plain");
    expect(decodeUserinfo("100%")).toBe("100%");
  });

  it.each([
    "postgresql://postgres@localhost:5432/oxy_org_x",
    "postgresql://localhost/oxy_org_x",
    "not-a-url"
  ])("passes %j through when there is no password", (dsn) => {
    expect(splitPassword(dsn)).toEqual([dsn, undefined]);
  });
});

describe("oltp expose", () => {
  it("posts the derived writer, visible", async () => {
    const r = await oxyc(["oltp", "expose", "--org", ORG_ID, "--writer", "app:store-ops"]);
    expect(r.status, r.stderr).toBe(0);
    const post = requests.find((q) => q.url === `${OLTP}/visibility`);
    expect(JSON.parse(post?.body ?? "{}")).toEqual({ writer: "app:store_ops", visible: true });
    expect(r.stdout).toContain("app_store_ops is readable by the read-only analyst");
  });

  it("--revoke posts visible: false", async () => {
    const r = await oxyc([
      "oltp",
      "expose",
      "--org",
      ORG_ID,
      "--writer",
      "pipeline:toast",
      "--revoke"
    ]);
    expect(r.status, r.stderr).toBe(0);
    const post = requests.find((q) => q.url === `${OLTP}/visibility`);
    expect(JSON.parse(post?.body ?? "{}")).toEqual({ writer: "pipeline:toast", visible: false });
    expect(r.stdout).toContain("raw_toast is hidden from");
  });

  it("needs --writer, and sends nothing without it", async () => {
    const r = await oxyc(["oltp", "expose", "--org", ORG_ID]);
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });
});

describe("oltp deprovision", () => {
  it("refuses without --yes with exit 8, printing what it would destroy, and sends no delete", async () => {
    const r = await oxyc(["oltp", "deprovision", "--org", ORG_ID]);
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(r.stderr).toContain("This DESTROYS oxy_org_acme on ep-x.neon.tech (neon)");
    expect(r.stderr).toContain("cannot be recovered");
    expect(sent()).toEqual([`GET ${OLTP}`]);
  });

  it("refuses a writer without --yes with exit 8, and posts nothing", async () => {
    const r = await oxyc(["oltp", "deprovision", "--org", ORG_ID, "--writer", "app:store-ops"]);
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(r.stderr).toContain("This DROPS schema app_store_ops");
    expect(sent()).toEqual([`GET ${OLTP}`]);
  });

  it.each(["", "store-ops", "bogus:x"])(
    "a malformed --writer %j is a usage error, never the whole-tenant delete",
    async (typed) => {
      const r = await oxyc(["oltp", "deprovision", "--org", ORG_ID, "--writer", typed, "--yes"]);
      expect(r.status).toBe(ExitCode.USAGE);
      expect(requests).toEqual([]);
    }
  );

  it("--writer --yes posts that writer to deprovision-writer, and deletes nothing", async () => {
    const r = await oxyc([
      "oltp",
      "deprovision",
      "--org",
      ORG_ID,
      "--writer",
      "app:store-ops",
      "--yes"
    ]);
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`GET ${OLTP}`, `POST ${OLTP}/deprovision-writer`]);
    const post = requests.find((q) => q.url === `${OLTP}/deprovision-writer`);
    expect(JSON.parse(post?.body ?? "{}")).toEqual({ writer: "app:store_ops" });
    expect(r.stdout).toContain("deprovisioned writer app:store_ops (schema app_store_ops)");
  });

  it("--yes without --writer sends the whole-tenant DELETE", async () => {
    const r = await oxyc(["oltp", "deprovision", "--org", ORG_ID, "--yes"]);
    expect(r.status, r.stderr).toBe(0);
    expect(sent()).toEqual([`GET ${OLTP}`, `DELETE ${OLTP}`]);
    expect(r.stdout).toContain("deprovisioned oxy_org_acme (neon)");
  });

  it("an org with no database is a no-op that exits 0", async () => {
    provisioned = false;
    const r = await oxyc(["oltp", "deprovision", "--org", ORG_ID, "--yes"]);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain("nothing to do");
    expect(sent()).toEqual([`GET ${OLTP}`]);
  });
});
