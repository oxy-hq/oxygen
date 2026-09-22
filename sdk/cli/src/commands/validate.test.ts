/**
 * `oxyc validate`, against the schemas the Rust types generate.
 *
 * The schema mapping is the part worth pinning: a file kind mapped to the
 * wrong schema validates cleanly against rules that do not apply to it, which
 * is worse than not validating at all.
 */

import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, describe, expect, it } from "vitest";
import { appWriterName, WRITER_NAME_MAX } from "../publish/manifest.js";
import { checkAppPlacement, type PlacementIssue } from "../publish/placement.js";
import { scanAirhouseMigration, secretsUsedAsState } from "../publish/placement-scan.js";
import { ExitCode } from "../util/errors.js";
import {
  formatsInSchemaPosition,
  listValidatable,
  schemaFor,
  walkable,
  whyUnchecked,
  whyUnreadable
} from "./validate.js";

const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

function workspace(files: Record<string, string>): string {
  const dir = mkdtempSync(join(tmpdir(), "oxyc-validate-"));
  SCRATCH.push(dir);
  for (const [path, content] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), content);
  }
  return dir;
}

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");
const SCHEMAS = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "json-schemas");

/**
 * Run the real binary, so the EXIT CODE is what is under test.
 *
 * `runValidate` throws a `CliError`; which number that becomes is decided by
 * the renderer in `main.ts`, and an agent branches on the number. Nothing but
 * the program answers that.
 */
function oxycValidate(cwd: string, args: string[], schemasDir = SCHEMAS, home?: string) {
  if (!existsSync(BIN)) throw new Error(`${BIN} missing — run \`pnpm build\``);
  const r = spawnSync(process.execPath, [BIN, "validate", ...args], {
    cwd,
    encoding: "utf8",
    env: {
      ...process.env,
      // `os.homedir()` reads $HOME on POSIX, so the home ceiling is reachable
      // from a test without going anywhere near the developer's real one.
      ...(home ? { HOME: home } : {}),
      OXYC_SCHEMAS_DIR: schemasDir,
      OXY_CREDENTIALS_PATH: join(BIN, "..", "__no_creds__.json"),
      OXYC_CACHE_DIR: join(BIN, "..", "__no_cache__"),
      OXY_TOKEN: "",
      NO_COLOR: "1"
    }
  });
  return { status: r.status ?? -1, stdout: r.stdout ?? "", stderr: r.stderr ?? "" };
}

/**
 * A schemas directory holding only the named files.
 *
 * This is how the `unchecked` path is reached without breaking the install: a
 * workspace file whose schema is absent here was NOT checked, and the whole
 * point of the accounting is that it must not be counted as valid.
 */
function partialSchemas(keep: string[]): string {
  const dir = mkdtempSync(join(tmpdir(), "oxyc-schemas-"));
  SCRATCH.push(dir);
  for (const name of keep) copyFileSync(join(SCHEMAS, name), join(dir, name));
  return dir;
}

/**
 * Is `mkfifo` available? Probed ONCE, at load.
 *
 * There is no Node API for a FIFO, and two cases below need one. Returning
 * early from the test body was the first attempt and it was wrong in the way
 * this branch has now caught twice: a body that returns is reported GREEN, so
 * a box without `mkfifo` showed two vacuous passes and a warning nobody
 * reads. `it.skipIf` marks them skipped, which is what the comment claimed.
 * The runner this must pass on is arm64 Linux; skipping elsewhere loses no
 * coverage that matters.
 */
const CAN_MKFIFO = (() => {
  const probe = mkdtempSync(join(tmpdir(), "oxyc-mkfifo-"));
  SCRATCH.push(probe);
  return spawnSync("mkfifo", [join(probe, "probe")], { encoding: "utf8" }).status === 0;
})();

function mkfifo(path: string): void {
  const r = spawnSync("mkfifo", [path], { encoding: "utf8" });
  if (r.status !== 0) throw new Error(`mkfifo failed: ${r.error?.message ?? r.stderr}`);
}

/** A workspace root — `findWorkspace` looks for `config.yml`, so it needs one. */
const MINIMAL_CONFIG = "databases: []\nmodels: []\n";

// ── data placement in oxy-app.json ──────────────────────────────────────────

/** An app directory: `oxy-app.json` from `manifest`, plus any other files. */
function app(manifest: unknown, files: Record<string, string> = {}): string {
  return workspace({ "oxy-app.json": JSON.stringify(manifest), ...files });
}

const errorsOf = (issues: PlacementIssue[]) => issues.filter((i) => i.level === "error");
const warningsOf = (issues: PlacementIssue[]) => issues.filter((i) => i.level === "warning");

/** A function whose source exists, so the secrets scan has something to read. */
const SYNC_SOURCE = { "functions/sync.ts": "export default async function sync() {}\n" };

describe("the writer an app slug derives", () => {
  /** Mirrors `app_writer_name` in crates/oltp/src/schema.rs, length cap included. */
  it("turns hyphens into underscores and refuses what cannot back a schema", () => {
    expect(appWriterName("store-ops")).toBe("store_ops");
    expect(appWriterName("bookings")).toBe("bookings");
    // An underscore would alias `my-app` onto the same schema and role.
    expect(appWriterName("my_app")).toBeUndefined();
    expect(appWriterName("1bad")).toBeUndefined();
    expect(appWriterName("Bad")).toBeUndefined();
    expect(appWriterName("")).toBeUndefined();
  });

  it("caps the writer at 63 bytes minus `app_` and `_rw`", () => {
    expect(WRITER_NAME_MAX).toBe(56);
    expect(appWriterName("a".repeat(WRITER_NAME_MAX))).toBe("a".repeat(WRITER_NAME_MAX));
    expect(appWriterName("a".repeat(WRITER_NAME_MAX + 1))).toBeUndefined();
  });
});

describe("customerWarehouseWrites", () => {
  const withWrites = (fn: Record<string, unknown>) =>
    app({ slug: "store-ops", functions: { sync: fn } }, SYNC_SOURCE);

  it("warns for a valid exception, naming function, database and reason", () => {
    const dir = withWrites({
      destinations: ["clickhouse"],
      customerWarehouseWrites: { clickhouse: "backfills the legacy rollup until Airway lands it" }
    });
    const issues = checkAppPlacement(dir, readManifest(dir), "store-ops");
    expect(errorsOf(issues)).toEqual([]);
    const [only, ...rest] = warningsOf(issues);
    expect(rest).toEqual([]);
    expect(only?.path).toBe("functions.sync.customerWarehouseWrites.clickhouse");
    for (const part of [
      "function `sync`",
      "`clickhouse`",
      "backfills the legacy rollup until Airway lands it",
      "read-only by default",
      "`ctx.airhouse`",
      "`ctx.oltp`"
    ]) {
      expect(only?.message).toContain(part);
    }
  });

  it("refuses anything but an object of reasons", () => {
    const dir = withWrites({
      destinations: ["clickhouse"],
      customerWarehouseWrites: ["clickhouse"]
    });
    const [only] = errorsOf(checkAppPlacement(dir, readManifest(dir), "store-ops"));
    expect(only?.message).toMatch(/must be an object mapping a database/);
  });

  it("refuses an empty or non-string reason", () => {
    const dir = withWrites({
      destinations: ["clickhouse", "snowflake"],
      customerWarehouseWrites: { clickhouse: "   ", snowflake: 42 }
    });
    const issues = checkAppPlacement(dir, readManifest(dir), "store-ops");
    expect(errorsOf(issues).map((i) => i.path)).toEqual([
      "functions.sync.customerWarehouseWrites.clickhouse",
      "functions.sync.customerWarehouseWrites.snowflake"
    ]);
    for (const e of errorsOf(issues)) expect(e.message).toMatch(/non-empty string/);
    expect(warningsOf(issues)).toEqual([]);
  });

  it("refuses an exception for a database missing from destinations, as dead config", () => {
    const dir = withWrites({
      destinations: ["airhouse"],
      customerWarehouseWrites: { clickhouse: "a real reason" }
    });
    const issues = checkAppPlacement(dir, readManifest(dir), "store-ops");
    const [only] = errorsOf(issues);
    expect(only?.message).toMatch(/not in this function's `destinations`/);
    expect(only?.message).toMatch(/dead config/);
    expect(warningsOf(issues)).toEqual([]);
  });

  it("says nothing about a function that declares no exception", () => {
    const dir = withWrites({ destinations: ["clickhouse"] });
    expect(checkAppPlacement(dir, readManifest(dir), "store-ops")).toEqual([]);
  });
});

describe("airhouse.enabled", () => {
  const gated = (airhouse: unknown, slug?: string) => {
    const dir = app({ ...(slug ? { slug } : {}), functions: { sync: { airhouse } } }, SYNC_SOURCE);
    return checkAppPlacement(dir, readManifest(dir), slug);
  };

  it("is an error when the slug contains an underscore, naming the rule", () => {
    const [only] = errorsOf(gated({ enabled: true }, "my_app"));
    expect(only?.path).toBe("functions.sync.airhouse.enabled");
    expect(only?.message).toMatch(/`app_<writer>`/);
    expect(only?.message).toMatch(/contains `_`/);
  });

  it("is an error when the derived writer is too long", () => {
    const [only] = errorsOf(gated({ enabled: true }, "a".repeat(WRITER_NAME_MAX + 1)));
    expect(only?.message).toMatch(/1-56 characters/);
  });

  it("is fine for a slug that derives a writer, or when the gate is off", () => {
    expect(gated({ enabled: true }, "store-ops")).toEqual([]);
    expect(gated({ enabled: false }, "my_app")).toEqual([]);
  });

  it("warns when no slug is known to derive the schema from", () => {
    const [only] = warningsOf(gated({ enabled: true }));
    expect(only?.path).toBe("slug");
    expect(only?.message).toMatch(/no slug is known/);
  });

  it("refuses a gate that is not an object", () => {
    const [only] = errorsOf(gated(true, "store-ops"));
    expect(only?.message).toMatch(/must be an object/);
  });
});

describe("airhouseMigrations", () => {
  const CLEAN = "CREATE TABLE app_store_ops.events (at TIMESTAMP, kind VARCHAR);\n";
  const check = (manifest: Record<string, unknown>, files: Record<string, string> = {}) => {
    const dir = app({ slug: "store-ops", ...manifest }, files);
    return checkAppPlacement(dir, readManifest(dir), (manifest.slug as string) ?? "store-ops");
  };

  it("requires dir", () => {
    const [only] = errorsOf(check({ airhouseMigrations: {} }));
    expect(only?.path).toBe("airhouseMigrations.dir");
    expect(only?.message).toMatch(/is required/);
  });

  it("refuses a directory that does not exist", () => {
    const [only] = errorsOf(check({ airhouseMigrations: { dir: "airhouse-migrations" } }));
    expect(only?.message).toMatch(/`airhouse-migrations` does not exist/);
  });

  it("refuses the OLTP migrations directory", () => {
    const [only] = errorsOf(
      check(
        { migrations: { dir: "migrations/" }, airhouseMigrations: { dir: "migrations" } },
        { "migrations/0001.sql": CLEAN }
      )
    );
    expect(only?.message).toMatch(/also `migrations.dir`/);
  });

  it("refuses a path that is not a plain relative directory", () => {
    const [only] = errorsOf(check({ airhouseMigrations: { dir: "../elsewhere" } }));
    expect(only?.message).toMatch(/not a safe path inside the bundle/);
  });

  it("refuses a directory with no .sql in it", () => {
    const [only] = errorsOf(
      check(
        { airhouseMigrations: { dir: "airhouse-migrations" } },
        {
          "airhouse-migrations/README.md": "later\n"
        }
      )
    );
    expect(only?.message).toMatch(/no `.sql` files/);
  });

  it("finds the directory under public/, which Vite copies into the bundle", () => {
    const issues = check(
      { airhouseMigrations: { dir: "airhouse-migrations" } },
      { "public/airhouse-migrations/0001_init.sql": CLEAN }
    );
    expect(issues).toEqual([]);
  });

  it("reports each SQL problem at its file and line", () => {
    const [only, ...rest] = check(
      { airhouseMigrations: { dir: "airhouse-migrations" } },
      {
        "airhouse-migrations/0001_init.sql": CLEAN,
        "airhouse-migrations/0002_orders.sql":
          "-- orders\n\nCREATE TABLE app_store_ops.orders (id INTEGER PRIMARY KEY);\n"
      }
    );
    expect(rest).toEqual([]);
    expect(only).toMatchObject({
      level: "error",
      file: "airhouse-migrations/0002_orders.sql",
      path: "line 3"
    });
  });

  it("is an error when the slug cannot derive the schema they run in", () => {
    const issues = check(
      { slug: "my_app", airhouseMigrations: { dir: "airhouse-migrations" } },
      { "airhouse-migrations/0001_init.sql": CLEAN }
    );
    expect(errorsOf(issues).map((i) => i.path)).toEqual(["airhouseMigrations"]);
  });
});

describe("scanning an Airhouse migration", () => {
  const SCHEMA = "app_store_ops";

  it.each([
    ["CREATE TABLE app_store_ops.t (id INTEGER PRIMARY KEY);", "PRIMARY KEY"],
    ["CREATE TABLE app_store_ops.t (id INTEGER UNIQUE);", "UNIQUE"],
    ["CREATE INDEX t_id ON app_store_ops.t (id);", "CREATE INDEX"],
    ["create unique index t_id on app_store_ops.t (id);", "CREATE UNIQUE INDEX"],
    ["CREATE TABLE app_store_ops.t (s INTEGER REFERENCES app_store_ops.s (id));", "REFERENCES"],
    [
      "ALTER TABLE app_store_ops.t ADD FOREIGN KEY (a) REFERENCES app_store_ops.s (id);",
      "FOREIGN KEY"
    ]
  ])("refuses %j, which DuckLake cannot carry", (sql, written) => {
    const hits = scanAirhouseMigration(sql, SCHEMA);
    // ONE hit: `CREATE UNIQUE INDEX` is not also a `UNIQUE`, and `FOREIGN KEY …
    // REFERENCES` is one constraint.
    expect(hits).toHaveLength(1);
    expect(hits[0]?.level).toBe("error");
    expect(hits[0]?.message).toContain(`\`${written}\` is not supported on Airhouse`);
    expect(hits[0]?.message).toMatch(/DuckLake has no primary keys/);
  });

  it("ignores those keywords in comments, strings and quoted identifiers", () => {
    const sql = [
      "-- a PRIMARY KEY would break this",
      "/* UNIQUE, REFERENCES,",
      "   CREATE INDEX /* nested */ FOREIGN KEY */",
      `CREATE TABLE app_store_ops.events ("unique" VARCHAR, note VARCHAR DEFAULT 'no primary key');`,
      "COMMENT ON TABLE app_store_ops.events IS $$append-only, REFERENCES nothing$$;"
    ].join("\n");
    expect(scanAirhouseMigration(sql, SCHEMA)).toEqual([]);
  });

  it("reports the line in the original text", () => {
    const sql = "/* one\n two\n three */\nCREATE TABLE app_store_ops.t (id INTEGER PRIMARY KEY);";
    expect(scanAirhouseMigration(sql, SCHEMA).map((h) => h.line)).toEqual([4]);
  });

  it("refuses an unqualified target and prints the schema to use", () => {
    const [only] = scanAirhouseMigration("CREATE TABLE orders (id INTEGER);", SCHEMA);
    expect(only?.message).toContain("`CREATE TABLE orders` is unqualified");
    expect(only?.message).toContain("write `app_store_ops.orders`");
  });

  it("refuses a target in another schema", () => {
    const [only] = scanAirhouseMigration("INSERT INTO raw_toast.orders SELECT 1;", SCHEMA);
    expect(only?.message).toContain("is outside the app's schema");
    expect(only?.message).toContain("write `app_store_ops.orders`");
  });

  it.each([
    "CREATE OR REPLACE VIEW v AS SELECT 1;",
    "CREATE TEMP TABLE t (x INTEGER);",
    "ALTER TABLE t ADD COLUMN x INTEGER;",
    "DROP TABLE IF EXISTS t;",
    "DROP VIEW v;",
    "INSERT INTO t VALUES (1);",
    "INSERT OR REPLACE INTO t VALUES (1);",
    "UPDATE t SET x = 1;",
    "DELETE FROM t;",
    "COMMENT ON TABLE t IS 'x';",
    "COMMENT ON COLUMN t.x IS 'y';"
  ])("checks the target of %j", (sql) => {
    const hits = scanAirhouseMigration(sql, SCHEMA);
    expect(hits).toHaveLength(1);
    expect(hits[0]?.message).toContain("write `app_store_ops.");
  });

  it.each([
    "CREATE TABLE IF NOT EXISTS app_store_ops.orders (id INTEGER);",
    'CREATE OR REPLACE TABLE "app_store_ops"."Orders" AS SELECT 1;',
    'INSERT INTO "APP_STORE_OPS".orders VALUES (1);',
    "insert into APP_STORE_OPS.orders values (1);",
    "UPDATE app_store_ops . orders SET x = 1;",
    'DELETE FROM "app_store_ops".orders WHERE id = 1;',
    "DROP TABLE app_store_ops.a, app_store_ops.b;",
    "COMMENT ON COLUMN app_store_ops.orders.id IS 'x';",
    "INSERT INTO app_store_ops.t SELECT * FROM raw_toast.orders ON CONFLICT DO UPDATE SET x = 1;"
  ])("accepts the qualified %j", (sql) => {
    expect(scanAirhouseMigration(sql, SCHEMA)).toEqual([]);
  });

  it("checks every name a DROP lists", () => {
    const hits = scanAirhouseMigration("DROP TABLE app_store_ops.a, b;", SCHEMA);
    expect(hits).toHaveLength(1);
    expect(hits[0]?.message).toContain("`DROP TABLE b`");
  });

  it("warns that CREATE SCHEMA is unnecessary, naming the schema", () => {
    const hits = scanAirhouseMigration("CREATE SCHEMA IF NOT EXISTS app_store_ops;", SCHEMA);
    expect(hits).toEqual([
      {
        line: 1,
        level: "warning",
        message:
          "`CREATE SCHEMA` is unnecessary — the platform creates `app_store_ops` before it applies migrations"
      }
    ]);
  });

  it("still refuses constraints when there is no schema to check targets against", () => {
    const hits = scanAirhouseMigration("CREATE TABLE t (id INTEGER PRIMARY KEY);", undefined);
    expect(hits.map((h) => h.message)).toEqual([expect.stringContaining("`PRIMARY KEY`")]);
  });
});

describe("ctx.secrets.set used as state", () => {
  it("flags a JSON.stringify value", () => {
    const hits = secretsUsedAsState(
      "export default async (ctx) => {\n  await ctx.secrets.set(KEY, JSON.stringify(state));\n};"
    );
    expect(hits).toEqual([
      { line: 2, call: "ctx.secrets.set(KEY, …)", why: "a `JSON.stringify(…)` value" }
    ]);
  });

  it.each(["SYNC_CURSOR", "last_run", "checkpoint", "progress", "row-count", "cache"])(
    "flags a key that names state: %s",
    (key) => {
      const [only] = secretsUsedAsState(`await ctx.secrets.set("${key}", value);`);
      expect(only?.why).toBe(`a key that names state (\`${key}\`)`);
    }
  );

  it.each(["QB_ACCOUNT_ID", "STATEMENT_EMAIL", "lastname"])(
    "does not read %s as state because a state word hides inside it",
    (key) => {
      expect(secretsUsedAsState(`await ctx.secrets.set("${key}", value);`)).toEqual([]);
    }
  );

  it("reads a camelCase key word by word", () => {
    const [only] = secretsUsedAsState('await ctx.secrets.set("syncCursor", value);');
    expect(only?.why).toBe("a key that names state (`syncCursor`)");
  });

  it("leaves a rotated credential alone, even stringified", () => {
    const source = [
      'await ctx.secrets.set("QB_REFRESH_TOKEN", JSON.stringify(tokens));',
      "await ctx.secrets.set('API_KEY', key);",
      "await ctx.secrets.set(`STATE_SECRET`, JSON.stringify(s));",
      'await ctx.secrets.set("LAST_PASSWORD", p);'
    ].join("\n");
    expect(secretsUsedAsState(source)).toEqual([]);
  });

  it("does not see a call in a comment or a string", () => {
    const source = [
      '// ctx.secrets.set("cursor", JSON.stringify(s))',
      "/* ctx.secrets.set('state', x) */",
      `const doc = "ctx.secrets.set('cursor', JSON.stringify(1))";`
    ].join("\n");
    expect(secretsUsedAsState(source)).toEqual([]);
  });

  it("leaves reads and computed keys with plain values alone", () => {
    const source = 'await ctx.secrets.get("cursor");\nawait ctx.secrets.set(keyFor(org), value);';
    expect(secretsUsedAsState(source)).toEqual([]);
  });

  it("is not thrown off by a regex literal holding a quote", () => {
    const source = 'const re = /"/;\nawait ctx.secrets.set("cursor", c);';
    expect(secretsUsedAsState(source).map((h) => h.line)).toEqual([2]);
  });

  it("reaches validate as a warning at the function's source line", () => {
    const dir = app(
      { slug: "store-ops", functions: { sync: { secrets: { write: true } } } },
      {
        "functions/sync.ts":
          'export default async (ctx) => {\n  await ctx.secrets.set("sync_cursor", c);\n};\n'
      }
    );
    const issues = checkAppPlacement(dir, readManifest(dir), "store-ops");
    expect(errorsOf(issues)).toEqual([]);
    const [only] = warningsOf(issues);
    expect(only).toMatchObject({ file: "functions/sync.ts", path: "line 2" });
    expect(only?.message).toContain(
      "function `sync` stores a key that names state (`sync_cursor`)"
    );
    expect(only?.message).toContain(
      "secrets hold credentials; state that changes belongs in `ctx.oltp` (records) or `ctx.airhouse` (facts)"
    );
  });

  it("says so when a function's source could not be scanned", () => {
    const dir = app({ slug: "store-ops", functions: { sync: {} } });
    const [only] = warningsOf(checkAppPlacement(dir, readManifest(dir), "store-ops"));
    expect(only?.message).toMatch(
      /not scanned for `ctx.secrets.set`: functions\/sync.ts could not be read \(ENOENT\)/
    );
  });
});

describe("oxyc validate on an oxy-app.json", () => {
  const BAD_MIGRATION = {
    "oxy-app.json": JSON.stringify({
      slug: "store-ops",
      airhouseMigrations: { dir: "airhouse-migrations" }
    }),
    "airhouse-migrations/0001_init.sql": "CREATE TABLE orders (id INTEGER PRIMARY KEY);\n"
  };

  it("fails the run on a placement error, and prints the schema to use", () => {
    const root = workspace(BAD_MIGRATION);
    const human = oxycValidate(root, []);
    expect(human.status).toBe(ExitCode.FAILURE);
    expect(human.stdout).toContain("2 problem(s) in 1 file(s)");

    const r = oxycValidate(root, ["--json"]);
    expect(r.status).toBe(ExitCode.FAILURE);
    const report = JSON.parse(r.stdout);
    // The document's shape does not move: warnings are stderr's, never a key.
    expect(Object.keys(report)).toEqual(["checked", "unchecked", "broken", "findings"]);
    expect(report.checked).toBe(1);
    expect(report.findings).toEqual([
      expect.objectContaining({
        file: "airhouse-migrations/0001_init.sql",
        path: "line 1",
        message: expect.stringContaining("`PRIMARY KEY` is not supported on Airhouse")
      }),
      expect.objectContaining({
        file: "airhouse-migrations/0001_init.sql",
        path: "line 1",
        message: expect.stringContaining("write `app_store_ops.orders`")
      })
    ]);
  });

  it("finds a nested app, and names its files from the workspace root", () => {
    const root = workspace(
      Object.fromEntries(
        Object.entries(BAD_MIGRATION).map(([k, v]) => [`apps/acme/store-ops/${k}`, v])
      )
    );
    const r = oxycValidate(root, ["--json"]);
    const files = JSON.parse(r.stdout).findings.map((f: { file: string }) => f.file);
    expect(files).toContain("apps/acme/store-ops/airhouse-migrations/0001_init.sql");
  });

  it("prints a warning on stderr without failing", () => {
    const root = app(
      {
        slug: "store-ops",
        functions: {
          sync: {
            destinations: ["clickhouse"],
            customerWarehouseWrites: { clickhouse: "legacy rollup" }
          }
        }
      },
      SYNC_SOURCE
    );
    const r = oxycValidate(root, []);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain("1 file(s) valid");
    expect(r.stderr).toMatch(
      /warning: oxy-app\.json \(functions\.sync\.customerWarehouseWrites\.clickhouse\)/
    );
    expect(r.stderr).toContain("read-only by default");
  });

  it("accepts --file oxy-app.json", () => {
    const root = app({ slug: "store-ops" });
    const r = oxycValidate(root, ["--file", "oxy-app.json"]);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain("1 file(s) valid");
    expect(r.stderr).not.toContain("warning:");
  });

  it("reports unparsable JSON as a finding rather than ending the walk", () => {
    const root = workspace({ "oxy-app.json": "{", "config.yml": MINIMAL_CONFIG });
    const r = oxycValidate(root, ["--json"]);
    expect(r.status).toBe(ExitCode.FAILURE);
    const report = JSON.parse(r.stdout);
    expect(report.checked).toBe(2);
    expect(report.findings).toEqual([
      expect.objectContaining({ file: "oxy-app.json", path: "(parse)" })
    ]);
  });
});

describe("oxyc validate lints an app's functions", () => {
  const LINT_FIXTURES = resolve(
    dirname(fileURLToPath(import.meta.url)),
    "..",
    "..",
    "fixtures",
    "function-lint"
  );

  /** A workspace holding one fixture app's files, so the run is on a copy. */
  function fixtureWorkspace(name: string): string {
    const files: Record<string, string> = {};
    const walk = (dir: string, prefix: string) => {
      for (const entry of readdirSync(join(dir, prefix), { withFileTypes: true })) {
        const rel = prefix ? `${prefix}/${entry.name}` : entry.name;
        if (entry.isDirectory()) walk(dir, rel);
        else files[rel] = readFileSync(join(dir, rel), "utf8");
      }
    };
    walk(join(LINT_FIXTURES, name), "");
    return workspace(files);
  }

  it("fails the run on a call whose capability the manifest lacks, naming file, line, call and fix", () => {
    const root = fixtureWorkspace("capability-violating");
    const human = oxycValidate(root, []);
    expect(human.status).toBe(ExitCode.FAILURE);
    expect(human.stdout).toContain("10 problem(s) in 1 file(s)");
    expect(human.stdout).toContain("functions/sync.ts");
    expect(human.stdout).toContain("line 3");
    expect(human.stdout).toContain("`ctx.secrets.set(` needs the `secrets.write` capability");
    expect(human.stdout).toContain(
      'add "secrets": { "write": true } to functions.sync in oxy-app.json'
    );

    const r = oxycValidate(root, ["--json"]);
    expect(r.status).toBe(ExitCode.FAILURE);
    const report = JSON.parse(r.stdout);
    expect(report.findings[0]).toEqual({
      file: "functions/sync.ts",
      path: "line 3",
      message: expect.stringMatching(/^\[function-lint\/capability\] `ctx\.secrets\.set\(`/)
    });
  });

  it("fails on a global the isolate does not have, and on a write outside destinations", () => {
    const globals = oxycValidate(fixtureWorkspace("globals-violating"), ["--json"]);
    expect(globals.status).toBe(ExitCode.FAILURE);
    const rules = (JSON.parse(globals.stdout).findings as Array<{ message: string }>).map(
      (f) => f.message.split("]")[0]
    );
    expect(new Set(rules)).toEqual(new Set(["[function-lint/isolate-global"]));

    const destinations = oxycValidate(fixtureWorkspace("destinations-violating"), []);
    expect(destinations.status).toBe(ExitCode.FAILURE);
    expect(destinations.stdout).toContain("declares no `destinations`");
    expect(destinations.stdout).toContain("not in `functions.other.destinations`");
  });

  it("passes a clean app, and says the engine half waits for publish", () => {
    const r = oxycValidate(fixtureWorkspace("destinations-clean"), []);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toContain("1 file(s) valid");
    expect(r.stderr).toContain("the engine half of the function lint");
    expect(r.stderr).toContain("skipped offline");
    expect(r.stderr).not.toContain("warning:");

    // No write, no note: a function that never touches a warehouse has nothing waiting.
    const quiet = oxycValidate(fixtureWorkspace("capability-clean"), []);
    expect(quiet.status, quiet.stderr).toBe(0);
    expect(quiet.stderr).not.toContain("engine half");
  });

  it("says which function it could not read, without failing the run", () => {
    const root = app({ slug: "store-ops", functions: { ghost: {} } });
    const r = oxycValidate(root, []);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stderr).toContain(
      "function `ghost` was not linted: functions/ghost.ts could not be read (ENOENT)"
    );
  });
});

function readManifest(dir: string): Record<string, unknown> {
  return JSON.parse(readFileSync(join(dir, "oxy-app.json"), "utf8"));
}

describe("schemaFor", () => {
  it("maps each file kind to its own schema", () => {
    expect(schemaFor("orders.automation.yml")).toBe("workflow.json");
    expect(schemaFor("analyst.agentic.yml")).toBe("agentic.json");
    expect(schemaFor("sales.app.yml")).toBe("app.json");
    expect(schemaFor("config.yml")).toBe("config.json");
  });

  /**
   * `.procedure.yml` and `.workflow.yml` are the retired spellings of
   * `.automation.yml`; the platform still accepts both, so a validator that
   * skipped them would report a workspace as clean without checking them.
   */
  it("accepts the retired automation spellings", () => {
    expect(schemaFor("x.procedure.yml")).toBe("workflow.json");
    expect(schemaFor("x.workflow.yml")).toBe("workflow.json");
  });

  it("ignores YAML it has no schema for", () => {
    expect(schemaFor("docker-compose.yml")).toBeUndefined();
    expect(schemaFor("semantics/views/orders.view.yml")).toBeUndefined();
  });

  /** `config.yml` is matched as a whole name, in any directory. */
  it("finds config.yml in a subdirectory", () => {
    expect(schemaFor("oxy/config.yml")).toBe("config.json");
  });

  /** …but not a file that merely ends in those letters. */
  it("does not match a lookalike name", () => {
    expect(schemaFor("myconfig.yml")).toBeUndefined();
    expect(schemaFor("notanapp.yml")).toBeUndefined();
  });
});

describe("the walk", () => {
  it("finds every validatable file and nothing else", () => {
    const dir = workspace({
      "config.yml": "x: 1",
      "flows/orders.automation.yml": "x: 1",
      "apps/sales.app.yml": "x: 1",
      "semantics/views/orders.view.yml": "x: 1",
      "README.md": "x"
    });
    expect(listValidatable(dir)).toEqual([
      "apps/sales.app.yml",
      "config.yml",
      "flows/orders.automation.yml"
    ]);
  });

  /**
   * A stray copy under a build directory is the same class of bug as the
   * semantic layer's "duplicate view name" errors — it reports problems about
   * a file nobody edits.
   */
  it("does not descend into build or vcs directories", () => {
    for (const skip of ["node_modules", "target", ".git", "dist", ".worktrees"]) {
      expect(walkable(skip), skip).toBe(false);
    }
    const dir = workspace({
      "config.yml": "x: 1",
      "node_modules/pkg/config.yml": "junk: true"
    });
    expect(listValidatable(dir)).toEqual(["config.yml"]);
  });
});

describe("the .yaml spelling", () => {
  /**
   * The first version mapped only `.yml`. A workspace authored in `.yaml` was
   * walked, matched nothing, and reported clean without a file being read —
   * the worst available answer, since it is the one a caller acts on. Monaco
   * validates both spellings, so the product already accepted them.
   */
  it("maps every kind under both spellings", () => {
    for (const [yml, yaml] of [
      ["x.automation.yml", "x.automation.yaml"],
      ["x.agentic.yml", "x.agentic.yaml"],
      ["x.app.yml", "x.app.yaml"],
      ["x.agent.test.yml", "x.agent.test.yaml"],
      ["config.yml", "config.yaml"]
    ]) {
      expect(schemaFor(yaml as string), yaml).toBe(schemaFor(yml as string));
      expect(schemaFor(yaml as string), yaml).toBeDefined();
    }
  });

  it("walks a .yaml file as readily as a .yml one", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "a.app.yaml": "x",
      "b.app.yml": "x"
    });
    expect(listValidatable(root)).toEqual(["a.app.yaml", "b.app.yml", "config.yml"]);
  });
});

describe("the verdict", () => {
  it("reports a clean workspace as valid, and exits 0", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    const r = oxycValidate(root, []);
    expect(r.status).toBe(0);
    expect(r.stdout).toMatch(/1 file\(s\) valid/);
  });

  /**
   * FINDINGS ARE A FAILURE, NOT A REQUEST ERROR. `REQUEST` (6) means the
   * deployment refused the call; nothing here talks to a deployment, and an
   * agent branching on 6 would go looking at the network for a YAML problem.
   */
  it("exits FAILURE on a violation, not REQUEST", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "broken.app.yml": "tasks: not-a-list\n"
    });
    const r = oxycValidate(root, []);
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.status).not.toBe(ExitCode.REQUEST);
    expect(r.stdout + r.stderr).toMatch(/broken\.app\.yml/);
  });

  it("reports a YAML syntax error as one finding, not as a crash", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "bad.app.yml": "tasks: [\n  unclosed\n"
    });
    const r = oxycValidate(root, []);
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stdout + r.stderr).toMatch(/bad\.app\.yml/);
  });
});

describe("--file", () => {
  it("checks one file and leaves the rest of the workspace alone", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      // `app.json` requires both `display` and `tasks`.
      "ok.app.yml": "display: []\ntasks: []\n",
      "broken.app.yml": "tasks: not-a-list\n"
    });
    const r = oxycValidate(root, ["--file", "ok.app.yml"]);
    expect(r.status).toBe(0);
    expect(r.stdout + r.stderr).not.toMatch(/broken\.app\.yml/);
  });

  /**
   * A SYMLINK KEEPS ITS OWN NAME. `schemaFor` decides a file's kind from its
   * basename, so resolving the argument through the link changed what the file
   * IS — `ln -s tpl/base.yml my.app.yml` became `base.yml`, matched nothing,
   * and `--file` rejected a file the whole-workspace walk validates happily.
   * Both directions are asserted here, because the bug was the DISAGREEMENT:
   * either answer alone looks reasonable.
   *
   * ONLY THE FIRST HALF IS A GUARD. Restoring `physical()` around the argument
   * turns `--file` red on both its assertions; making the walk disagree would
   * take resolving names inside it, which no one-token change does. The `all`
   * half is here to state what the right answer is, not to catch a regression —
   * worth knowing before anyone trims it as redundant.
   */
  it("decides a symlinked file's kind the same way the walk does", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "tpl/base.yml": "display: []\ntasks: []\n"
    });
    symlinkSync(join(root, "tpl", "base.yml"), join(root, "my.app.yml"), "file");

    const one = oxycValidate(root, ["--file", "my.app.yml"]);
    expect(one.status, one.stderr).toBe(0);
    expect(one.stderr).not.toMatch(/does not know how to check/);

    // `tpl/base.yml` is not a validatable kind, so the walk checks config.yml
    // and the link — under the link's name, via `Dirent`, which does not follow.
    const all = oxycValidate(root, []);
    expect(all.status, all.stderr).toBe(0);
    expect(all.stdout).toMatch(/2 file\(s\) valid/);
  });

  /**
   * A typo'd path is NOT_FOUND (5), never a parse error. Reading it outside
   * the YAML try is what buys that: reported as a parse failure it said
   * "ENOENT: no such file or directory", which sends the reader to look
   * inside a file for a problem that is its absence.
   */
  it("says NOT_FOUND for a path that does not exist", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    const r = oxycValidate(root, ["--file", "typo.app.yml"]);
    expect(r.status).toBe(ExitCode.NOT_FOUND);
    expect(r.stderr).toMatch(/no such file/);
    expect(r.stderr).not.toMatch(/ENOENT/);
  });

  /** A kind with no schema is a USAGE error that lists what it does check. */
  it("says USAGE, and what it does check, for an unknown kind", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG, "notes.yml": "a: 1\n" });
    const r = oxycValidate(root, ["--file", "notes.yml"]);
    expect(r.status).toBe(ExitCode.USAGE);
    expect(r.stderr).toMatch(/\.app\.yml/);
  });
});

describe("a filesystem condition is never a parse error", () => {
  /**
   * THE RULE, STATED ONCE FOR BOTH BRANCHES. The walk skips a directory link
   * because `apps.app.yml` naming a directory reaches `readFileSync` and comes
   * back `EISDIR`, which the YAML try then reports as a `(parse)` finding —
   * sending the reader to look INSIDE a file for a problem that is the file
   * being a directory. `--file` reached the same `readFileSync`, because
   * `existsSync` is true for a directory, and kept the exception.
   */
  it("refuses --file naming a real directory, rather than reporting EISDIR", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    mkdirSync(join(root, "plain.app.yml"));

    const r = oxycValidate(root, ["--file", "plain.app.yml"]);
    // FAILURE, not NOT_FOUND: a directory named `x.app.yml` is emphatically
    // there, and 5 tells a caller branching on the number that it is not.
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stderr).toMatch(/not a file/);
    expect(r.stderr).toMatch(/it is a directory/);
    expect(r.stdout + r.stderr).not.toMatch(/EISDIR/);
    expect(r.stdout + r.stderr).not.toMatch(/\(parse\)/);
  });

  it("refuses --file naming a link to a directory the same way", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    const target = workspace({ "unrelated.md": "a real directory\n" });
    symlinkSync(target, join(root, "apps.app.yml"), "dir");

    const r = oxycValidate(root, ["--file", "apps.app.yml"]);
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stdout + r.stderr).not.toMatch(/EISDIR/);
  });
});

describe("why a link could not be followed", () => {
  /**
   * THE REASON IS CHECKED, NOT ASSUMED. Every `stat` failure used to print
   * "the target is gone", which is true for ENOENT and a claim about a
   * condition nobody established for the rest. A self-referential link throws
   * ELOOP — worth naming on a walk that deliberately avoids needing cycle
   * detection, since "the target is gone" sends you looking for a target.
   */
  it("names a link cycle as a cycle, not as a missing target", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    symlinkSync("loop.app.yml", join(root, "loop.app.yml"), "file");

    const r = oxycValidate(root, []);
    expect(r.status, r.stderr).toBe(0);
    // One shared reason goes on the headline, so path and reason are on
    // separate lines — asserted independently, not as one concatenation.
    expect(r.stderr).toMatch(/the link points at itself, or round a cycle/);
    expect(r.stderr).toMatch(/loop\.app\.yml/);
    expect(r.stderr).not.toMatch(/the target is gone/);
  });

  it("carries the code into --json so a caller can branch on it", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    symlinkSync("loop.app.yml", join(root, "loop.app.yml"), "file");

    const r = oxycValidate(root, ["--json"]);
    expect(JSON.parse(r.stdout).broken).toEqual([{ path: "loop.app.yml", code: "ELOOP" }]);
  });
});

describe("both branches answer one condition the same way", () => {
  /**
   * `--file` threw `no such file` for EVERY stat failure, so a self-referential
   * link — which `ls` shows you — was reported as not existing, while the walk
   * branch named it a cycle. Two contradictory answers to one condition, and
   * the `--file` one was the inaccurate half.
   */
  it("names a link cycle as a cycle under --file, not as a missing file", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    symlinkSync("loop.app.yml", join(root, "loop.app.yml"), "file");

    const r = oxycValidate(root, ["--file", "loop.app.yml"]);
    // The code has to agree with the message. 5 means the path does not exist
    // — returning it here tells an agent the opposite of what it just read.
    // FAILURE, not NOT_FOUND — 5 would tell a caller branching on the number
    // the opposite of what the message beside it says. (`toBe` already
    // forecloses 5; a second `not.toBe` would read as a guard without being
    // one, which this branch has removed several of.)
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stderr).toMatch(/cannot read loop\.app\.yml/);
    expect(r.stderr).toMatch(/points at itself, or round a cycle/);
    expect(r.stderr).not.toMatch(/no such file/);
  });

  /** A genuinely absent path keeps the message that fits it. */
  it("still says no such file for a path that is actually absent", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    const r = oxycValidate(root, ["--file", "typo.app.yml"]);
    expect(r.status).toBe(ExitCode.NOT_FOUND);
    expect(r.stderr).toMatch(/no such file/);
  });

  /**
   * THE LAST DOOR TO THE WHOLE-RUN ABORT. A FIFO named `x.app.yml` is not a
   * `Dirent` directory, has a `.yml` name, matches `schemaFor`, and is not a
   * symlink — so it cleared every arm, reached `validateFile`, and took its
   * `not a file` throw and every other file with it. `walk` now emits regular
   * files and links to them, full stop.
   */
  it.skipIf(!CAN_MKFIFO)("does not let a FIFO named .app.yml abort the walk", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "good.app.yml": "display: []\ntasks: []\n"
    });
    mkfifo(join(root, "pipe.app.yml"));

    const r = oxycValidate(root, []);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toMatch(/2 file\(s\) valid/);
    expect(r.stderr).toMatch(/not a regular file/);
    expect(r.stderr).toMatch(/pipe\.app\.yml/);
  });

  /**
   * A link to a FIFO clears the directory arm the same way a link to a regular
   * file does, so the stat has to ask `isFile()` and not merely
   * `!isDirectory()` — otherwise the link is emitted and `validateFile`'s
   * `not a file` throw aborts the run through the symlink door instead.
   */
  it.skipIf(!CAN_MKFIFO)("does not let a link to a FIFO abort the walk either", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "good.app.yml": "display: []\ntasks: []\n"
    });
    const pipes = workspace({});
    mkfifo(join(pipes, "pipe"));
    symlinkSync(join(pipes, "pipe"), join(root, "linked.app.yml"), "file");

    const r = oxycValidate(root, []);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toMatch(/2 file\(s\) valid/);
    expect(r.stderr).toMatch(/not a regular file/);
    expect(r.stderr).toMatch(/linked\.app\.yml/);
  });

  /** And an errno the reader should not have to look up gets a sentence. */
  it("explains ENOTDIR rather than printing the code", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    symlinkSync("config.yml/nested", join(root, "x.app.yml"), "file");

    const r = oxycValidate(root, []);
    // Collected, not emitted — so the run continues, which is the whole point.
    expect(r.status, r.stderr).toBe(0);
    expect(r.stderr).toMatch(/a component of the path is a file/);
    expect(r.stderr).toMatch(/x\.app\.yml/);
    expect(r.stderr).not.toMatch(/— ENOTDIR/);
  });
});

describe("--json always emits a document", () => {
  /**
   * An empty stdout makes `oxyc validate --json | jq` fail on the one workspace
   * shape whose answer is simply "nothing to check" — the early return sat
   * above the `--json` block, so the promise of a document had an exception
   * exactly where a script is least able to handle one.
   */
  it("emits an empty result rather than nothing, for a workspace with no YAML", () => {
    const root = workspace({ "notes.md": "nothing here\n" });
    const r = oxycValidate(root, ["--json"]);
    expect(r.status).toBe(0);
    expect(JSON.parse(r.stdout)).toEqual({
      checked: 0,
      unchecked: [],
      broken: [],
      findings: []
    });
  });
});

describe("files it could not check", () => {
  /**
   * THE HEADLINE FIX. `validateFile` returns `[]` for "checked, no findings"
   * and `undefined` for "no schema for this in this installation" — collapsing
   * the two printed "N file(s) valid" over files nothing had read.
   */
  it("names them instead of counting them as valid", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "dash.app.yml": "tasks: []\n"
    });
    // `app.json` withheld: `dash.app.yml` maps to a schema this install lacks.
    const r = oxycValidate(root, [], partialSchemas(["config.json"]));
    expect(r.stderr).toMatch(/1 file\(s\) NOT checked/);
    expect(r.stderr).toMatch(/dash\.app\.yml/);
    // And the verdict counts only what was actually read.
    expect(r.stdout).toMatch(/1 file\(s\) valid/);
    expect(r.stdout).not.toMatch(/2 file\(s\) valid/);
  });

  it("reports the same split in --json", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "dash.app.yml": "tasks: []\n"
    });
    const r = oxycValidate(root, ["--json"], partialSchemas(["config.json"]));
    expect(JSON.parse(r.stdout)).toEqual({
      checked: 1,
      unchecked: [{ path: "dash.app.yml", code: "SCHEMA_MISSING" }],
      broken: [],
      findings: []
    });
  });
});

describe("symlinked directories in the walk", () => {
  /**
   * THE ARM THAT HAD NO TEST. `Dirent` does not follow links, so a link to a
   * directory answers `false` to `isDirectory()` and falls through to the
   * extension check exactly as a linked FILE does. Named `apps.app.yml` it
   * passed that check, matched `.app.yml`, and reached `readFileSync` — which
   * reported `EISDIR: illegal operation on a directory` as a YAML PARSE
   * finding. Same wrong answer the `NOT_FOUND` handling exists to prevent, in
   * through the other door.
   */
  it("skips a directory link even when its name says .app.yml", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    const target = workspace({ "unrelated.md": "a real directory\n" });
    symlinkSync(target, join(root, "apps.app.yml"), "dir");

    const r = oxycValidate(root, ["--json"]);
    expect(r.status, r.stderr).toBe(0);
    expect(JSON.parse(r.stdout)).toEqual({
      checked: 1,
      unchecked: [],
      broken: [],
      findings: []
    });
    expect(r.stdout).not.toMatch(/EISDIR/);
  });

  /**
   * A link INSIDE the root costs nothing: the walk reaches the target by its
   * real path, so the file is checked — just under the name it really has.
   * This is the half that makes "silently invisible" too strong a description.
   *
   * NOT A GUARD. The link is named `apps`, so it never had an extension to
   * clear — before the fix it was skipped at `extname`, after it at the stat,
   * and the counts agree either way. This states the intent; the `.app.yml`
   * case above is what a mutation kills.
   */
  it("still checks a link's target when the target is inside the root", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "shared/x.app.yml": "display: []\ntasks: []\n"
    });
    symlinkSync(join(root, "shared"), join(root, "apps"), "dir");

    const r = oxycValidate(root, ["--json"]);
    expect(r.status, r.stderr).toBe(0);
    // config.yml + shared/x.app.yml — once, not twice, and not zero times.
    expect(JSON.parse(r.stdout).checked).toBe(2);
  });

  /**
   * A link pointing OUTSIDE the root is the one shape genuinely missed, and
   * `--file` is the escape hatch for it. Both halves asserted, because the
   * claim being pinned is the DIFFERENCE between the two branches — which is
   * written down nowhere else. Like the case above this is a statement rather
   * than a guard: the link has no extension, so both versions skip it.
   */
  it("does not walk a link out of the root, though --file reads through it", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    const outside = workspace({ "x.app.yml": "display: []\ntasks: []\n" });
    symlinkSync(outside, join(root, "apps"), "dir");

    const walked = oxycValidate(root, ["--json"]);
    expect(JSON.parse(walked.stdout).checked).toBe(1);

    const direct = oxycValidate(root, ["--file", "apps/x.app.yml"]);
    expect(direct.status, direct.stderr).toBe(0);
    expect(direct.stdout).toMatch(/1 file\(s\) valid/);
  });
});

describe("a broken symlink in the walk", () => {
  /**
   * ONE STALE LINK USED TO ABORT THE RUN. `walk` emitted it, `validateFile`'s
   * `existsSync` follows links and answered false, and the `NOT_FOUND` throw —
   * written for `--file`, where "no such file" answers a typo'd argument —
   * stopped the whole workspace at exit 5 with nothing said about any other
   * file. A `models/x.view.yml` pointing into a removed worktree was enough.
   */
  it("does not stop the run, and still checks every other file", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "good.app.yml": "display: []\ntasks: []\n"
    });
    symlinkSync(join(root, "gone", "target.yml"), join(root, "stale.app.yml"), "file");

    const r = oxycValidate(root, []);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toMatch(/2 file\(s\) valid/);
  });

  /** And it is NAMED — a file nothing read must never pass as valid. */
  it("names the link rather than passing over it", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    symlinkSync(join(root, "gone", "target.yml"), join(root, "stale.app.yml"), "file");

    const r = oxycValidate(root, []);
    // One entry, so the reason REPLACES the category descriptor rather than
    // chaining after it — `… — could not read them — the target is gone` was
    // two clauses saying one thing.
    // The headline carries the reason (one entry, so it hoists); the path is
    // listed under it. No third assertion — `/the target is gone/` alone
    // cannot fail unless the headline match above already has.
    expect(r.stderr).toMatch(/1 file\(s\) NOT checked — the target is gone/);
    expect(r.stderr).toMatch(/stale\.app\.yml/);
    // NO REMEDY ON THIS BUCKET, so no separator either. `listSkipped` spreads a
    // possibly-empty array into `log.remedy`, which makes that function's
    // emptiness guard reachable — without it this run ends on two stray blank
    // lines, and nothing else here would notice.
    expect(r.stderr).not.toMatch(/\n\n/);
  });

  it("reports it in --json, apart from unchecked — the fix differs", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    symlinkSync(join(root, "gone", "target.yml"), join(root, "stale.app.yml"), "file");

    const r = oxycValidate(root, ["--json"]);
    expect(JSON.parse(r.stdout)).toEqual({
      checked: 1,
      unchecked: [],
      broken: [{ path: "stale.app.yml", code: "ENOENT" }],
      findings: []
    });
  });

  /**
   * `--file` KEEPS THE THROW — the abort was only ever wrong on the branch that
   * enumerated the file itself. But a DANGLING LINK is not an absent path:
   * `statSync` throws `ENOENT` for both, and answering "no such file" for a
   * link `ls` shows you is the same wrong answer the walk arm stopped giving.
   * `lstat` separates them, so this one reads like the walk's.
   */
  it("names a stale link as a stale link under --file, and does not claim it is absent", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    symlinkSync(join(root, "gone", "target.yml"), join(root, "stale.app.yml"), "file");

    const r = oxycValidate(root, ["--file", "stale.app.yml"]);
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stderr).toMatch(/cannot read stale\.app\.yml/);
    expect(r.stderr).toMatch(/the target is gone/);
    expect(r.stderr).not.toMatch(/no such file/);

    // HALF THE RENDER ORDER IS OBSERVABLE, AND ONLY HALF. `reportAndExit`
    // prints detail, then hint, then remedy. This error is the one run in the
    // suite carrying TWO of the three — `detail: "the target is gone"` and
    // `hint: WHOLE_WORKSPACE_HINT` — so it pins `detail → hint`.
    //
    // A first attempt pinned this from `--file typo.app.yml`, which carries a
    // HINT ALONE: it asserted that a hint prints below the error line, which
    // was never in question, and swapping the hint and remedy blocks left it
    // green. `hint → remedy` still has no producer — no `CliError` in the tool
    // carries both — so that adjacency is unpinned, and saying so beats
    // pointing at a run that cannot show it.
    // No `> -1` guard: `toMatch(/the target is gone/)` above already has it, and
    // a missing HINT makes the right-hand side `-1`, which `toBeLessThan` fails
    // on its own. An assertion foreclosed by one above it reads as a guard
    // without being one — the third this branch has removed for that.
    expect(r.stderr.indexOf("the target is gone")).toBeLessThan(
      r.stderr.indexOf("to check the whole workspace")
    );
  });

  /** A path that is genuinely absent still gets NOT_FOUND, which is 5's job. */
  it("keeps NOT_FOUND for an argument that names nothing at all", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    const r = oxycValidate(root, ["--file", "typo.app.yml"]);
    expect(r.status).toBe(ExitCode.NOT_FOUND);
    expect(r.stderr).toMatch(/no such file/);
  });
});

describe("an error's remedy", () => {
  /**
   * THE ERROR PATH'S HALF OF THE SPLIT. `CliError.hint` renders through
   * `log.hint`, so a remedy handed to it printed as `→ …` — an instruction in
   * the run of elaborations, which is exactly what `log.remedy` exists to
   * prevent. `CliError.remedy` is the channel; this is its first producer, and
   * without one the wiring would have been written and never exercised.
   */
  it("prints set apart, not under the elaboration marker", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    const noSchemas = mkdtempSync(join(tmpdir(), "oxyc-noschemas-"));
    SCRATCH.push(noSchemas);

    const r = oxycValidate(root, [], noSchemas);
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stderr).toMatch(/no JSON Schemas are available/);
    // THE CHANNEL, NOT THE TEXT. Which sentence prints depends on `inCheckout`,
    // which resolves off `tmpdir()` — `/json-schemas` on Linux, but a `TMPDIR`
    // inside a checkout flips it to "run `pnpm build`", and an assertion on
    // "reinstall" then goes quiet with the negative one passing vacuously.
    expect(r.stderr).toMatch(/\n\n {2}fix: /);
    // DERIVED FROM WHAT PRINTED, because two previous versions of this line
    // enumerated the `inCheckout` texts and were broken by the next rewording —
    // and `/→ .*pnpm/` was only the third guess at a phrase that would hold
    // (`npm i -g` is the obvious next wording, and would break it again).
    // Reading the remedy back out of the output cannot go stale, and unlike a
    // flat `not.toMatch(/→/)` it still works once this error has a hint too.
    const printed = r.stderr.match(/fix: (.*)/)?.[1];
    expect(printed).toBeDefined();
    expect(r.stderr).not.toContain(`→ ${printed}`);
  });
});

describe("why a file was not checked", () => {
  /**
   * TWO PRODUCERS, TWO REMEDIES. The count line used to carry ONE reason for
   * both, so every skipped file was told to reinstall — right for a schema
   * absent from the installation, useless for a kind `oxyc` maps nothing to,
   * where no reinstall produces one.
   *
   * The rule now: a reason the whole bucket SHARES goes on the count, and a
   * bucket that disagrees gets one per file. This case is one file, so it
   * hoists — which makes it an instance of the shared arm, not the per-file
   * one an earlier version of this comment described.
   */
  it("tells a file with a missing schema to reinstall", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "dash.app.yml": "display: []\ntasks: []\n"
    });
    const r = oxycValidate(root, [], partialSchemas(["config.json"]));
    expect(r.stderr).toMatch(/the schema is not in this installation/);
    expect(r.stderr).toMatch(/dash\.app\.yml/);
    expect(r.stderr).toMatch(/OXYC_SCHEMAS_DIR/);
  });

  /** And the code reaches `--json`, so a caller acts on the reason. */
  it("carries the reason into --json, the way broken already did", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "dash.app.yml": "display: []\ntasks: []\n"
    });
    const r = oxycValidate(root, ["--json"], partialSchemas(["config.json"]));
    expect(JSON.parse(r.stdout).unchecked).toEqual([
      { path: "dash.app.yml", code: "SCHEMA_MISSING" }
    ]);
  });

  /**
   * ONE SHARED REASON IS PRINTED ONCE. Moving the reason per file fixed a
   * message asserting one producer's remedy for both — and then repeated a
   * long sentence once per path, which reads worse than the bug did. Counted
   * rather than matched, because "it appears" is true under both behaviours.
   */
  it("prints a reason the whole bucket shares exactly once", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    symlinkSync(join(root, "gone-a.yml"), join(root, "a.app.yml"), "file");
    symlinkSync(join(root, "gone-b.yml"), join(root, "b.app.yml"), "file");

    const r = oxycValidate(root, []);
    expect(r.stderr.match(/the target is gone/g) ?? []).toHaveLength(1);
    // The paths are still all listed — only the reason was hoisted.
    expect(r.stderr).toMatch(/a\.app\.yml/);
    expect(r.stderr).toMatch(/b\.app\.yml/);
  });

  /**
   * AND WHEN THEY DIFFER, EACH PATH CARRIES ITS OWN. This is the case the
   * per-file wording exists for, and the one a shared headline cannot serve.
   */
  it("puts a reason beside each path when the bucket disagrees", () => {
    const root = workspace({ "config.yml": MINIMAL_CONFIG });
    symlinkSync(join(root, "gone.yml"), join(root, "stale.app.yml"), "file");
    symlinkSync("loop.app.yml", join(root, "loop.app.yml"), "file");

    const r = oxycValidate(root, []);
    expect(r.stderr).toMatch(/stale\.app\.yml — the target is gone/);
    expect(r.stderr).toMatch(/loop\.app\.yml — the link points at itself/);
    // …and the headline stays a bare count, with no reason hoisted onto it.
    expect(r.stderr).toMatch(/2 file\(s\) NOT checked — could not read them\n/);
  });

  /**
   * `KIND_UNKNOWN` CANNOT BE PRODUCED BY A RUN — `walk` filters on
   * `!schemaFor(rel)` before emitting and `--file` throws USAGE on the same
   * predicate — so its message is pinned here directly. Without this, a
   * mutation swapping the two codes passes the whole suite, and the remedy
   * that ships the day a filter moves is the wrong one.
   */
  it("does not tell an unmapped kind to reinstall", () => {
    expect(whyUnchecked("SCHEMA_MISSING")).toEqual({
      reason: "the schema is not in this installation",
      remedy: "reinstall, or set OXYC_SCHEMAS_DIR at a checkout's json-schemas/"
    });

    // NO REMEDY AT ALL, which is the asymmetry the split exists for: no
    // reinstall produces a mapping oxyc does not have, so there is nothing to
    // hint and the field is absent rather than filled with the other one's.
    expect(whyUnchecked("KIND_UNKNOWN").reason).toMatch(/no schema for this kind/);
    expect(whyUnchecked("KIND_UNKNOWN").remedy).toBeUndefined();
  });

  /**
   * A CLEAN RUN SAYS NOTHING ABOUT SKIPS. The emptiness guard moved into
   * `listSkipped` when the call sites stopped checking, so it is now the only
   * thing standing between a spotless workspace and two `0 file(s) NOT
   * checked` warnings. Nothing asserted that until this — and the guard is
   * exactly the kind of term this branch has twice deleted for being inert,
   * which it would have looked like without a case that goes red.
   */
  it("warns about nothing when every file was checked", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "ok.app.yml": "display: []\ntasks: []\n"
    });
    const r = oxycValidate(root, []);
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).toMatch(/2 file\(s\) valid/);
    expect(r.stderr).not.toMatch(/NOT checked/);
    expect(r.stderr).not.toMatch(/0 file\(s\)/);
  });

  /**
   * THE ARMS A RUN CANNOT REACH. `EACCES`/`EPERM` needs `chmod` or root and
   * `EUNKNOWN` needs a throw carrying no code at all — neither happens on CI,
   * so swapping their strings passed everything. Same gap, and same remedy, as
   * `KIND_UNKNOWN` above: assert the mapping directly.
   */
  it("explains the codes a run cannot produce", () => {
    expect(whyUnreadable("EACCES").reason).toMatch(/permission denied/);
    expect(whyUnreadable("EPERM").reason).toMatch(/permission denied/);
    expect(whyUnreadable("EUNKNOWN").reason).toMatch(/without saying why/);
    // And an errno nobody mapped falls through to itself — the fallback
    // `whyUnchecked` deliberately does NOT have, because OS codes are
    // open-ended and ours are two constants.
    expect(whyUnreadable("EMFILE")).toEqual({ reason: "EMFILE" });
  });

  /**
   * THE REMEDY IS A HINT, NOT PART OF THE REASON. Folded together, the only
   * reachable shared headline ran to 140 characters with two em dashes, the
   * second half of which was a remedy rather than a reason — the exact shape
   * the shared-reason rule had just been written to remove, surviving in the
   * one case a user actually hits.
   */
  it("puts the remedy on its own line, once, below the paths", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "a.app.yml": "display: []\ntasks: []\n",
      "b.app.yml": "display: []\ntasks: []\n"
    });
    const r = oxycValidate(root, [], partialSchemas(["config.json"]));

    expect(r.stderr).toMatch(/2 file\(s\) NOT checked — the schema is not in this installation\n/);
    // Once for two files, and not chained onto the headline.
    expect(r.stderr.match(/OXYC_SCHEMAS_DIR/g) ?? []).toHaveLength(1);
    expect(r.stderr).not.toMatch(/installation — reinstall/);
    // BELOW THE PATHS — the half the test's name promises, and the half the
    // three assertions above hold identically whether or not it is true.
    expect(r.stderr.indexOf("OXYC_SCHEMAS_DIR")).toBeGreaterThan(r.stderr.indexOf("b.app.yml"));
    // And under its own marker: `→` elaborates the line above, so a remedy
    // wearing it reads in sequence with the paths rather than apart from them.
    expect(r.stderr).toMatch(/fix: reinstall/);
    // SEPARATED ON BOTH SIDES. With a blank only above, the line attached
    // downward to whatever followed — and on the `--file` path that is a
    // `log.error` with no leading blank, so `fix:` grouped with an unrelated
    // error instead of with the warning it belongs to.
    expect(r.stderr).toMatch(/\n\n {2}fix: reinstall[^\n]*\n\n/);
  });

  /**
   * THE CAP ANNOUNCES ITSELF. `11 file(s) NOT checked` over ten listed paths
   * reads as a display bug rather than a cap.
   */
  it("says how many it elided when there are more than ten", () => {
    const files: Record<string, string> = { "config.yml": MINIMAL_CONFIG };
    for (let i = 0; i < 12; i++) files[`a${i}.app.yml`] = "display: []\ntasks: []\n";
    const r = oxycValidate(workspace(files), [], partialSchemas(["config.json"]));
    expect(r.stderr).toMatch(/12 file\(s\) NOT checked/);
    expect(r.stderr).toMatch(/… and 2 more/);
  });
});

describe("nothing read is not success", () => {
  /**
   * THE SHARP ONE. The caller named exactly one file, its schema was absent
   * from the installation, so it was never opened — and the command answered
   * exit 0. There is no partial result to defend here, and `util/errors.ts`
   * exists precisely so "printed an error and exited 0" is unrepresentable.
   * The word was fixed a round earlier; the number kept saying `valid`.
   */
  it("fails when --file named a file whose schema this installation lacks", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "dash.app.yml": "display: []\ntasks: []\n"
    });
    const r = oxycValidate(root, ["--file", "dash.app.yml"], partialSchemas(["config.json"]));
    // `status` IS THE GUARD. The two below pass under the bug as well — the
    // pre-fix stdout was `nothing checked`, which contains no "valid", and the
    // warning was already correct. They state the surrounding behaviour; the
    // number is the half that was wrong.
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stderr).toMatch(/NOT checked/);
    expect(r.stdout).not.toMatch(/valid/);

    // THE GROUPING CASE THE SEPARATOR WAS REPORTED FOR: this is the run where
    // an `error:` follows the remedy immediately, and a blank only above left
    // `fix:` closer to that error than to the warning it belongs to. The
    // whole-workspace test that asserts the spacing has nothing after it, so
    // its trailing `\n\n` is satisfied by end-of-stderr.
    expect(r.stderr).toMatch(/fix: reinstall[^\n]*\n\nerror:/);
  });

  it("fails when a whole workspace was walked and nothing in it could be read", () => {
    const root = workspace({
      "a.app.yml": "display: []\ntasks: []\n",
      "b.app.yml": "display: []\ntasks: []\n"
    });
    const r = oxycValidate(root, [], partialSchemas(["config.json"]));
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stderr).toMatch(/2 file\(s\) NOT checked/);
  });

  it("reports the same verdict under --json", () => {
    const root = workspace({ "a.app.yml": "display: []\ntasks: []\n" });
    const r = oxycValidate(root, ["--json"], partialSchemas(["config.json"]));
    expect(r.status).toBe(ExitCode.FAILURE);
    // The document is still emitted — the exit code is the only thing that moved.
    expect(JSON.parse(r.stdout).unchecked).toEqual([{ path: "a.app.yml", code: "SCHEMA_MISSING" }]);
  });

  /**
   * AND THE OTHER SIDE. A workspace with nothing to check is not a failure.
   *
   * What protects it is the EARLY RETURN, which fires before the verdict is
   * reached — not the `unchecked + broken > 0` conjunct, which is unreachable
   * today and which a mutation can remove without failing this. Said plainly
   * because a case that looks like it pins a term it does not is the thing
   * several rounds of this branch have been spent deleting.
   */
  it("still exits 0 for a workspace that genuinely holds nothing", () => {
    const root = workspace({ "notes.md": "nothing here\n" });
    const r = oxycValidate(root, []);
    expect(r.status).toBe(0);
    expect(r.stderr).toMatch(/no validatable YAML found/);
  });

  /** A broken link is the other way in, and it counts the same. */
  it("fails when the only candidate was a link that could not be read", () => {
    const root = workspace({ "notes.md": "nothing here\n" });
    symlinkSync(join(root, "gone.yml"), join(root, "stale.app.yml"), "file");
    const r = oxycValidate(root, []);
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stderr).toMatch(/1 file\(s\) NOT checked — the target is gone/);
  });
});

describe("the workspace walk", () => {
  it("finds the root from a subdirectory", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "apps/broken.app.yml": "tasks: not-a-list\n"
    });
    const r = oxycValidate(join(root, "apps"), []);
    expect(r.status).toBe(ExitCode.FAILURE);
    expect(r.stdout + r.stderr).toMatch(/apps\/broken\.app\.yml/);
  });

  /**
   * CEILINGED AT THE GIT ROOT. Unbounded, this walked to `/`, so running it
   * outside a workspace on a machine with a `~/config.yml` adopted `$HOME` as
   * the root and walked the entire home directory.
   *
   * The fixture is built so the two behaviours DISAGREE, which the obvious
   * version of this test does not: a `config.yml` sits one level ABOVE a git
   * root, and the cwd sits inside it. Ceilinged, the walk stops at the repo and
   * reports nothing validatable. Unceilinged, it climbs past the repo, adopts
   * the outer `config.yml` and reports a file valid. A fixture with no
   * `config.yml` anywhere above passes either way and proves nothing.
   */
  it("stops at the git root instead of adopting a config.yml above it", () => {
    const outer = workspace({
      "config.yml": MINIMAL_CONFIG,
      "repo/sub/notes.md": "nothing validatable here\n"
    });
    // A REAL repo: `repoRoot` shells out to `git rev-parse --show-toplevel`,
    // so a hand-made `.git/` directory is not a ceiling at all.
    const init = spawnSync("git", ["init", "-q"], { cwd: join(outer, "repo"), encoding: "utf8" });
    expect(init.status, init.stderr).toBe(0);

    const r = oxycValidate(join(outer, "repo", "sub"), []);
    expect(r.status).toBe(0);
    expect(r.stderr).toMatch(/no validatable YAML found/);
    // The outer config.yml was never reached, so it was never counted.
    expect(r.stdout).not.toMatch(/file\(s\) valid/);
  });

  /**
   * THE HOME CEILING, which the first version only held when the cwd happened
   * to be under `$HOME`. Outside a repo and outside home — `/tmp`, `/srv`, a
   * container workdir — `homedir()` is never an ancestor, so the stop never
   * fired and the walk climbed to `/` exactly as before it was added. Here the
   * cwd IS under home, so the ceiling is live and the `config.yml` sitting at
   * home itself must not be adopted: a home directory is not a workspace, and
   * treating one as a root walks the whole thing.
   */
  it("does not adopt a config.yml sitting at $HOME", () => {
    const home = workspace({
      "config.yml": MINIMAL_CONFIG,
      "projects/thing/notes.md": "nothing validatable here\n"
    });
    const r = oxycValidate(join(home, "projects", "thing"), [], SCHEMAS, home);
    expect(r.status).toBe(0);
    expect(r.stderr).toMatch(/no validatable YAML found/);
    expect(r.stdout).not.toMatch(/file\(s\) valid/);
  });

  /**
   * THE HOME STOP DOES WORK OF ITS OWN, separate from not adopting `~` itself:
   * without it the walk crosses home and keeps checking `/Users`, `/home`, `/`.
   * The fixture puts a `config.yml` ONE LEVEL ABOVE the home directory, so the
   * two behaviours disagree — stopped, it is never seen; unstopped, it is
   * adopted and a directory outside the user's home becomes the workspace root.
   */
  it("does not climb past $HOME into the directory above it", () => {
    const above = workspace({
      "config.yml": MINIMAL_CONFIG,
      "home/projects/notes.md": "nothing validatable here\n"
    });
    const home = join(above, "home");
    const r = oxycValidate(join(home, "projects"), [], SCHEMAS, home);
    expect(r.status).toBe(0);
    expect(r.stderr).toMatch(/no validatable YAML found/);
    expect(r.stdout).not.toMatch(/file\(s\) valid/);
  });

  /**
   * THE HOME STOP ACROSS A SYMLINK, which is the shape a real machine has.
   * `os.homedir()` returns `$HOME` verbatim; `process.cwd()` is `getcwd(3)` and
   * therefore physical. On macOS `$TMPDIR` lives under `/var`, itself a link to
   * `/private/var`, so the child is handed one spelling of home and computes
   * another — `dir === home` never fires, the walk climbs past home and adopts
   * the `config.yml` above it.
   *
   * The fixture forces that shape explicitly rather than relying on the host's
   * temp directory: the other two home cases pass on a box whose `tmpdir()` is
   * already physical, which is exactly how this went unnoticed.
   */
  it("stops at $HOME even when the path to it crosses a symlink", () => {
    const scratch = workspace({
      "real/config.yml": MINIMAL_CONFIG,
      "real/home/projects/notes.md": "nothing validatable here\n"
    });
    symlinkSync(join(scratch, "real"), join(scratch, "link"), "dir");

    // HOME is the LOGICAL spelling; the cwd resolves to the physical one.
    const r = oxycValidate(
      join(scratch, "link", "home", "projects"),
      [],
      SCHEMAS,
      join(scratch, "link", "home")
    );
    expect(r.status).toBe(0);
    expect(r.stderr).toMatch(/no validatable YAML found/);
    expect(r.stdout).not.toMatch(/file\(s\) valid/);
  });

  /**
   * And a real ancestor inside the tree IS still found — the stops bound the
   * walk, they do not disable it. Without this the two cases above are also
   * satisfied by a `findWorkspace` that never climbs at all.
   */
  it("still finds a config.yml in a genuine ancestor", () => {
    const root = workspace({
      "config.yml": MINIMAL_CONFIG,
      "apps/dashboards/finance/notes.md": "nothing validatable here\n"
    });
    // $HOME points elsewhere: the workspace root must be found on its own
    // merits, not because it happens to be the home directory.
    const r = oxycValidate(
      join(root, "apps", "dashboards", "finance"),
      [],
      SCHEMAS,
      workspace({ "unrelated.md": "not this tree\n" })
    );
    expect(r.status).toBe(0);
    expect(r.stdout).toMatch(/1 file\(s\) valid/);
  });

  /** With nothing above it either, the cwd is simply the answer. */
  it("falls back to the cwd outside any repo", () => {
    const root = workspace({ "notes.md": "nothing here\n" });
    const r = oxycValidate(root, []);
    expect(r.status).toBe(0);
    expect(r.stderr).toMatch(/no validatable YAML found/);
  });

  /** `build` holds rendered copies of workspace files; checking them is noise. */
  it("skips build and the vcs directories", () => {
    for (const name of ["build", "dist", "node_modules", ".git", "target", ".worktrees"]) {
      expect(walkable(name), name).toBe(false);
    }
    expect(walkable("apps")).toBe(true);
  });
});

describe("scanning a schema for formats", () => {
  /**
   * A `format` only counts beside the keywords that make its object a schema.
   * The version this replaced regex-scanned the raw JSON text, which cannot
   * tell a constraint from the word "format" appearing in data.
   */
  it("finds a format in schema position", () => {
    const seen = new Set<string>();
    formatsInSchemaPosition({ properties: { port: { type: "integer", format: "uint16" } } }, seen);
    expect([...seen]).toEqual(["uint16"]);
  });

  /**
   * THE CASE THE TEXT SCAN GOT WRONG. `default` and `examples` hold INSTANCE
   * data — a serialised config whose own key happens to be `format`.
   *
   * The data object here carries `type` as well, so it looks exactly like a
   * schema from the inside: NOT DESCENDING is the only thing that saves it,
   * which is what makes this a test of the skip rather than of the sibling
   * check downstream. A fixture whose data object is a bare `{format: "csv"}`
   * passes with the skip deleted, because the sibling check catches it anyway.
   */
  it("does not descend into default or examples, even when the data looks like a schema", () => {
    const seen = new Set<string>();
    formatsInSchemaPosition(
      {
        type: "object",
        properties: {
          export: {
            type: "object",
            default: { type: "csv", format: "wide" },
            examples: [{ type: "parquet", format: "tall" }],
            const: { type: "json", format: "nested" }
          }
        }
      },
      seen
    );
    expect([...seen]).toEqual([]);
  });

  /**
   * And the other guard, alone. An annotation keyword is not `default`, so the
   * walk DOES descend into it — only the missing schema siblings stop the
   * `format` inside from counting. Deleting the sibling check makes this fail
   * while every `default`-shaped fixture still passes.
   */
  it("does not count a format with no schema keyword beside it", () => {
    const seen = new Set<string>();
    formatsInSchemaPosition(
      {
        type: "object",
        properties: { a: { type: "string" } },
        "x-oxy-export": { format: "csv" },
        metadata: { format: "internal", owner: "platform" }
      },
      seen
    );
    expect([...seen]).toEqual([]);
  });

  it("descends through arrays and nested subschemas", () => {
    const seen = new Set<string>();
    formatsInSchemaPosition(
      {
        anyOf: [
          { type: "string", format: "date-time" },
          { items: { type: "integer", format: "int64" } }
        ]
      },
      seen
    );
    expect([...seen].sort()).toEqual(["date-time", "int64"]);
  });

  /**
   * Every format the SHIPPED schemas use must be one `registerRustFormats`
   * teaches ajv — an unknown one is silently ignored, so the constraint stops
   * applying with nothing to show for it. This is the assertion that fires
   * when a Rust type starts emitting a width nobody registered.
   */
  it("finds nothing unregistered in the schemas this package ships", () => {
    const seen = new Set<string>();
    for (const name of [
      "config.json",
      "app.json",
      "workflow.json",
      "agentic.json",
      "agent-test.json"
    ]) {
      formatsInSchemaPosition(JSON.parse(readFileSync(join(SCHEMAS, name), "utf8")), seen);
    }
    const KNOWN = new Set([
      "uint",
      "uint8",
      "uint16",
      "uint32",
      "uint64",
      "int8",
      "int16",
      "int32",
      "int64",
      "double",
      "float",
      "date",
      "time",
      "date-time",
      "duration",
      "uri",
      "uri-reference",
      "email",
      "hostname",
      "ipv4",
      "ipv6",
      "uuid",
      "regex",
      "json-pointer",
      "byte",
      "binary",
      "password",
      "int32-or-string"
    ]);
    expect([...seen].filter((f) => !KNOWN.has(f))).toEqual([]);

    // AND IT FOUND THEM. Asserting only "nothing unknown" is satisfied by a
    // walk that finds nothing at all — a sibling list too narrow to match what
    // `schemars` really emits would pass the line above and silently stop
    // checking every format in the package.
    expect(seen.size).toBeGreaterThan(0);
    for (const expected of ["uint", "uint32", "int64", "double"]) {
      expect(seen.has(expected), expected).toBe(true);
    }
  });
});
