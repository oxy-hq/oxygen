/**
 * `oxyc sync`, driven through the real binary against a fake deployment.
 *
 * A LOCAL SERVER, for `airway.test.ts`'s reason: what is under test is the wire
 * contract with `POST /api/{workspace}/databases/sync` (the body, one request
 * per database), what lands in the checkout, and that a path the server should
 * never return writes nothing. The exit codes are pinned case by case, because
 * they are the contract a script branches on.
 *
 * The write rules are the ones the `oxy sync` binary tests pinned, ported here
 * with the disk half of the command: an overwrite prunes a stale file from
 * `models/` and never touches the database directory.
 */

import { spawn } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync
} from "node:fs";
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
import { parseFiles, plan, type RenderedFile, resolveInside } from "./sync.js";

vi.setConfig({ testTimeout: CLI_TEST_TIMEOUT_MS });
const track = reapChildrenAfterEach();

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");
const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});
function scratch(prefix = "oxyc-sync-"): string {
  const dir = mkdtempSync(join(tmpdir(), prefix));
  SCRATCH.push(dir);
  return dir;
}

const WS = "11111111-2222-3333-4444-555555555555";
const SYNC = `/api/${WS}/databases/sync`;
const LIST = `/api/${WS}/databases`;

const SQL = ".databases/primary_database/dbt_prod_metrics.sql";
const MODEL = (t: string) => `.databases/primary_database/dbt_prod_metrics/models/${t}.schema.yml`;

const BQ_FILES: RenderedFile[] = [
  {
    path: SQL,
    content: "CREATE TABLE `df-warehouse-prod.dbt_prod_metrics.monthly_active_users` (id INT64);\n"
  },
  {
    path: MODEL("monthly_active_users"),
    content: "table: dbt_prod_metrics.monthly_active_users\n"
  }
];

let server: Server;
let base: string;
let requests: Array<{ method: string; url: string; body: string }> = [];
let databases: unknown[] = [];
/** The reply per database name. */
let replies: Record<string, { status: number; body: unknown }> = {};

beforeEach(() => {
  requests = [];
  databases = [{ name: "primary_database" }, { name: "airhouse" }, { name: "broken" }];
  replies = {
    primary_database: { status: 200, body: BQ_FILES },
    airhouse: {
      status: 422,
      body: {
        error: "not_syncable",
        message: "database 'airhouse' is airhouse_managed, which sync cannot introspect"
      }
    },
    broken: { status: 502, body: { error: "introspection_failed", message: "connection reset" } }
  };
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
      if (method === "GET" && url === LIST) return json(res, databases);
      if (method === "POST" && url === SYNC) {
        const name = (JSON.parse(body) as { database: string }).database;
        const reply = replies[name] ?? {
          status: 404,
          body: { error: "database_not_found", message: `Database '${name}' not found in config` }
        };
        return json(res, reply.body, reply.status);
      }
      res.writeHead(404);
      res.end("{}");
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(() => server.close());

/** Async for `airway.test.ts`'s reason: `spawnSync` would block the server above. */
function oxyc(
  cwd: string,
  ...args: string[]
): Promise<{ status: number; stdout: string; stderr: string }> {
  if (!existsSync(BIN)) throw new Error(`${BIN} missing — run \`pnpm build\``);
  const home = scratch("oxyc-sync-home-");
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

const posts = () => requests.filter((q) => q.method === "POST").map((q) => JSON.parse(q.body));

describe("one database", () => {
  it("posts the database and its datasets, and writes the files under the checkout", async () => {
    const repo = scratch();
    const r = await oxyc(
      repo,
      "sync",
      "primary_database",
      "-d",
      "dbt_prod_metrics",
      "--workspace",
      WS
    );
    expect(r.status, r.stderr).toBe(0);
    expect(posts()).toEqual([
      { database: "primary_database", datasets: ["dbt_prod_metrics"], tables: [] }
    ]);
    // The paths and contents are the server's, byte for byte.
    expect(readFileSync(join(repo, SQL), "utf8")).toBe(BQ_FILES[0]?.content);
    expect(readFileSync(join(repo, MODEL("monthly_active_users")), "utf8")).toContain(
      "table: dbt_prod_metrics.monthly_active_users"
    );
    expect(requests.some((q) => q.method === "GET")).toBe(false);
  });

  it("--database is the positional's spelling, and --tables goes in the body", async () => {
    const repo = scratch();
    const r = await oxyc(
      repo,
      "sync",
      "--database",
      "primary_database",
      "--tables",
      "dbt_prod_metrics.monthly_active_users",
      "--workspace",
      WS
    );
    expect(r.status, r.stderr).toBe(0);
    expect(posts()[0]).toEqual({
      database: "primary_database",
      datasets: [],
      tables: ["dbt_prod_metrics.monthly_active_users"]
    });
  });

  it("an unknown database exits 5 with the server's message", async () => {
    const r = await oxyc(scratch(), "sync", "non_existent_database", "--workspace", WS);
    expect(r.status).toBe(ExitCode.NOT_FOUND);
    expect(r.stderr).toContain("Database 'non_existent_database' not found in config");
  });

  it("a named type sync cannot introspect exits 6", async () => {
    const r = await oxyc(scratch(), "sync", "airhouse", "--workspace", WS);
    expect(r.status).toBe(ExitCode.REQUEST);
    expect(r.stderr).toContain("airhouse_managed");
  });

  it.each([
    [400, ExitCode.REQUEST],
    [403, ExitCode.AUTH],
    [408, ExitCode.UNAVAILABLE],
    [502, ExitCode.UNAVAILABLE]
  ])("a %i exits %i and writes nothing", async (status, code) => {
    replies.primary_database = { status, body: { error: "x", message: "nope" } };
    const repo = scratch();
    const r = await oxyc(repo, "sync", "primary_database", "--workspace", WS);
    expect(r.status).toBe(code);
    expect(existsSync(join(repo, ".databases"))).toBe(false);
  });

  it("a body that is not a list of files exits 1", async () => {
    replies.primary_database = { status: 200, body: { files: [] } };
    const r = await oxyc(scratch(), "sync", "primary_database", "--workspace", WS);
    expect(r.status).toBe(ExitCode.FAILURE);
  });
});

describe("every database", () => {
  it("asks for each listed database; a non-syncable one is skipped and a failed one fails the run", async () => {
    const repo = scratch();
    const r = await oxyc(repo, "sync", "--workspace", WS);
    expect(posts().map((p) => p.database)).toEqual(["primary_database", "airhouse", "broken"]);
    // The database that rendered is written even though another failed.
    expect(existsSync(join(repo, SQL))).toBe(true);
    expect(r.stderr).toContain("skipped airhouse");
    expect(r.stderr).toContain("broken: connection reset");
    expect(r.status).toBe(ExitCode.UNAVAILABLE);
  });

  it("exits 0 when the only refusals are skips", async () => {
    databases = [{ name: "primary_database" }, { name: "airhouse" }];
    const r = await oxyc(scratch(), "sync", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
  });

  it("--json reports each database and each file", async () => {
    databases = [{ name: "primary_database" }, { name: "airhouse" }];
    const r = await oxyc(scratch(), "sync", "--workspace", WS, "--json");
    expect(r.status, r.stderr).toBe(0);
    const report = JSON.parse(r.stdout);
    expect(Object.keys(report)).toEqual(["dry_run", "databases", "files"]);
    expect(report.databases).toEqual([
      { database: "primary_database", status: "rendered", files: 2 },
      expect.objectContaining({ database: "airhouse", status: "skipped" })
    ]);
    expect(report.files).toEqual([
      { path: SQL, action: "create" },
      { path: MODEL("monthly_active_users"), action: "create" }
    ]);
  });

  it("-d without a database is a usage error, before any request", async () => {
    const r = await oxyc(scratch(), "sync", "-d", "x", "--workspace", WS);
    expect(r.status).toBe(ExitCode.USAGE);
    expect(requests).toEqual([]);
  });
});

describe("writing into the checkout", () => {
  it("skips an existing file unless --overwrite", async () => {
    const repo = scratch();
    mkdirSync(dirname(join(repo, SQL)), { recursive: true });
    writeFileSync(join(repo, SQL), "hand edited\n");
    let r = await oxyc(repo, "sync", "primary_database", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(readFileSync(join(repo, SQL), "utf8")).toBe("hand edited\n");
    expect(r.stderr).toContain("--overwrite");

    r = await oxyc(repo, "sync", "primary_database", "--overwrite", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(readFileSync(join(repo, SQL), "utf8")).toBe(BQ_FILES[0]?.content);
  });

  // Ported from `test_sync_database_with_overwrite_flag`.
  it("--overwrite prunes a stale file from a models/ directory", async () => {
    const repo = scratch();
    expect((await oxyc(repo, "sync", "primary_database", "--workspace", WS)).status).toBe(0);
    const dummy = join(repo, MODEL("dummy_test_file"));
    writeFileSync(dummy, "test content for deletion");
    const r = await oxyc(repo, "sync", "primary_database", "-o", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(existsSync(dummy)).toBe(false);
    expect(existsSync(join(repo, SQL))).toBe(true);
  });

  // Ported from `test_sync_entire_database_with_overwrite_flag`.
  it("--overwrite never touches a file in the database directory", async () => {
    const repo = scratch();
    expect((await oxyc(repo, "sync", "primary_database", "--workspace", WS)).status).toBe(0);
    const kept = join(repo, ".databases/primary_database/dummy_test_file.txt");
    writeFileSync(kept, "test content that should not be deleted");
    const r = await oxyc(repo, "sync", "primary_database", "--overwrite", "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(existsSync(kept)).toBe(true);
  });

  it("--tables with --overwrite replaces only those models: no prune, the dataset's .sql kept", async () => {
    const repo = scratch();
    expect((await oxyc(repo, "sync", "primary_database", "--workspace", WS)).status).toBe(0);
    const other = join(repo, MODEL("monthly_active_organizations"));
    writeFileSync(other, "table: dbt_prod_metrics.monthly_active_organizations\n");
    writeFileSync(join(repo, SQL), "full ddl\n");
    writeFileSync(join(repo, MODEL("monthly_active_users")), "stale\n");
    const r = await oxyc(
      repo,
      "sync",
      "primary_database",
      "--tables",
      "dbt_prod_metrics.monthly_active_users",
      "-o",
      "--workspace",
      WS
    );
    expect(r.status, r.stderr).toBe(0);
    expect(existsSync(other)).toBe(true);
    expect(readFileSync(join(repo, SQL), "utf8")).toBe("full ddl\n");
    expect(readFileSync(join(repo, MODEL("monthly_active_users")), "utf8")).toBe(
      BQ_FILES[1]?.content
    );
  });

  it("--dry-run writes and deletes nothing", async () => {
    const repo = scratch();
    expect((await oxyc(repo, "sync", "primary_database", "--workspace", WS)).status).toBe(0);
    const dummy = join(repo, MODEL("dummy_test_file"));
    writeFileSync(dummy, "stale");
    const r = await oxyc(
      repo,
      "sync",
      "primary_database",
      "-o",
      "--dry-run",
      "--json",
      "--workspace",
      WS
    );
    expect(r.status, r.stderr).toBe(0);
    expect(existsSync(dummy)).toBe(true);
    const report = JSON.parse(r.stdout);
    expect(report.dry_run).toBe(true);
    expect(report.files).toContainEqual({ path: MODEL("dummy_test_file"), action: "delete" });
  });

  it("--dir writes under that checkout", async () => {
    const repo = scratch();
    const r = await oxyc(scratch(), "sync", "primary_database", "--dir", repo, "--workspace", WS);
    expect(r.status, r.stderr).toBe(0);
    expect(existsSync(join(repo, SQL))).toBe(true);
  });

  it.each([
    "../escape.sql",
    ".databases/../../escape.sql",
    "/etc/passwd",
    ".databases/db/../../../escape.sql",
    ".git/hooks/pre-commit",
    "config.yml",
    ".databases\\db\\x.sql",
    ".databases/db"
  ])("refuses %j with exit 8 and writes nothing", async (path) => {
    replies.primary_database = {
      status: 200,
      body: [...BQ_FILES, { path, content: "#!/bin/sh\necho owned\n" }]
    };
    const parent = scratch();
    const repo = join(parent, "repo");
    mkdirSync(repo);
    const r = await oxyc(repo, "sync", "primary_database", "--workspace", WS);
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(r.stderr).toContain("nothing was written");
    expect(existsSync(join(repo, ".databases"))).toBe(false);
    expect(existsSync(join(parent, "escape.sql"))).toBe(false);
  });

  it("refuses to follow a .databases symlink out of the checkout", async () => {
    const outside = scratch();
    const repo = scratch();
    symlinkSync(outside, join(repo, ".databases"));
    const r = await oxyc(repo, "sync", "primary_database", "--workspace", WS);
    expect(r.status).toBe(ExitCode.REFUSED);
    expect(existsSync(join(outside, "primary_database"))).toBe(false);
  });
});

describe("pure helpers", () => {
  it("resolveInside accepts a .databases file and rejects the rest", () => {
    const root = scratch();
    expect("abs" in resolveInside(root, SQL)).toBe(true);
    expect("abs" in resolveInside(root, MODEL("t"))).toBe(true);
    for (const bad of ["", "a\0b", ".databases/./x/y", ".databases//x", "x/.databases/y/z"]) {
      expect("reason" in resolveInside(root, bad), bad).toBe(true);
    }
  });

  it("parseFiles takes only an array of {path, content}", () => {
    expect(parseFiles(JSON.stringify(BQ_FILES))).toEqual(BQ_FILES);
    expect(parseFiles("[]")).toEqual([]);
    expect(parseFiles('[{"path":1,"content":"x"}]')).toBeUndefined();
    expect(parseFiles('{"files":[]}')).toBeUndefined();
    expect(parseFiles("not json")).toBeUndefined();
  });

  it("plan prunes only models/ directories the response names", () => {
    const root = scratch();
    mkdirSync(join(root, ".databases/db/ds/models"), { recursive: true });
    mkdirSync(join(root, ".databases/db/other/models"), { recursive: true });
    writeFileSync(join(root, ".databases/db/ds/models/stale.schema.yml"), "x");
    writeFileSync(join(root, ".databases/db/other/models/untouched.schema.yml"), "x");
    writeFileSync(join(root, ".databases/db/notes.txt"), "x");
    const files = [{ path: ".databases/db/ds/models/t.schema.yml", content: "y" }];
    expect(plan(root, files, true)).toEqual([
      { path: ".databases/db/ds/models/t.schema.yml", action: "create" },
      { path: ".databases/db/ds/models/stale.schema.yml", action: "delete" }
    ]);
    expect(plan(root, files, true, true)).toEqual([
      { path: ".databases/db/ds/models/t.schema.yml", action: "create" }
    ]);
    expect(plan(root, files, false)).toEqual([
      { path: ".databases/db/ds/models/t.schema.yml", action: "create" }
    ]);
  });
});
