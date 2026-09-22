/**
 * The Oxy Functions lint, rule by rule, against the fixture apps in
 * `fixtures/function-lint/` — a violating app and a clean twin for each — and
 * against the first-party apps, which must stay clean or say which side is
 * wrong.
 */

import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { ExitCode } from "../util/errors.js";
import { capabilitiesFor, GATED_CAPABILITIES, WRITE_OPS } from "./capabilities.js";
import {
  checkEngines,
  type DatabaseEngine,
  fetchDatabaseEngines,
  functionLintFailure
} from "./function-engines.js";
import { ABSENT_GLOBALS, describeLintIssue, lintAppFunctions } from "./function-lint.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const FIXTURES = resolve(HERE, "..", "..", "fixtures", "function-lint");
const REPO = resolve(HERE, "..", "..", "..", "..");

function fixture(name: string) {
  const dir = join(FIXTURES, name);
  const manifest = JSON.parse(readFileSync(join(dir, "oxy-app.json"), "utf8"));
  return { dir, manifest, result: lintAppFunctions(dir, manifest) };
}

const at = (file: string, line: number) => expect.objectContaining({ file, path: `line ${line}` });

describe("the capability map", () => {
  it("names one host field per entry, and every entry an op", () => {
    const fields = GATED_CAPABILITIES.map((c) => c.hostField);
    expect(new Set(fields).size).toBe(fields.length);
    for (const cap of GATED_CAPABILITIES) expect(cap.ops.length).toBeGreaterThan(0);
  });

  it("answers a member path, an area wildcard, and two gates for storage.copy", () => {
    expect(capabilitiesFor("secrets.set").map((c) => c.manifest)).toEqual(["secrets.write"]);
    expect(capabilitiesFor("oltp.tx").map((c) => c.manifest)).toEqual(["oltp.enabled"]);
    expect(capabilitiesFor("storage.copy").map((c) => c.manifest)).toEqual([
      "storage.read",
      "storage.write"
    ]);
    expect(capabilitiesFor("log")).toEqual([]);
    expect(capabilitiesFor("warehouse.query")).toEqual([]);
    expect(WRITE_OPS).toEqual(["warehouse.insert", "warehouse.exec", "warehouse.upsert", "tx"]);
  });
});

describe("capability", () => {
  it("reports each ctx call whose capability the manifest lacks, with the fix", () => {
    const { result } = fixture("capability-violating");
    const capability = result.issues.filter((i) => i.rule === "capability");
    expect(
      capability.map((i) => [i.path, i.message.match(/`([a-z.A-Z]+)` capability/)?.[1]])
    ).toEqual([
      ["line 3", "secrets.write"],
      ["line 4", "email.send"],
      ["line 5", "org.read"],
      ["line 6", "storage.write"],
      ["line 7", "storage.write"],
      ["line 8", "storage.read"],
      ["line 9", "storage.read"],
      ["line 9", "storage.write"],
      ["line 10", "oltp.enabled"],
      ["line 11", "airhouse.enabled"]
    ]);
    const put = capability.find((i) => i.path === "line 7");
    expect(put).toEqual(at("functions/sync.ts", 7));
    expect(put?.fn).toBe("sync");
    expect(put?.message).toContain("`ctx.storage.put(`");
    expect(put?.message).toContain(
      'add "storage": { "write": true } to functions.sync in oxy-app.json'
    );
    expect(put?.level).toBe("error");
  });

  it("is silent when every capability is declared", () => {
    const { result } = fixture("capability-clean");
    expect(result.issues).toEqual([]);
    expect(result.skipped).toEqual([]);
  });

  it("does not see a call in a comment, a string, a template literal or a regex", () => {
    const { result } = fixture("globals-clean");
    expect(result.issues.filter((i) => i.rule === "capability")).toEqual([]);
  });
});

describe("isolate-global", () => {
  it("reports one value use of each absent global, naming what to use instead", () => {
    const { result } = fixture("globals-violating");
    const globals = result.issues.filter((i) => i.rule === "isolate-global");
    expect(globals.map((i) => [i.path, i.message.match(/^`([^`]+)`/)?.[1]])).toEqual([
      ["line 3", "Buffer."],
      ["line 4", "new TextEncoder("],
      ["line 5", "new TextDecoder("],
      ["line 6", "new Blob("],
      ["line 7", "new File("],
      ["line 8", "new FormData("],
      ["line 9", "crypto.subtle"],
      ["line 10", "process.env"]
    ]);
    expect(globals[0]?.message).toContain("has no `Buffer`");
    expect(globals[0]?.message).toContain("ReferenceError");
    expect(globals[6]?.message).toContain("use `ctx.crypto`");
    expect(globals[7]?.message).toContain("`ctx.env`");
    expect(globals.every((i) => i.file === "functions/handler.ts")).toBe(true);
  });

  it("covers every global the docs rule out", () => {
    const names = ABSENT_GLOBALS.map((g) => (g.member ? `${g.name}.${g.member}` : g.name));
    expect(names).toEqual([
      "Buffer",
      "TextEncoder",
      "TextDecoder",
      "Blob",
      "File",
      "FormData",
      "crypto.subtle",
      "process"
    ]);
  });

  it("is silent for comments, strings, templates, regex, typeof, a type, a property, and a shadow", () => {
    const { result } = fixture("globals-clean");
    expect(result.issues).toEqual([]);
  });

  it("treats a type-only import as no shadow, and a value import as one", () => {
    // `import type { Blob }` names a type; `new Blob(` would still throw.
    const source =
      'import type { Blob } from "./x.js";\nexport default (r: unknown, ctx: any) => new Blob([]);';
    const app = tmpApp({ "functions/f.ts": source });
    expect(lintAppFunctions(app.dir, app.manifest).issues).toEqual([at("functions/f.ts", 2)]);

    const valued = tmpApp({
      "functions/f.ts": 'import { Blob } from "./x.js";\nexport default () => new Blob([]);',
      "functions/x.ts": "export class Blob {}"
    });
    expect(lintAppFunctions(valued.dir, valued.manifest).issues).toEqual([]);
  });
});

describe("destinations", () => {
  it("refuses a write with no destinations, and one outside the allowlist", () => {
    const { result } = fixture("destinations-violating");
    expect(result.issues.map((i) => [i.rule, i.fn, i.file, i.path])).toEqual([
      ["destinations", "etl", "functions/etl.ts", "line 3"],
      ["destinations", "other", "functions/other.ts", "line 3"],
      ["destinations", "other", "functions/other.ts", "line 4"]
    ]);
    expect(result.issues[0]?.message).toContain('`ctx.warehouse.insert("analytics", …)`');
    expect(result.issues[0]?.message).toContain("declares no `destinations`");
    expect(result.issues[1]?.message).toContain(
      'not in `functions.other.destinations` (["reports"])'
    );
    expect(result.issues[2]?.message).toContain('`ctx.tx("analytics", …)`');
    expect(result.writes).toEqual([]);
  });

  it("collects each allowed write for the engine check, with the literal database when there is one", () => {
    const { result } = fixture("destinations-clean");
    expect(result.issues).toEqual([]);
    expect(result.writes.map((w) => [w.member, w.database, w.line])).toEqual([
      ["warehouse.insert", "analytics", 5],
      ["warehouse.exec", "analytics", 6],
      ["warehouse.upsert", "analytics", 7],
      ["tx", "analytics", 8],
      ["warehouse.exec", undefined, 9]
    ]);
  });
});

describe("relative imports", () => {
  it("follows ./x.js, ./dir/index.ts and a re-import, never a package or a path outside the app", () => {
    const { result } = fixture("imports");
    expect(result.issues.map((i) => [i.rule, i.file, i.path])).toEqual([
      ["isolate-global", "functions/helpers/index.ts", "line 2"],
      ["capability", "functions/steps.ts", "line 4"]
    ]);
    // `escape.ts` holds `new TextEncoder()`; reaching it would add a third.
    expect(result.issues.some((i) => i.file.includes("escape"))).toBe(false);
    expect(result.files.sort()).toEqual([
      "functions/entry.ts",
      "functions/helpers/index.ts",
      "functions/steps.ts"
    ]);
  });

  it("reads a helper two functions share once: one global finding, but one capability finding each", () => {
    const app = tmpApp(
      {
        "functions/a.ts":
          'import { go } from "./shared.js";\nexport default (r: unknown, ctx: any) => go(ctx);',
        "functions/b.ts":
          'import { go } from "./shared.js";\nexport default (r: unknown, ctx: any) => go(ctx);',
        "functions/shared.ts":
          "export const go = (ctx: any) => ctx.email.send({ body: Buffer.from('x') });\n"
      },
      { functions: { a: {}, b: { email: { send: true } } } }
    );
    const result = lintAppFunctions(app.dir, app.manifest);
    expect(result.issues.map((i) => [i.rule, i.fn, i.file, i.path])).toEqual([
      // The file's fact, once, credited to the first function that reached it.
      ["isolate-global", "a", "functions/shared.ts", "line 1"],
      // The entry's fact: `a` lacks `email.send`, `b` declares it.
      ["capability", "a", "functions/shared.ts", "line 1"]
    ]);
    expect(result.files.sort()).toEqual([
      "functions/a.ts",
      "functions/b.ts",
      "functions/shared.ts"
    ]);
  });
});

describe("a parameter shadows an absent global", () => {
  const cases: Array<[label: string, source: string]> = [
    ["a function parameter", "export function f(Buffer: any) { return Buffer.from('x'); }"],
    ["an arrow parameter", "export const f = (Buffer: any) => Buffer.from('x');"],
    ["a bare arrow parameter", "export const f = Buffer => Buffer.from('x');"],
    ["a destructured parameter", "export const f = ({ Buffer }: any) => Buffer.from('x');"],
    ["a method parameter", "export const o = { f(Buffer: any) { return Buffer.from('x'); } };"],
    ["a defaulted parameter", "export function f(Buffer = null) { return Buffer.from('x'); }"]
  ];

  it.each(cases)("%s is the author's value, not the missing global", (_label, source) => {
    const app = tmpApp({ "functions/f.ts": `${source}\n` });
    expect(lintAppFunctions(app.dir, app.manifest).issues).toEqual([]);
  });

  it("does not read a type annotation, a generic or a default-value use as a shadow", () => {
    const app = tmpApp({
      "functions/f.ts":
        "export function f(x: Buffer, y: Array<Blob>, z: Blob | File, w = TextEncoder.name) {\n" +
        "  return Buffer.from('x');\n" +
        "}\n"
    });
    const issues = lintAppFunctions(app.dir, app.manifest).issues;
    expect(issues.map((i) => [i.path, i.message.match(/^`([^`]+)`/)?.[1]])).toEqual([
      ["line 1", "TextEncoder."],
      ["line 2", "Buffer."]
    ]);
  });

  it("says which function was not linted when its entry is missing", () => {
    const app = tmpApp({}, { functions: { ghost: {} } });
    const result = lintAppFunctions(app.dir, app.manifest);
    expect(result.issues).toEqual([]);
    expect(result.skipped).toEqual([
      "function `ghost` was not linted: functions/ghost.ts could not be read (ENOENT)"
    ]);
  });

  it("sees ctx handed around as a member of something, but not a longer name", () => {
    const app = tmpApp({
      "functions/f.ts":
        "export default async (r: unknown, ctx: any) => {\n" +
        "  const run = { ctx };\n" +
        "  await run.ctx.email.send({});\n" +
        "  const myctx = { email: { send: async () => 1 } };\n" +
        "  await myctx.email.send();\n" +
        "  await run?.ctx?.secrets?.set('k', 'v');\n" +
        "};"
    });
    const issues = lintAppFunctions(app.dir, app.manifest).issues;
    expect(issues.map((i) => [i.path, i.message.slice(0, 20)])).toEqual([
      ["line 3", "`ctx.email.send(` ne"],
      ["line 6", "`ctx.secrets.set(` n"]
    ]);
  });
});

describe("engines", () => {
  const DATABASES: DatabaseEngine[] = [
    { name: "ch", dialect: "clickhouse", db_type: "clickhouse" },
    { name: "pg", dialect: "postgres", db_type: "postgres" },
    { name: "oltp", dialect: "postgres", db_type: "postgres_managed" },
    { name: "ah", dialect: "duckdb", db_type: "airhouse_managed" }
  ];

  /** One write of `member` into `database`, allowed by the manifest as `writes` says. */
  function write(member: string, database: string | undefined, writes?: Record<string, string>) {
    const manifest = {
      functions: {
        f: { destinations: DATABASES.map((d) => d.name), customerWarehouseWrites: writes }
      }
    };
    const call = {
      fn: "f",
      file: "functions/f.ts",
      line: 7,
      member,
      call: `ctx.${member}(`,
      database
    };
    return checkEngines([call], manifest, DATABASES).map((i) => [i.rule, i.message]);
  }

  it("refuses upsert where ON CONFLICT is not SQL, by dialect", () => {
    expect(write("warehouse.upsert", "ch", { ch: "legacy" })).toEqual([
      ["engine", expect.stringContaining("`ch` is clickhouse, and `upsert` compiles to")]
    ]);
    expect(write("warehouse.upsert", "pg", { pg: "legacy" })).toEqual([]);
    expect(write("warehouse.upsert", "ah")).toEqual([]);
  });

  it("refuses ctx.tx anywhere but Postgres", () => {
    expect(write("tx", "ah")).toEqual([
      ["engine", expect.stringContaining("`ah` is duckdb, and `ctx.tx` is Postgres-only")]
    ]);
    expect(write("tx", "pg", { pg: "orders" })).toEqual([]);
  });

  it("refuses a customer-warehouse write without a reason, and the org's OLTP outright", () => {
    expect(write("warehouse.insert", "ch")).toEqual([
      [
        "customer-warehouse",
        expect.stringContaining("a customer warehouse, and customer warehouses are read-only")
      ]
    ]);
    expect(write("warehouse.insert", "ch")[0]?.[1]).toContain('"customerWarehouseWrites"');
    expect(write("warehouse.insert", "ch", { ch: "  " })).toHaveLength(1);
    expect(write("warehouse.insert", "ch", { ch: "legacy facts" })).toEqual([]);
    expect(write("tx", "ch")[0]?.[1]).toContain("counts as a write even when it only reads");
    expect(write("warehouse.exec", "oltp")).toEqual([
      ["customer-warehouse", expect.stringContaining("the org's OLTP store")]
    ]);
    expect(write("warehouse.exec", "oltp")[0]?.[1]).toContain("`ctx.oltp`");
  });

  it("refuses a database the project does not have, and skips one it cannot name", () => {
    expect(write("warehouse.insert", "nope")).toEqual([
      ["destinations", expect.stringContaining("not a database of this project")]
    ]);
    expect(write("warehouse.upsert", undefined)).toEqual([]);
  });

  it("fails with the validate exit code, naming every issue and the escape hatch", () => {
    const { result } = fixture("capability-violating");
    const error = functionLintFailure(result.issues);
    expect(error.code).toBe(ExitCode.FAILURE);
    expect(error.message).toBe(
      "10 Oxy Function lint problem(s) — the host would refuse these at the first call"
    );
    expect(error.detail?.split("\n")).toHaveLength(10);
    expect(error.detail).toContain(
      "[function-lint/capability] functions/sync.ts (line 3): `ctx.secrets.set(`"
    );
    expect(error.remedy).toContain("--allow-function-lint");
    expect(error.remedy).toContain("open an issue");
    expect(
      describeLintIssue(result.issues[0] as (typeof result.issues)[number], "apps/x/")
    ).toMatch(/^\[function-lint\/capability\] apps\/x\/functions\/sync\.ts \(line 3\): /);
  });
});

describe("fetching the engines", () => {
  let server: Server;
  let target: string;
  let answer: { status: number; body: string } = { status: 200, body: "[]" };
  beforeAll(async () => {
    server = createServer((req, res) => {
      const ok = req.headers.authorization === "Bearer tok" && req.url === "/api/proj-1/databases";
      res.writeHead(ok ? answer.status : 403, { "content-type": "application/json" });
      res.end(ok ? answer.body : "{}");
    });
    await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
    target = `http://127.0.0.1:${(server.address() as AddressInfo).port}/`;
  });
  afterAll(() => server.close());

  it("reads name, dialect and db_type from the list", async () => {
    const row = {
      name: "ch",
      dialect: "clickhouse",
      db_type: "clickhouse",
      datasets: null,
      synced: false
    };
    answer = { status: 200, body: JSON.stringify([row]) };
    expect(await fetchDatabaseEngines(target, "proj-1", "tok")).toEqual({ databases: [row] });
  });

  it("skips the whole list when one row cannot be read, rather than refusing on a guess", async () => {
    // A row without `db_type` is a database this cannot see; dropping it would
    // make its writes "not a database of this project" — a refusal at exit 1.
    answer = {
      status: 200,
      body: JSON.stringify([
        { name: "ch", dialect: "clickhouse", db_type: "clickhouse" },
        { name: "pg", dialect: "postgres" }
      ])
    };
    expect(await fetchDatabaseEngines(target, "proj-1", "tok")).toEqual({
      skipped: expect.stringContaining("1 row(s) without a string name, dialect and db_type")
    });
  });

  it("skips, with the reason, on a refusal, a non-JSON body, a non-list and a dead host", async () => {
    expect(await fetchDatabaseEngines(target, "proj-1", "wrong")).toEqual({
      skipped: expect.stringContaining("answered 403")
    });
    answer = { status: 200, body: "<html>" };
    expect(await fetchDatabaseEngines(target, "proj-1", "tok")).toEqual({
      skipped: expect.stringContaining("did not answer JSON")
    });
    answer = { status: 200, body: '{"databases":[]}' };
    expect(await fetchDatabaseEngines(target, "proj-1", "tok")).toEqual({
      skipped: expect.stringContaining("did not answer a list")
    });
    expect(await fetchDatabaseEngines("http://127.0.0.1:1", "proj-1", "tok")).toEqual({
      skipped: expect.stringContaining("failed")
    });
  });
});

/**
 * The apps this repo ships must lint clean — the canary, the OLTP example and
 * every scaffold template the `templates` CI job builds. A finding here means
 * either the app is wrong (it would fail closed in prod) or the rule is, and
 * the test's job is to make someone decide which.
 */
describe("first-party apps", () => {
  const load = (rel: string) => {
    const dir = join(REPO, rel);
    const manifest = JSON.parse(readFileSync(join(dir, "oxy-app.json"), "utf8"));
    return { rel, result: lintAppFunctions(dir, manifest) };
  };

  /** Apps that declare functions: "clean" is guarded by "and something was read". */
  const WITH_FUNCTIONS = [
    "customer-apps/examples/platform-canary",
    "customer-apps/examples/oltp-bookings",
    "sdk/create-oxy-app/templates/functions"
  ];

  it.each(WITH_FUNCTIONS)("%s lints clean, and was actually read", (rel) => {
    const { result } = load(rel);
    expect(result.issues.map((i) => describeLintIssue(i, `${rel}/`))).toEqual([]);
    expect(result.skipped).toEqual([]);
    expect(result.files.length).toBeGreaterThan(0);
  });

  /**
   * The other three templates declare no functions, so there is nothing to
   * lint and "clean" would assert nothing. Kept as a statement of that fact:
   * the day one of them gains a `functions` block, this fails and the template
   * moves to the guarded list above.
   */
  const WITHOUT_FUNCTIONS = [
    "sdk/create-oxy-app/templates/dashboard",
    "sdk/create-oxy-app/templates/single-store",
    "sdk/create-oxy-app/templates/vite"
  ];

  it.each(WITHOUT_FUNCTIONS)("%s declares no functions, so nothing is linted", (rel) => {
    const { result } = load(rel);
    expect(result.files).toEqual([]);
    expect(result.issues).toEqual([]);
  });

  it("actually reads the canary: its ctx calls live in steps.ts, behind the entry's import", () => {
    const dir = join(REPO, "customer-apps/examples/platform-canary");
    const manifest = JSON.parse(readFileSync(join(dir, "oxy-app.json"), "utf8"));
    const result = lintAppFunctions(dir, manifest);
    expect(result.writes.length).toBeGreaterThan(0);
    expect(result.writes.every((w) => w.fn === "canary")).toBe(true);
    // steps.ts, and shape-zoo.ts behind it — two hops from the entry.
    expect([...new Set(result.writes.map((w) => w.file))].sort()).toEqual([
      "functions/shape-zoo.ts",
      "functions/steps.ts"
    ]);
    // Its database is a constant, so the engine check has nothing to look up.
    expect(result.writes.every((w) => w.database === undefined)).toBe(true);
  });
});

// ── helpers ──────────────────────────────────────────────────────────────────

const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

/** An app directory with `files`, and a manifest declaring `f` unless given one. */
function tmpApp(
  files: Record<string, string>,
  manifest: Record<string, unknown> = { functions: { f: {} } }
) {
  const dir = mkdtempSync(join(tmpdir(), "oxyc-fn-lint-"));
  SCRATCH.push(dir);
  for (const [rel, content] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, rel)), { recursive: true });
    writeFileSync(join(dir, rel), content);
  }
  return { dir, manifest };
}
