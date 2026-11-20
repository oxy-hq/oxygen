// Started from pokehouse-oxy:apps/store-ops/src/lib/csv.ts `downloadCsv` (BEYOND-GALLERY row 13:
// the tested copy; Bookkeeping's `lib/downloadCsv.ts` is the same, with the later revoke taken
// here). The document is `toCsv` from `@oxy-hq/sdk/format` (RFC 4180 quoting, formula guard,
// CRLF), which a Function can import too; this half is the browser's: a Blob and a link.
//
// - The UTF-8 byte-order mark goes in front HERE, not in `toCsv`, so a test of the document
//   asserts the document: without it Excel reads UTF-8 as Latin-1 and a name with an accent
//   arrives mangled.
// - The object URL is revoked a second later, not in the same tick: that cancels Safari's
//   download.
// - A filename without `.csv` gets it, so the file opens in a spreadsheet on every system.
//
// The button that starts it is an ordinary `Button` ("Export CSV"); a download takes no focus
// and opens no page, so there is nothing else to draw.

import { type CsvColumn, toCsv } from "../../format";

/** The byte-order mark that makes Excel read the file as UTF-8. */
const BOM = "﻿";

/** Rows as a CSV file the browser downloads: `toCsv(rows, columns)`, named `filename`. */
export function downloadCsv<Row>(
  rows: readonly Row[],
  columns: readonly CsvColumn<Row>[],
  filename: string
): void {
  const blob = new Blob([`${BOM}${toCsv(rows, columns)}`], { type: "text/csv;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = /\.csv$/i.test(filename) ? filename : `${filename}.csv`;
  link.rel = "noopener";
  link.style.display = "none";
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
