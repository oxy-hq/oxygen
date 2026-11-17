/**
 * The console's read-only browser of an app's own OLTP data
 * (`GET /customer-apps/{id}/data/tables` and `…/data/rows`), as
 * `admin::apps::data` serializes it.
 */

/** How a column's values render. */
export type AppDataColumnKind = "text" | "json" | "bytes" | "other";

export interface AppDataColumn {
  name: string;
  /** Postgres's own spelling: `text`, `character varying(40)`, `numeric(10,2)`. */
  type: string;
  kind: AppDataColumnKind;
  nullable: boolean;
  primary_key: boolean;
  /** Whether Postgres can order by it. `json`, `xml` and `point` cannot, so the grid
   *  offers no sort on them; the server refuses one anyway. */
  sortable: boolean;
}

export interface AppDataTable {
  name: string;
  /** Postgres's estimate from the last analyze; `null` when never analyzed, which is
   *  not zero rows. */
  approx_rows: number | null;
  columns: AppDataColumn[];
}

/** Where the environment's data is, or why it has none. */
export type AppDataStore =
  | { state: "ready"; schema: string }
  | { state: "absent"; code: string; reason: string };

/** The caps every read is made under. */
export interface AppDataCaps {
  max_rows: number;
  text_chars: number;
  bytes_shown: number;
  statement_timeout_ms: number;
}

export interface AppDataTables {
  environment: string;
  store: AppDataStore;
  tables: AppDataTable[];
  caps: AppDataCaps;
}

/**
 * One value. `null` is SQL `NULL`. Text is cut at `caps.text_chars` (then `truncated`,
 * and `size` in bytes when known); JSON that fits arrives as JSON; a `bytea` sends its
 * first bytes in hex and its size.
 */
export type AppDataCell =
  | null
  | { text: string; truncated?: boolean; size?: number }
  | { json: unknown }
  | { hex: string; size: number; truncated: boolean };

export type AppDataDir = "asc" | "desc";

export interface AppDataOrderTerm {
  column: string;
  dir: AppDataDir;
}

export interface AppDataRows {
  environment: string;
  schema: string;
  table: string;
  columns: AppDataColumn[];
  /** One array per row, one cell per column in `columns`' order. */
  rows: AppDataCell[][];
  /** The page size read — fewer than `requested_limit` for a wide table. */
  limit: number;
  requested_limit: number;
  offset: number;
  /** The column asked for, then the primary key as a tie break. */
  order: AppDataOrderTerm[];
  has_more: boolean;
  caps: AppDataCaps;
}

/** What to read of one table. */
export interface AppDataPage {
  table: string;
  limit: number;
  offset: number;
  order?: AppDataOrderTerm;
}
