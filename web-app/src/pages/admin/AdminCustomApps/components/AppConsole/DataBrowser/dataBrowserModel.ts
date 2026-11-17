import type {
  AppDataCell,
  AppDataColumn,
  AppDataOrderTerm,
  AppDataRows,
  AppDataStore
} from "@/types/appData";
import { formatBytes } from "../../StorageTab/utils";

/** How one value reads in the grid. */
export interface CellView {
  text: string;
  /** SQL `NULL` — drawn apart from the text "NULL" and from an empty string. */
  isNull: boolean;
  /** Said on hover: why the value is not shown whole. */
  title?: string;
}

/** One cell as the grid draws it. The server has already cut it; this says so. */
export function cellView(cell: AppDataCell, textChars: number): CellView {
  if (cell === null) return { text: "NULL", isNull: true };
  if ("json" in cell) return { text: JSON.stringify(cell.json), isNull: false };
  if ("hex" in cell) {
    return {
      text: `\\x${cell.hex}${cell.truncated ? "…" : ""}`,
      isNull: false,
      title: `bytea, ${formatBytes(cell.size)}${cell.truncated ? " — first bytes shown" : ""}`
    };
  }
  if (!cell.truncated) return { text: cell.text, isNull: false };
  const whole = cell.size === undefined ? "" : ` of ${formatBytes(cell.size)}`;
  return {
    text: `${cell.text}…`,
    isNull: false,
    title: `Cut at ${textChars} characters${whole}`
  };
}

/** `pg_class.reltuples` as a person reads it. Never analyzed is not "0 rows". */
export function approxRowsLabel(approx: number | null): string {
  if (approx === null) return "not analyzed";
  return `~${approx.toLocaleString("en-US")} row${approx === 1 ? "" : "s"}`;
}

/** A header's hover text: the column's type, and whether it can order the page. */
export function headerTitle(column: AppDataColumn): string {
  const facts = `${column.type}${column.nullable ? "" : " not null"}${column.primary_key ? ", primary key" : ""}`;
  return column.sortable
    ? `${facts} — order by this column`
    : `${facts} — Postgres cannot order by this type`;
}

/** Clicking a column header: ascending, then descending, then back to the table's
 *  own order (its primary key). Another column starts again at ascending. */
export function nextOrder(
  current: AppDataOrderTerm | undefined,
  column: string
): AppDataOrderTerm | undefined {
  if (current?.column !== column) return { column, dir: "asc" };
  return current.dir === "asc" ? { column, dir: "desc" } : undefined;
}

/**
 * A page's rows, each with its one-based position in the table's order and each cell
 * beside its column's name. A row has no identity of its own (a table may have no
 * key) and a page is replaced whole, never reordered, so its position is its key.
 */
export function pageRows(page: Pick<AppDataRows, "rows" | "columns" | "offset">): {
  position: number;
  cells: { column: string; cell: AppDataCell }[];
}[] {
  return page.rows.map((row, i) => ({
    position: page.offset + i + 1,
    cells: row.map((cell, c) => ({ column: page.columns[c]?.name ?? `#${c + 1}`, cell }))
  }));
}

/** "Rows 51–100", for the pager. */
export function pageRange(offset: number, count: number): string {
  if (count === 0)
    return offset === 0 ? "No rows" : `No rows past ${offset.toLocaleString("en-US")}`;
  return `Rows ${(offset + 1).toLocaleString("en-US")}–${(offset + count).toLocaleString("en-US")}`;
}

/** A wide table reads fewer rows a page than asked: the page is capped by size. */
export function narrowedNote(rows: Pick<AppDataRows, "limit" | "requested_limit">): string | null {
  if (rows.limit >= rows.requested_limit) return null;
  return `${rows.limit} rows a page: this table is wide, and a page is capped by size.`;
}

/** The empty state of an environment with no OLTP store, in the server's words. */
export function absentCopy(
  store: Extract<AppDataStore, { state: "absent" }>,
  environmentLabel: string
): { title: string; description: string } {
  const reason = store.reason.trim();
  const sentence = reason ? `${reason.charAt(0).toUpperCase()}${reason.slice(1)}` : "";
  return {
    title: `No OLTP data to browse in ${environmentLabel}.`,
    description: sentence && !/[.!?]$/.test(sentence) ? `${sentence}.` : sentence
  };
}
