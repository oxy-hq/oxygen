// A table as a CSV document somebody opens in a spreadsheet. Text only: the download (a Blob
// and a link) belongs to the ui entry, so this runs in an Oxy Function too.
//
// Started from store-ops' `src/lib/csv.ts` (tested, the most careful of the four copies;
// locations' and bookkeeping's quote the same way but neither guards formulas):
//
// - **Quoting is RFC 4180**: a field with a comma, a quote, a CR or an LF is wrapped in
//   quotes and its quotes doubled. A field with leading or trailing spaces is quoted too, so
//   the spaces survive the round trip.
// - **A text field starting `=`, `+`, `-`, `@`, a tab or a CR gets a leading apostrophe**, so
//   a spreadsheet shows it as text instead of running it as a formula (OWASP's CSV injection
//   advice). Numbers are never touched, and neither is a string that is only a decimal
//   number (`-4`, `-0.5`): it has no operator after the sign for a formula to ride on.
// - **CRLF between rows, none after the last**, as RFC 4180 and Excel expect.
// - **A missing value is an empty field**, never `NaN` or `null`: null, undefined, NaN and
//   ±Infinity.
//
// The byte-order mark Excel needs to read UTF-8 is the download's job, not this text's.

/** One value of a CSV field. */
export type CsvCell = string | number | bigint | boolean | null | undefined;

/** One column: its heading, and the row's key or a function that reads the value. */
export interface CsvColumn<Row> {
  header: string;
  /** A key of the row, or a reader. A key's value that is not a {@link CsvCell} is written as text: a `Date` as ISO, an object or array as JSON. */
  value: (keyof Row & string) | ((row: Row, index: number) => CsvCell);
}

const NEEDS_QUOTE = /[",\r\n]/;
const FORMULA_LEAD = /^[=+\-@\t\r]/;
const PLAIN_NUMBER = /^-?\d+(\.\d+)?$/;

function asCell(value: unknown): CsvCell {
  if (value === null || value === undefined) return value;
  switch (typeof value) {
    case "string":
    case "number":
    case "bigint":
    case "boolean":
      return value;
    default:
      if (Object.prototype.toString.call(value) === "[object Date]") {
        const date = value as Date;
        return Number.isNaN(date.getTime()) ? null : date.toISOString();
      }
      // An object or array is written as JSON, never "[object Object]"; a symbol or a
      // function by its own description.
      if (typeof value === "object") return JSON.stringify(value) ?? null;
      return String(value as symbol | ((...args: unknown[]) => unknown));
  }
}

/**
 * One field, escaped and quoted as the file header says.
 *
 * @example csvCell('Say "hi"') // "\"Say \"\"hi\"\"\""
 * @example csvCell("=SUM(A1:A9)") // "'=SUM(A1:A9)"
 * @example csvCell(-5) // "-5"
 */
export function csvCell(value: CsvCell): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "number") return Number.isFinite(value) ? String(value) : "";
  if (typeof value !== "string") return String(value);
  let text = value;
  if (FORMULA_LEAD.test(text) && !PLAIN_NUMBER.test(text)) text = `'${text}`;
  if (NEEDS_QUOTE.test(text) || text !== text.trim()) return `"${text.replace(/"/g, '""')}"`;
  return text;
}

/**
 * Rows as a CSV document: a header row from the columns, then one line per row, CRLF between
 * lines and none after the last. No rows gives the header alone.
 *
 * @example
 * toCsv(stores, [
 *   { header: "Store", value: "name" },
 *   { header: "Sales", value: (s) => s.sales }
 * ]);
 */
export function toCsv<Row>(rows: readonly Row[], columns: readonly CsvColumn<Row>[]): string {
  const lines = [columns.map((column) => csvCell(column.header)).join(",")];
  rows.forEach((row, index) => {
    const cells = columns.map(({ value }) =>
      csvCell(typeof value === "function" ? value(row, index) : asCell(row[value]))
    );
    lines.push(cells.join(","));
  });
  return lines.join("\r\n");
}
