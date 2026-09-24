// The context's stores: per-database warehouse tables, the app's OLTP rows,
// its Airhouse tables — each a `TableStore` evaluating the SQL subset in
// `sql.ts` — plus storage objects, secrets, emails, fetch answers and org
// fixtures. Plain in-memory state a test inspects directly; no timers, no
// network, no filesystem.

import type { OxyFunctionRow } from "../custom-app/function-context";
import { contextError } from "./host-error";
import {
  type Delete,
  type Insert,
  literal,
  parse,
  type Select,
  type Statement,
  type Update
} from "./sql";
import {
  expectation,
  knownTypes,
  type RowSource,
  renderValue,
  ZOO,
  type ZooEngine,
  type ZooPlane,
  ZooRefusal,
  zooColumn,
  zooTableName
} from "./zoo";

/** One stored row and where its values came from. */
interface StoredRow {
  values: Record<string, unknown | ZooRefusal>;
  source: RowSource;
}

interface Table {
  name: string;
  /** Native types by column for a typed table; `null` for a raw one. */
  columns: Record<string, string> | null;
  rows: StoredRow[];
}

/** A read's rows, and the weakest source among them. */
export interface ReadResult {
  rows: OxyFunctionRow[];
  source: RowSource | null;
}

const RANK: Record<RowSource, number> = { zoo: 0, typed: 1, author: 2 };
const weakest = (a: RowSource | null, b: RowSource): RowSource =>
  a === null || RANK[b] > RANK[a] ? b : a;

/**
 * One SQL-evaluating store: a warehouse database, the OLTP schema or the
 * Airhouse schema. `engine` picks which zoo cases render typed columns;
 * `plane` picks which expectation a case yields.
 */
export class TableStore {
  private readonly tables = new Map<string, Table>();

  constructor(
    readonly engine: ZooEngine,
    readonly plane: ZooPlane,
    /** For a schema-qualified name to resolve to a bare one: the schema, or `null`. */
    readonly schema: string | null = null
  ) {}

  /** The zoo table for this engine: column `cNNN` is case NNN's expectation. */
  zoo(): this {
    const cases = ZOO.engines[this.engine].cases;
    const columns: Record<string, string> = {};
    const values: Record<string, unknown | ZooRefusal> = {};
    cases.forEach((c, i) => {
      const column = zooColumn(i);
      columns[column] = c.native_type;
      values[column] = expectation(c, this.plane, column);
    });
    this.tables.set(zooTableName(), {
      name: zooTableName(),
      columns,
      rows: [{ values, source: "zoo" }]
    });
    return this;
  }

  /**
   * Declare a typed table: `{ id: "UInt64", amount: "Decimal(18, 4)" }`. A
   * type the zoo has no case for is an error here, not a guess at read time.
   */
  table(name: string, columns: Record<string, string>): TypedTable {
    const key = this.key(name);
    for (const [column, type] of Object.entries(columns)) {
      if (!ZOO.engines[this.engine].cases.some((c) => c.native_type === type)) {
        throw contextError(
          `the zoo has no ${this.engine} case for type "${type}" (column "${column}" of "${name}"); it pins: ${knownTypes(this.engine).join(", ")}`
        );
      }
    }
    if (!this.tables.has(key)) this.tables.set(key, { name: key, columns, rows: [] });
    return new TypedTable(this, key);
  }

  /** Seed rows as JS values, unrendered — every read of them is `source: "author"`. */
  raw(name: string, rows: OxyFunctionRow[]): this {
    const key = this.key(name);
    const table = this.tables.get(key) ?? { name: key, columns: null, rows: [] };
    for (const row of rows) table.rows.push({ values: { ...row }, source: "author" });
    this.tables.set(key, table);
    return this;
  }

  /** Every row of `name` as a function would read it; a `$error` cell is its refusal text. */
  rows(name: string): OxyFunctionRow[] {
    return this.lookup(name)?.rows.map((r) => materialise(r.values)) ?? [];
  }

  /** The names of every table declared, seeded or created. */
  tableNames(): string[] {
    return [...this.tables.keys()];
  }

  /** Whether `name` exists. */
  has(name: string): boolean {
    return this.lookup(name) !== undefined;
  }

  /** Evaluate `sql` as a read: rows, or a throw. */
  query(sql: string, params: readonly unknown[] = []): ReadResult {
    const statement = parse(sql);
    if (statement.kind !== "select")
      throw contextError(
        `expected a SELECT, got ${statement.kind.toUpperCase()}: ${sql.slice(0, 80)}`
      );
    return this.select(statement, params);
  }

  /** Evaluate `sql` as a statement: rows affected, or a throw. */
  exec(sql: string, params: readonly unknown[] = []): number {
    const statement = parse(sql);
    switch (statement.kind) {
      case "create":
        return this.create(statement);
      case "insert":
        return this.insert(statement, params);
      case "delete":
        return this.remove(statement, params);
      case "update":
        return this.update(statement, params);
      case "select":
        return this.select(statement, params).rows.length;
    }
  }

  /** Append JS rows to `name` (`ctx.warehouse.insert`, `ctx.airhouse.append`): unrendered. */
  append(name: string, rows: OxyFunctionRow[]): number {
    this.raw(name, rows);
    return rows.length;
  }

  /** Snapshot every table, for `ctx.tx` rollback. */
  snapshot(): () => void {
    const copy = new Map(
      [...this.tables].map(([k, t]) => [
        k,
        { ...t, rows: t.rows.map((r) => ({ ...r, values: { ...r.values } })) }
      ])
    );
    return () => {
      this.tables.clear();
      for (const [k, t] of copy) this.tables.set(k, t);
    };
  }

  private key(name: string): string {
    const bare =
      this.schema && name.startsWith(`${this.schema}.`) ? name.slice(this.schema.length + 1) : name;
    return bare;
  }

  private lookup(name: string): Table | undefined {
    return this.tables.get(this.key(name)) ?? this.tables.get(name);
  }

  private require(name: string, statement: Statement): Table {
    const table = this.lookup(name);
    if (!table) {
      throw contextError(
        `${statement.kind.toUpperCase()} names a table the test context does not have: "${name}". Declare it with t.state.<store>.table(name, columns), seed it with .raw(name, rows), or let the function CREATE it first. Known: ${this.tableNames().join(", ") || "none"}`
      );
    }
    return table;
  }

  private create(statement: Statement & { kind: "create" }): number {
    const key = this.key(statement.table);
    if (this.tables.has(key)) {
      if (statement.ifNotExists) return 0;
      throw contextError(`CREATE TABLE "${statement.table}": the table already exists`);
    }
    const columns: Record<string, string> = {};
    for (const c of statement.columns) columns[c.name] = c.type;
    this.tables.set(key, { name: key, columns, rows: [] });
    return 0;
  }

  private insert(statement: Insert, params: readonly unknown[]): number {
    const table = this.require(statement.table, statement);
    const names = statement.columns ?? (table.columns ? Object.keys(table.columns) : null);
    for (const tuple of statement.tuples) {
      if (!names || names.length !== tuple.length) {
        throw contextError(
          `INSERT INTO "${statement.table}" gives ${tuple.length} value(s) for ${names?.length ?? "an unknown number of"} column(s)`
        );
      }
      const values: Record<string, unknown | ZooRefusal> = {};
      let source: RowSource | null = null;
      names.forEach((column, i) => {
        const type = table.columns?.[column];
        if (type && !/^\$\d+$/.test(tuple[i].trim())) {
          const rendered = renderValue(this.engine, type, tuple[i], this.plane, column);
          values[column] = rendered.value;
          source = weakest(source, rendered.source);
        } else {
          values[column] = literal(tuple[i], params);
          source = weakest(source, "author");
        }
      });
      table.rows.push({ values, source: source ?? "author" });
    }
    return statement.tuples.length;
  }

  private matching(
    table: Table,
    statement: Select | Delete | Update,
    params: readonly unknown[]
  ): StoredRow[] {
    return table.rows.filter((row) =>
      statement.where.every((p) => {
        const cell = row.values[p.column];
        return p.values.some((v) => looselyEqual(cell, literal(v, params)));
      })
    );
  }

  private select(statement: Select, params: readonly unknown[]): ReadResult {
    if (statement.table === null) {
      const row: OxyFunctionRow = {};
      for (const item of statement.items) {
        if (item.kind === "literal") row[item.alias] = literal(item.raw, params);
        else
          throw contextError(
            `SELECT without FROM can only project literals: ${JSON.stringify(item)}`
          );
      }
      return { rows: [row], source: "author" };
    }
    const table = this.require(statement.table, statement);
    let matched = this.matching(table, statement, params);
    if (statement.items.some((i) => i.kind === "count")) {
      const alias =
        statement.items.find((i): i is { kind: "count"; alias: string } => i.kind === "count")
          ?.alias ?? "count(*)";
      return { rows: [{ [alias]: matched.length }], source: matched[0]?.source ?? null };
    }
    if (statement.orderBy) {
      const { column, desc } = statement.orderBy;
      matched = [...matched].sort(
        (a, b) => compare(a.values[column], b.values[column]) * (desc ? -1 : 1)
      );
    }
    if (statement.limit !== null) matched = matched.slice(0, statement.limit);
    let source: RowSource | null = null;
    const rows = matched.map((stored) => {
      source = weakest(source, stored.source);
      const out: OxyFunctionRow = {};
      for (const item of statement.items) {
        if (item.kind === "all") Object.assign(out, materialise(stored.values));
        else if (item.kind === "column")
          out[item.alias] = materialise({ v: stored.values[item.column] }).v;
        else if (item.kind === "literal") out[item.alias] = literal(item.raw, params);
      }
      return out;
    });
    return { rows, source };
  }

  private remove(statement: Delete, params: readonly unknown[]): number {
    const table = this.require(statement.table, statement);
    const gone = new Set(this.matching(table, statement, params));
    table.rows = table.rows.filter((r) => !gone.has(r));
    return gone.size;
  }

  private update(statement: Update, params: readonly unknown[]): number {
    const table = this.require(statement.table, statement);
    const hit = this.matching(table, statement, params);
    for (const row of hit) {
      for (const { column, raw } of statement.set) {
        row.values[column] = literal(raw, params);
        row.source = "author";
      }
    }
    return hit.length;
  }
}

/** A declared typed table: insert SQL-side values, rendered through the zoo. */
export class TypedTable {
  constructor(
    private readonly store: TableStore,
    readonly name: string
  ) {}

  /** Insert rows of SQL-side values (`{ id: "42", at: "'2024-03-10 12:34:56'" }`). */
  insert(rows: Record<string, string>[]): this {
    for (const row of rows) {
      const columns = Object.keys(row);
      const values = columns.map((c) => row[c]);
      this.store.exec(
        `INSERT INTO ${this.name} (${columns.join(", ")}) VALUES (${values.join(", ")})`
      );
    }
    return this;
  }
}

/** A stored row as a read returns it; a refusal cell throws the host's sentence. */
function materialise(values: Record<string, unknown | ZooRefusal>): OxyFunctionRow {
  const out: OxyFunctionRow = {};
  for (const [k, v] of Object.entries(values)) {
    if (v instanceof ZooRefusal) throw refusalOf(v);
    out[k] = v;
  }
  return out;
}

/** Marker so the context can wrap a zoo refusal as the OLTP surface's `HostError`. */
export class ZooRefusalError extends Error {
  constructor(readonly refusal: ZooRefusal) {
    super(refusal.message());
    this.name = "ZooRefusalError";
  }
}

const refusalOf = (r: ZooRefusal) => new ZooRefusalError(r);

function looselyEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (a === null || b === null || a === undefined || b === undefined) return false;
  return String(a) === String(b);
}

function compare(a: unknown, b: unknown): number {
  if (typeof a === "number" && typeof b === "number") return a - b;
  return String(a).localeCompare(String(b));
}

// ── the other stores ─────────────────────────────────────────────────────────

/** One object in the app's storage silo, body as base64. */
export interface StoredObject {
  key: string;
  body: string;
  contentType: string | null;
  size: number;
  lastModified: string;
}

/** What `ctx.fetch` should answer for a URL: a result, or a function of the call. */
export type FetchAnswer =
  | { status?: number; body?: string; encoding?: "utf8" | "base64" }
  | ((
      url: string,
      init: RequestInit & { encoding?: "utf8" | "base64" }
    ) => {
      status?: number;
      body?: string;
      encoding?: "utf8" | "base64";
    });

/** The `ctx.fetch` side: answers by URL, and what was received. */
export class FetchStore {
  readonly received: { url: string; init: RequestInit & { encoding?: "utf8" | "base64" } }[] = [];
  private readonly answers: { match: string | RegExp; answer: FetchAnswer }[] = [];

  /** Answer every URL equal to `match` (or matching the RegExp) with `answer`. */
  on(match: string | RegExp, answer: FetchAnswer): this {
    this.answers.push({ match, answer });
    return this;
  }

  answerFor(url: string): FetchAnswer | undefined {
    return this.answers.find(({ match }) =>
      typeof match === "string" ? match === url : match.test(url)
    )?.answer;
  }
}
