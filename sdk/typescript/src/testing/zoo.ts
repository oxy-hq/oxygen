// The shape zoo: the pinned JS value each engine's native types arrive as.
//
// `shape-zoo.json` is a byte copy of `fixtures/data-shapes/zoo.json` — 313
// cases, one per native type and value class, each `expect` being what a real
// read through the platform returned (`custom_app_functions_shape_zoo` on the
// Rust side loads every case through the connector and reads it back through
// a published function). `scripts/data-shapes/sync-canary-zoo.mjs` writes the
// copy and its digest; `shape-zoo-sync.test.ts` fails when either is stale.
//
// The context answers reads from it two ways: a zoo TABLE (`t.state.<store>.zoo()`)
// whose column `cNNN` is case NNN's expectation, and TYPED tables, where an
// inserted SQL-side value is rendered through the case for its type — exactly
// when the value IS a case's `value_sql`, and by the `plain` case's shape
// otherwise. An author's own JS values (`raw`) are never rendered.

import { REFUSALS } from "./host-contract";
import { render } from "./host-error";
import zooJson from "./shape-zoo.json";
import { ZOO_SHA256 } from "./shape-zoo-sha256";

export type ZooEngine = "clickhouse" | "postgres" | "duckdb";
/** Which plane read the value: `ctx.warehouse` / `ctx.query`, or `ctx.oltp` / `ctx.tx`. */
export type ZooPlane = "warehouse" | "oltp";

export interface ZooCase {
  key: string;
  native_type: string;
  class: string;
  value_sql: string;
  expect: { warehouse: unknown; oltp?: unknown };
}

export interface ShapeZoo {
  version: number;
  engines: Record<ZooEngine, { cases: ZooCase[] }>;
}

/** Where a row value came from — the honesty flag on every read. */
export type RowSource = "zoo" | "typed" | "author";

/** A value a read would REFUSE rather than return: `expect.oltp.$error`. */
export class ZooRefusal {
  constructor(
    readonly column: string,
    readonly nativeType: string,
    readonly detail: string
  ) {}
  /** The host's full sentence for it, from the contract. */
  message(): string {
    return render(REFUSALS.oltpUnsupportedColumn, { name: this.column, ty: this.nativeType });
  }
}

export const ZOO: ShapeZoo = zooJson as unknown as ShapeZoo;

/** The zoo table's name, from the fixture's digest — the canary's `zooTableName`. */
export function zooTableName(sha256Hex: string = ZOO_SHA256): string {
  return `oxy_shape_zoo_${sha256Hex.slice(0, 8)}`;
}

/** `c001`, `c002`, … — the zoo column for case `index`. */
export function zooColumn(index: number): string {
  return `c${String(index + 1).padStart(3, "0")}`;
}

/** The engine a dialect's cases are filed under. */
export function engineOf(dialect: "clickhouse" | "postgres" | "duckdb"): ZooEngine {
  return dialect;
}

const isError = (v: unknown): v is { $error: string } =>
  typeof v === "object" && v !== null && "$error" in v;

/** What a read of case `c` on `plane` returns — or refuses. */
export function expectation(c: ZooCase, plane: ZooPlane, column: string): unknown | ZooRefusal {
  const value = plane === "oltp" ? c.expect.oltp : c.expect.warehouse;
  return isError(value) ? new ZooRefusal(column, c.native_type, value.$error) : value;
}

/** Every case of `engine` for `nativeType`, as written in the zoo. */
export function casesFor(engine: ZooEngine, nativeType: string): ZooCase[] {
  return ZOO.engines[engine].cases.filter((c) => c.native_type === nativeType);
}

/** The native types the zoo pins for `engine`, for a declaration error to list. */
export function knownTypes(engine: ZooEngine): string[] {
  return [...new Set(ZOO.engines[engine].cases.map((c) => c.native_type))];
}

const I64_MAX = 9223372036854775807n;
const I64_MIN = -9223372036854775808n;

/**
 * Render one SQL-side value of a typed column through the zoo.
 *
 * - The value IS a case's `value_sql` for this type → that case's expectation
 *   (`source: "zoo"`).
 * - `NULL` → `null`.
 * - Otherwise the `plain` case's SHAPE decides: a number-shaped type parses the
 *   literal, except that an integer outside `i64` becomes its exact digits as a
 *   string where the zoo pins that (`precision_gt_18` expecting a string) — the
 *   host reads an overflowing column as `u64` rendered to text; a string-shaped
 *   type unquotes; a boolean-shaped type reads `true`/`false`/`1`/`0`; a JSON
 *   shape parses (`source: "typed"`).
 */
export function renderValue(
  engine: ZooEngine,
  nativeType: string,
  valueSql: string,
  plane: ZooPlane,
  column: string
): { value: unknown | ZooRefusal; source: RowSource } {
  const cases = casesFor(engine, nativeType);
  const raw = valueSql.trim();
  const exact = cases.find((c) => c.value_sql.trim() === raw);
  if (exact) return { value: expectation(exact, plane, column), source: "zoo" };
  if (/^NULL$/i.test(raw)) return { value: null, source: "typed" };
  const plain = cases.find((c) => c.class === "plain") ?? cases[0];
  const shape = expectation(plain, plane, column);
  if (shape instanceof ZooRefusal) return { value: shape, source: "zoo" };
  return { value: shaped(raw, shape, cases, plane, column), source: "typed" };
}

function shaped(
  raw: string,
  shape: unknown,
  cases: ZooCase[],
  plane: ZooPlane,
  column: string
): unknown {
  const unquoted =
    raw.startsWith("'") && raw.endsWith("'") ? raw.slice(1, -1).replace(/''/g, "'") : raw;
  switch (typeof shape) {
    case "number": {
      if (/^-?\d+$/.test(raw)) {
        const big = BigInt(raw);
        if (big > I64_MAX || big < I64_MIN) {
          const wide = cases.find((c) => c.class === "precision_gt_18");
          if (wide && typeof expectation(wide, plane, column) === "string") return raw;
        }
      }
      const n = Number(unquoted);
      return Number.isNaN(n) ? unquoted : n;
    }
    case "boolean":
      return /^(true|1)$/i.test(unquoted);
    case "string":
      return unquoted;
    case "object": {
      if (shape === null) return null;
      try {
        return JSON.parse(unquoted);
      } catch {
        return unquoted;
      }
    }
    default:
      return unquoted;
  }
}
