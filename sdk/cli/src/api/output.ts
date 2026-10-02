/**
 * Turning a response body into what the reader actually wanted.
 *
 * The default is the body VERBATIM, exactly as `gh api` does it, and that is a
 * deliberate refusal to be clever: the moment stdout depends on whether a
 * terminal is attached, `oxyc api … | jq` and `oxyc api …` stop being the same
 * command, and every script written against one breaks under the other. What
 * TTY-ness may change is pretty-printing whitespace — never structure.
 *
 * `--md` is the opt-in that matters for the agent audience: an array of
 * objects as a markdown table is dramatically cheaper than the same data as
 * JSON, because JSON repeats every field name on every row, and LLMs read
 * markdown tables more reliably than either JSON or aligned ASCII.
 */

import { spawnSync } from "node:child_process";
import { type Column, table } from "../ui/render.js";
import { stdoutIsTty } from "../ui/tty.js";
import { CliError, ExitCode } from "../util/errors.js";
import { parseJson } from "./request.js";

export interface OutputOptions {
  /** A jq program to run over the body. */
  jq?: string;
  /** Render arrays of objects as a markdown table. */
  md?: boolean;
  /** Print nothing at all. */
  silent?: boolean;
  /** Pretty-print JSON even when piped. */
  pretty?: boolean;
}

/**
 * Run `--jq` by shelling out to jq.
 *
 * The alternatives were each worse. A WASM jq adds megabytes to a package
 * whose selling point is `npx` start-up; a hand-written JSONPath subset would
 * be a *different language* wearing jq's flag name, and the first `--jq` that
 * silently means something else is worse than not having the flag. jq is
 * already a stated prerequisite of the tooling this command absorbs, so the
 * dependency is not new — and when it is missing the error says so and names
 * the install line, rather than producing wrong output.
 */
export function runJq(input: string, program: string): string {
  const result = spawnSync("jq", ["-r", program], {
    input,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024
  });

  if (result.error) {
    const isMissing = (result.error as NodeJS.ErrnoException).code === "ENOENT";
    throw new CliError(
      isMissing
        ? "--jq needs the `jq` binary, which is not on PATH"
        : `jq failed: ${result.error.message}`,
      {
        code: isMissing ? ExitCode.USAGE : ExitCode.FAILURE,
        hint: isMissing ? "brew install jq   (or apt-get install jq)" : undefined
      }
    );
  }
  if (result.status !== 0) {
    throw new CliError(`jq exited ${result.status}`, {
      code: ExitCode.USAGE,
      detail: result.stderr?.trim() || undefined,
      hint: "check the jq program — `oxyc api <path>` without --jq shows the shape"
    });
  }
  return result.stdout;
}

/**
 * Render an array of objects as a markdown table.
 *
 * Columns are the union of every row's keys IN FIRST-SEEN ORDER, not sorted:
 * handlers put the identifying field first, and alphabetising would bury `id`
 * and `name` in the middle of a row nobody can then scan.
 *
 * A nested value becomes compact JSON in its cell rather than `[object Object]`
 * — ugly, but it is still the data, and a cell that silently says nothing is
 * how a caller concludes a field is empty when it is populated.
 */
export function toMarkdown(payload: unknown): string | undefined {
  const columnar = fromColumnarShapes(payload);
  if (columnar) return columnar;

  const rows = Array.isArray(payload) ? payload : findFirstArray(payload);
  if (!rows || rows.length === 0) return undefined;
  if (!rows.every((r) => typeof r === "object" && r !== null && !Array.isArray(r))) {
    return undefined;
  }

  const keys: string[] = [];
  for (const row of rows as Record<string, unknown>[]) {
    for (const key of Object.keys(row)) {
      if (!keys.includes(key)) keys.push(key);
    }
  }
  const columns: Column<Record<string, unknown>>[] = keys.map((key) => ({
    header: key,
    value: (row) => cell(row[key])
  }));
  return table(rows as Record<string, unknown>[], columns);
}

/**
 * The two columnar shapes this API answers queries with.
 *
 * Handled explicitly because they are the MOST COMMON data response here and
 * neither is an array of objects, so the generic path below rejects both and a
 * caller asking for a table of query results — the single likeliest use of
 * `--md` — got raw JSON instead.
 *
 *   `[["id","name"],["1","ada"]]`          header row first; what
 *                                          `/sql/query` returns by default
 *   `{columns:[...], rows:[[...]]}`        what `/projects/*_/query` returns
 *
 * A single-row array-of-arrays is a header with no data, which renders as an
 * empty table rather than being mistaken for one data row.
 */
function fromColumnarShapes(payload: unknown): string | undefined {
  if (Array.isArray(payload) && payload.length > 0 && payload.every(Array.isArray)) {
    const [header, ...body] = payload as unknown[][];
    return renderColumnar((header ?? []).map(String), body);
  }
  if (typeof payload === "object" && payload !== null) {
    const record = payload as Record<string, unknown>;
    // `rows` must be rows — ARRAYS. A payload carrying `columns` and `rows`
    // where the rows are objects is a different shape entirely, and indexing
    // objects positionally renders a table of empty cells. Falling through to
    // the generic array-of-objects path below gets it right instead of
    // confidently getting it wrong.
    if (
      Array.isArray(record.columns) &&
      Array.isArray(record.rows) &&
      record.rows.every((r) => Array.isArray(r))
    ) {
      return renderColumnar(record.columns.map(String), record.rows as unknown[][]);
    }
  }
  return undefined;
}

function renderColumnar(headers: string[], body: unknown[][]): string {
  // KEYED BY INDEX, not by header name. `SELECT a.id, b.id` gives two columns
  // called `id`, and building a row object off the names collapses them — both
  // columns then print the second one's value, silently and identically.
  const rows = body.map((row) => headers.map((_h, i) => row?.[i] ?? null));
  const rendered = table(
    rows,
    headers.map((h, i) => ({ header: h, value: (r: unknown[]) => cell(r[i]) }))
  );
  if (rendered) return rendered;

  // Zero rows. `table()` prints nothing for an empty set, because a bare
  // header there is a claim it cannot support — it does not know whether the
  // set was inspected. HERE it was: the query ran and came back with these
  // columns and no rows, so the header IS the answer, and it tells the caller
  // what they queried as well as that it was empty.
  //
  // Rendered through `table()` with a single placeholder row rather than by
  // hand-writing pipes: the by-hand version was markdown unconditionally, so
  // on a terminal the same query came back aligned when it had rows and
  // pipe-delimited when it did not.
  return (
    table(
      [headers.map(() => "")],
      headers.map((h, i) => ({
        header: h,
        value: (r: string[]) => r[i] ?? ""
      }))
    )
      .split("\n")
      // Drop the placeholder row, keeping whichever header form the stream got.
      .slice(0, stdoutIsTty() ? 1 : 2)
      .join("\n")
  );
}

function cell(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return JSON.stringify(value);
}

/**
 * The rows of a list response: the first array-of-OBJECTS field.
 *
 * Not merely the first array. A response like `{columns: ["a"], rows: [{…}]}`
 * has two array fields and only the second is rows — taking the first found
 * `columns`, saw an array of strings, and gave up, so a payload the generic
 * path could have rendered came back as raw JSON instead.
 *
 * Falls back to the first array of any kind so a response whose rows are
 * genuinely scalars is still found and then rejected by the caller's own
 * shape check, rather than being invisible here.
 */
function findFirstArray(payload: unknown): unknown[] | undefined {
  if (typeof payload !== "object" || payload === null) return undefined;
  const arrays = Object.values(payload as Record<string, unknown>).filter(Array.isArray);
  return (
    arrays.find(
      (value) =>
        value.length > 0 &&
        value.every((v) => typeof v === "object" && v !== null && !Array.isArray(v))
    ) ?? arrays[0]
  );
}

/**
 * The final bytes for a response body.
 *
 * Order matters: `--jq` runs on the raw body, so a program can see fields
 * `--md` would have flattened, and `--md` then runs on jq's output so
 * `--jq '.threads' --md` is a table of exactly the rows you selected.
 */
export function formatBody(body: string, opts: OutputOptions): string {
  if (opts.silent) return "";

  let current = body;
  if (opts.jq) current = runJq(current, opts.jq);

  if (opts.md) {
    const rendered = toMarkdown(parseJson(current));
    // Not table-shaped — a scalar, a nested object, a jq program that emitted
    // lines. Falling back to the value is right: `--md` asks for the most
    // readable form available, and refusing to print would lose the answer.
    if (rendered !== undefined) return rendered;
  }

  if (opts.pretty || (stdoutIsTty() && !opts.jq)) {
    const parsed = parseJson(current);
    if (parsed !== undefined) return JSON.stringify(parsed, null, 2);
  }
  return current;
}
