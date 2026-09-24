// The small subset of SQL the context evaluates against its in-memory tables.
//
// Deliberately tiny and deliberately loud: `CREATE TABLE`, `INSERT … VALUES`,
// `SELECT <columns | count(*) | literals> FROM <table> [WHERE a = b AND …]
// [ORDER BY col] [LIMIT n]`, `DELETE FROM`, `UPDATE … SET`, and a `SELECT`
// of literals with no `FROM`. Anything else is not guessed at — the store
// throws a `TestContextError` naming the statement and the two ways out
// (`t.override(op, …)` to answer the call, or `t.state.<store>.raw(…)` to
// seed rows). Every VALUES entry keeps its raw SQL text, so a zoo case's
// `value_sql` round-trips exactly and `zoo.ts` can render it.

import { contextError } from "./host-error";

export interface ColumnDef {
  name: string;
  /** The native type as written: `Decimal(18, 4)`, `LowCardinality(String)`. */
  type: string;
}

export interface CreateTable {
  kind: "create";
  table: string;
  ifNotExists: boolean;
  columns: ColumnDef[];
}

export interface Insert {
  kind: "insert";
  table: string;
  /** The column list when written; otherwise the table's declared order. */
  columns: string[] | null;
  /** Each row's values as raw SQL text, in column order. */
  tuples: string[][];
}

export type Predicate = { column: string; values: string[] };

export type Projection =
  | { kind: "all" }
  | { kind: "count"; alias: string }
  | { kind: "column"; column: string; alias: string }
  | { kind: "literal"; raw: string; alias: string };

export interface Select {
  kind: "select";
  table: string | null;
  items: Projection[];
  where: Predicate[];
  orderBy: { column: string; desc: boolean } | null;
  limit: number | null;
}

export interface Delete {
  kind: "delete";
  table: string;
  where: Predicate[];
}

export interface Update {
  kind: "update";
  table: string;
  set: { column: string; raw: string }[];
  where: Predicate[];
}

export type Statement = CreateTable | Insert | Select | Delete | Update;

const STOP_WORDS =
  /^(DEFAULT|NOT|NULL|PRIMARY|UNIQUE|REFERENCES|CHECK|GENERATED|CODEC|COMMENT|TTL)$/i;
const CONSTRAINT_HEAD = /^(PRIMARY|UNIQUE|CONSTRAINT|FOREIGN|CHECK|KEY|INDEX)\b/i;

/** `sql` without its leading line and block comments and whitespace. */
export function stripLeadingComments(sql: string): string {
  let s = sql;
  for (;;) {
    const before = s;
    s = s
      .replace(/^\s+/, "")
      .replace(/^--[^\n]*\n?/, "")
      .replace(/^\/\*[\s\S]*?\*\//, "");
    if (s === before) return s;
  }
}

/** Parse one statement, or throw in the context's own words. */
export function parse(sql: string): Statement {
  const text = stripLeadingComments(sql).replace(/;\s*$/, "");
  const head = /^(\w+)/.exec(text)?.[1]?.toUpperCase() ?? "";
  switch (head) {
    case "CREATE":
      return parseCreate(text);
    case "INSERT":
      return parseInsert(text);
    case "SELECT":
    case "WITH":
      return parseSelect(text);
    case "DELETE":
      return parseDelete(text);
    case "UPDATE":
      return parseUpdate(text);
    default:
      throw unsupported(text);
  }
}

export function unsupported(sql: string): Error {
  const head = sql.trim().slice(0, 60).replace(/\s+/g, " ");
  return contextError(
    `the test context cannot evaluate this statement: "${head}…". It reads CREATE TABLE, INSERT … VALUES, SELECT <columns|count(*)|literals> FROM <table> [WHERE col = value AND …] [ORDER BY col] [LIMIT n], DELETE FROM and UPDATE … SET. Answer the call with t.override(op, …), or seed the rows it should find with t.state.<store>.raw(table, rows).`
  );
}

/** Strip the quotes an identifier was written with. */
export function ident(raw: string): string {
  const s = raw.trim();
  if ((s.startsWith('"') && s.endsWith('"')) || (s.startsWith("`") && s.endsWith("`"))) {
    return s.slice(1, -1);
  }
  return s;
}

/** Split `text` on top-level commas, respecting parentheses, brackets and quotes. */
export function splitTopLevel(text: string, separator = ","): string[] {
  const out: string[] = [];
  let depth = 0;
  let quote: string | null = null;
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quote) {
      if (c === quote) {
        if (text[i + 1] === quote) i++;
        else quote = null;
      }
      continue;
    }
    if (c === "'" || c === '"' || c === "`") quote = c;
    else if (c === "(" || c === "[") depth++;
    else if (c === ")" || c === "]") depth--;
    else if (depth === 0 && text.startsWith(separator, i)) {
      out.push(text.slice(start, i).trim());
      i += separator.length - 1;
      start = i + 1;
    }
  }
  out.push(text.slice(start).trim());
  return out.filter((s) => s !== "");
}

/** The index just past the `)` matching the `(` at `open`. */
function closeParen(text: string, open: number): number {
  let depth = 0;
  let quote: string | null = null;
  for (let i = open; i < text.length; i++) {
    const c = text[i];
    if (quote) {
      if (c === quote) {
        if (text[i + 1] === quote) i++;
        else quote = null;
      }
      continue;
    }
    if (c === "'" || c === '"') quote = c;
    else if (c === "(") depth++;
    else if (c === ")" && --depth === 0) return i + 1;
  }
  throw unsupported(text);
}

function parseCreate(text: string): CreateTable {
  const m = /^CREATE\s+TABLE\s+(IF\s+NOT\s+EXISTS\s+)?([\w."`]+)\s*\(/i.exec(text);
  if (!m) throw unsupported(text);
  const open = m[0].length - 1;
  const end = closeParen(text, open);
  const columns: ColumnDef[] = [];
  for (const def of splitTopLevel(text.slice(open + 1, end - 1))) {
    if (CONSTRAINT_HEAD.test(def)) continue;
    const words = splitTopLevel(def, " ");
    const name = ident(words[0] ?? "");
    const rest = def.slice(def.indexOf(words[0] ?? "") + (words[0]?.length ?? 0)).trim();
    const typeWords = splitTopLevel(rest, " ");
    const stop = typeWords.findIndex((w) => STOP_WORDS.test(w));
    const type = (stop < 0 ? typeWords : typeWords.slice(0, stop)).join(" ");
    if (name) columns.push({ name, type });
  }
  return { kind: "create", table: ident(m[2]), ifNotExists: Boolean(m[1]), columns };
}

function parseInsert(text: string): Insert {
  const m = /^INSERT\s+INTO\s+([\w."`]+)\s*(\(([^)]*)\))?\s*VALUES\s*/i.exec(text);
  if (!m) throw unsupported(text);
  const columns = m[3] ? splitTopLevel(m[3]).map(ident) : null;
  const tuples: string[][] = [];
  let at = m[0].length;
  while (at < text.length) {
    if (text[at] !== "(") throw unsupported(text);
    const end = closeParen(text, at);
    tuples.push(splitTopLevel(text.slice(at + 1, end - 1)));
    at = end;
    const gap = /^\s*,?\s*/.exec(text.slice(at))?.[0] ?? "";
    at += gap.length;
  }
  return { kind: "insert", table: ident(m[1]), columns, tuples };
}

/** `a = 1 AND b = 'x' AND c IN (1, 2)` → predicates; anything else throws. */
function parseWhere(text: string | undefined, whole: string): Predicate[] {
  if (!text) return [];
  return splitTopLevel(text, " AND ").map((clause) => {
    const eq = /^([\w."`]+)\s*=\s*(.+)$/s.exec(clause);
    if (eq) return { column: ident(eq[1]), values: [eq[2].trim()] };
    const inList = /^([\w."`]+)\s+IN\s*\((.*)\)$/is.exec(clause);
    if (inList) return { column: ident(inList[1]), values: splitTopLevel(inList[2]) };
    throw contextError(
      `the test context cannot evaluate this WHERE clause: "${clause}". It reads "col = value" and "col IN (…)" joined by AND. Answer the call with t.override(op, …) instead: ${whole.slice(0, 80)}`
    );
  });
}

/**
 * Whether `literal()` will read this expression as a value rather than hand it
 * back as its own SQL text. The two must agree: a projection the parser accepts
 * as a literal but `literal()` cannot read comes back to the caller as the SQL
 * they wrote — a column named `sum(amount)` holding the string `"sum(amount)"`,
 * which is the one thing this module promises never to do.
 */
function isReadableLiteral(expr: string): boolean {
  const s = expr.trim();
  return (
    /^\$\d+$/.test(s) ||
    /^(NULL|TRUE|FALSE)$/i.test(s) ||
    (s.startsWith("'") && s.endsWith("'") && s.length >= 2) ||
    /^-?\d+(\.\d+)?([eE][+-]?\d+)?$/.test(s)
  );
}

/**
 * The statement with every quoted string blanked, so a keyword scan reads the
 * SQL and not the data: `WHERE label = 'GROUP BY'` is a row, not a grouping.
 */
function withoutStringLiterals(text: string): string {
  return text.replace(/'(?:[^']|'')*'/g, "''");
}

function parseSelect(text: string): Select {
  // GROUP BY used to be an optional, non-capturing branch of the regex below:
  // it matched, nothing read it, and the grouping silently vanished — so
  // `SELECT status, count(*) FROM orders GROUP BY status` answered with one
  // ungrouped row and no `status` column. Refusing is the contract; aggregating
  // for real is not something a fake should attempt.
  if (/\bGROUP\s+BY\b/is.test(withoutStringLiterals(text))) {
    throw contextError(
      `the test context cannot evaluate GROUP BY: "${text.trim().slice(0, 60).replace(/\s+/g, " ")}…". ` +
        `It would have to aggregate, and a wrong aggregate reads like a right one. ` +
        `Answer the call with t.override(op, …) and assert on the arguments instead.`
    );
  }
  const m =
    /^SELECT\s+(.*?)(?:\s+FROM\s+([\w."`]+))?(?:\s+WHERE\s+(.*?))?(?:\s+ORDER\s+BY\s+([\w."`]+)(?:\s+(ASC|DESC))?)?(?:\s+LIMIT\s+(\d+))?\s*$/is.exec(
      text
    );
  if (!m || (!m[2] && /\bFROM\b/i.test(m[1]))) throw unsupported(text);
  const items: Projection[] = splitTopLevel(m[1]).map((item) => {
    if (item === "*") return { kind: "all" };
    const aliased = /^(.*?)\s+AS\s+([\w"`]+)$/is.exec(item);
    const expr = (aliased ? aliased[1] : item).trim();
    const alias = aliased ? ident(aliased[2]) : null;
    if (/^count\s*\(\s*\*\s*\)$/i.test(expr)) return { kind: "count", alias: alias ?? "count(*)" };
    if (/^[\w."`]+$/.test(expr) && !/^\d/.test(expr) && !/^(true|false|null)$/i.test(expr)) {
      return { kind: "column", column: ident(expr), alias: alias ?? ident(expr) };
    }
    // Everything else used to fall through as a literal, which handed the SQL
    // back as its own value: `sum(amount)`, `DISTINCT a`, a cast, a window.
    if (!isReadableLiteral(expr)) throw unsupported(text);
    return { kind: "literal", raw: expr, alias: alias ?? expr };
  });
  return {
    kind: "select",
    table: m[2] ? ident(m[2]) : null,
    items,
    where: parseWhere(m[3], text),
    orderBy: m[4] ? { column: ident(m[4]), desc: (m[5] ?? "").toUpperCase() === "DESC" } : null,
    limit: m[6] ? Number(m[6]) : null
  };
}

function parseDelete(text: string): Delete {
  const m = /^DELETE\s+FROM\s+([\w."`]+)(?:\s+WHERE\s+(.*))?\s*$/is.exec(text);
  if (!m) throw unsupported(text);
  return { kind: "delete", table: ident(m[1]), where: parseWhere(m[2], text) };
}

function parseUpdate(text: string): Update {
  const m = /^UPDATE\s+([\w."`]+)\s+SET\s+(.*?)(?:\s+WHERE\s+(.*))?\s*$/is.exec(text);
  if (!m) throw unsupported(text);
  const set = splitTopLevel(m[2]).map((assignment) => {
    const eq = /^([\w."`]+)\s*=\s*(.+)$/s.exec(assignment);
    if (!eq) throw unsupported(text);
    // Same rule as a projection, for the same reason: an assignment `literal()`
    // cannot read is stored as its own SQL text, so `SET n = n + 1` would leave
    // the string `"n + 1"` in a numeric column and the test would pass on it.
    const raw = eq[2].trim();
    if (!isReadableLiteral(raw)) throw unsupported(text);
    return { column: ident(eq[1]), raw };
  });
  return { kind: "update", table: ident(m[1]), set, where: parseWhere(m[3], text) };
}

/**
 * A raw SQL value as a JS value, with no engine's rendering applied: a quoted
 * string, a number, NULL, TRUE/FALSE, or a `$n` bound parameter. Anything else
 * (a function call, a cast, an array literal) stays as its SQL text.
 */
export function literal(raw: string, params: readonly unknown[] = []): unknown {
  const s = raw.trim();
  const param = /^\$(\d+)$/.exec(s);
  if (param) return params[Number(param[1]) - 1];
  if (/^NULL$/i.test(s)) return null;
  if (/^TRUE$/i.test(s)) return true;
  if (/^FALSE$/i.test(s)) return false;
  if (s.startsWith("'") && s.endsWith("'")) return s.slice(1, -1).replace(/''/g, "'");
  if (/^-?\d+(\.\d+)?([eE][+-]?\d+)?$/.test(s)) return Number(s);
  return s;
}
