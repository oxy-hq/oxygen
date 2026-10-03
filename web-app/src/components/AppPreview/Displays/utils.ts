import { DataType, type Float, Precision, Struct, Type } from "apache-arrow";

// Minimal structural interface for Apache Arrow Table/Schema so that the
// version bundled by @duckdb/duckdb-wasm (v17) and our own (v21) are both
// accepted without a hard type dependency on a specific Arrow release.
interface ArrowSchema {
  readonly fields: ReadonlyArray<{ readonly name: string; readonly type: DataType }>;
}
interface ArrowTable {
  readonly schema: ArrowSchema;
  toArray(): unknown[];
}

import dayjs from "dayjs";
import timezone from "dayjs/plugin/timezone";
import utc from "dayjs/plugin/utc";

dayjs.extend(utc);
dayjs.extend(timezone);

import { getDuckDB } from "@/libs/duckdb";
import { encodeBase64 } from "@/libs/encoding";
import { toText } from "@/libs/utils/string";
import { apiClient } from "@/services/api/axios";
import type { DataContainer, DisplayFormat } from "@/types/app";

const getArrowValue = (value: unknown): number | string | unknown => {
  if (value instanceof Uint32Array) return String(wordsToBigInt(value));
  if (value instanceof Float32Array) return singlePrecisionText(value[0]);
  if (value instanceof Float64Array) return String(value[0]);
  if (typeof value === "bigint" || typeof value === "number") return String(value);
  return value;
};

const columnCells = (table: ArrowTable, columnName: string): unknown[] =>
  table.toArray().map((row: unknown) => (row as Record<string, unknown>)[columnName]);

/**
 * A chart's column, each cell as it is labelled: on the x axis, as a series'
 * name or a pie slice's, and as the value plotted. A number is the text of the
 * value the column holds, which is what ECharts plots: a decimal keeps the
 * digits of its declared scale and a float is not rounded, so 0.004 is plotted
 * at 0.004, not at "0.00". Moments are labelled to the precision the column
 * needs: to the minute, unless one has seconds, or milliseconds.
 *
 * A number stays plain text, without thousands separators: ECharts reads a
 * numeric string back as a number only when it is one (`"43,149,473.45"` is
 * not, and blanks the axis and the line). Formatting with commas and `$`
 * belongs to the labels (`formatValue`, the chart's tooltip formatter).
 *
 * Two moments can share a label only when they are equal. To match a series'
 * points to the axis, use `getArrowColumnKeys`: a series holding only whole
 * minutes is labelled to the minute while the axis it goes on has seconds.
 */
export const getArrowColumnValues = (table: ArrowTable, columnName: string) => {
  const fieldType = getArrowFieldType(columnName, table.schema);
  const cells = columnCells(table, columnName);
  if (!fieldType) return cells.map(getArrowValue);
  const momentFormat = momentLabelFormat(cells, fieldType);
  return cells.map((value) =>
    momentFormat && value !== null && value !== undefined
      ? momentText(value, fieldType, momentFormat)
      : getArrowResultCell(value, fieldType)
  );
};

/**
 * A chart's column, each cell as the full value it holds, so that two cells
 * share a key only when they are equal: what a series' points are matched to
 * the x axis by, and what a series' rows are selected by. A moment's key is its
 * instant in UTC (`2024-03-05T12:34:56.000Z`), which DuckDB reads as that
 * instant whatever its own time zone, for a TIMESTAMP column and for one WITH
 * TIME ZONE. The clock time in the column's zone, which its label shows, is not
 * one: 01:30 comes twice the night New York's clocks go back. Every other cell
 * is keyed by `getArrowResultCell`'s text.
 */
export const getArrowColumnKeys = (table: ArrowTable, columnName: string) => {
  const fieldType = getArrowFieldType(columnName, table.schema);
  return columnCells(table, columnName).map((value) => {
    if (!fieldType) return getArrowValue(value);
    const millis = momentMillis(value, fieldType);
    return millis === null ? getArrowResultCell(value, fieldType) : instantText(millis);
  });
};

/**
 * One Arrow cell as the value it holds: what a table shows (a query result, or
 * an app table's column with no format) and what a chart plots. A number is the
 * value the column holds: a decimal keeps every digit of its declared scale and
 * a float is not rounded (0.004 is not "0.00", 0.123456 is not "0.12"), nor
 * padded (1.5 is not "1.50"). A date reads as a date, and a timestamp keeps its
 * seconds, and its milliseconds when it has any, as the CSV export does
 * (12:34:56 is not "12:34").
 */
export const getArrowResultCell = (value: unknown, type: DataType): unknown => {
  // A NULL cell stays null whatever its column type, for the caller to show as
  // it shows any other NULL. The readers below expect a value: a NULL decimal
  // throws in them and a NULL date reads "Invalid Date".
  if (value === null || value === undefined) return value;
  if (DataType.isDate(type)) return formatDate(value as number);
  if (DataType.isTime(type)) return formatTime(value as number);
  const moment = fullTimestampText(value, type);
  if (moment !== null) return moment;
  if (DataType.isDecimal(type)) return decimalText(value, type.scale);
  if (DataType.isFloat(type) && typeof value === "number") return floatText(value, type);
  return getArrowValue(value);
};

/**
 * The signed integer a run of little-endian 32-bit words holds, in two's
 * complement: a DECIMAL cell's unscaled value. DuckDB hands over a HUGEINT, and
 * the SUM of an integer column, as DECIMAL(38, 0), so this is also how those
 * are read: whole, where the first word alone is the value modulo 2^32.
 */
const wordsToBigInt = (words: Uint32Array): bigint => {
  let value = 0n;
  for (let index = words.length - 1; index >= 0; index--) {
    value = (value << 32n) | BigInt(words[index]);
  }
  return BigInt.asIntN(words.length * 32, value);
};

/**
 * The exact decimal an Arrow DECIMAL cell stands for, e.g. "1234.56". The cell
 * holds only the unscaled integer (123456); the scale is on the column's type.
 */
const decimalText = (value: unknown, scale: number): string => {
  // The cell is its words (a Uint32Array that Arrow's BigNum extends), read whole
  // here: BigNum.valueOf() / Number(bigNum) throws past Number.MAX_SAFE_INTEGER.
  // Anything else that stands for a decimal prints its own digits.
  const rawStr =
    value instanceof Uint32Array
      ? String(wordsToBigInt(value))
      : (value as { toString(): string }).toString();
  if (!scale) return rawStr;
  const isNeg = rawStr.startsWith("-");
  const digits = isNeg ? rawStr.slice(1) : rawStr;
  const padded = digits.padStart(scale + 1, "0");
  return `${isNeg ? "-" : ""}${padded.slice(0, -scale)}.${padded.slice(-scale)}`;
};

/**
 * A float as the shortest decimal that reads back as the value the column
 * holds, e.g. "0.123456". A DOUBLE prints as JavaScript prints any number. A
 * single-precision cell arrives widened to a double, where 0.1 has become
 * 0.10000000149011612: those extra digits are not in the column, so the
 * shortest decimal that is still the same single-precision value is printed.
 */
const floatText = (value: number, type: Float): string =>
  type.precision === Precision.SINGLE ? singlePrecisionText(value) : String(value);

const singlePrecisionText = (value: number): string => {
  if (!Number.isFinite(value)) return String(value);
  // Nine significant digits always identify a single-precision value.
  for (let digits = 1; digits <= 9; digits++) {
    const rounded = Number(value.toPrecision(digits));
    if (Math.fround(rounded) === value) return String(rounded);
  }
  return String(value);
};

const MINUTE_FORMAT = "YYYY-MM-DD HH:mm";
const SECOND_FORMAT = "YYYY-MM-DD HH:mm:ss";
const MILLISECOND_FORMAT = "YYYY-MM-DD HH:mm:ss.SSS";
const withoutZeroMillis = (timestamp: string) => timestamp.replace(/\.000$/, "");

/** The instant `millis` since the epoch stands for, in UTC: `2024-03-05T12:34:56.789Z`. */
const instantText = (millis: number) => dayjs.utc(millis).format("YYYY-MM-DDTHH:mm:ss.SSS[Z]");

/**
 * The milliseconds since the epoch a timestamp cell, or a Snowflake one, stands
 * for; null for any other cell.
 */
const momentMillis = (value: unknown, type: DataType): number | null => {
  if (value === null || value === undefined) return null;
  if (DataType.isTimestamp(type)) return value as number;
  // in the BE we are using snowflake-rs library which doesn't return field metadata
  // so there is no way to know if a field is snowflake timestamp or not
  // except checking the structure of the value itself
  if (isSnowflakeTimestamp(value, type)) return snowflakeMillis(value as SnowflakeTimestamp);
  return null;
};

/** A timestamp cell, or a Snowflake one, in `format`. */
const momentText = (value: unknown, type: DataType, format: string): string =>
  DataType.isTimestamp(type)
    ? formatDateTime(value as number, type.timezone, format)
    : dayjs.utc(snowflakeMillis(value as SnowflakeTimestamp)).format(format);

/** A timestamp cell to the second, or a Snowflake one; null for any other cell. */
const fullTimestampText = (value: unknown, type: DataType): string | null =>
  momentMillis(value, type) === null
    ? null
    : withoutZeroMillis(momentText(value, type, MILLISECOND_FORMAT));

/**
 * The one format that shows every moment of a column to the precision it has:
 * to the minute, or to the second when one has seconds, or to the millisecond
 * when one has those. Null for a column that holds no moment.
 */
const momentLabelFormat = (cells: unknown[], type: DataType): string | null => {
  let format: string | null = null;
  for (const value of cells) {
    const millis = momentMillis(value, type);
    if (millis === null) continue;
    if (millis % 1000 !== 0) return MILLISECOND_FORMAT;
    if (millis % 60_000 !== 0) format = SECOND_FORMAT;
    else format ??= MINUTE_FORMAT;
  }
  return format;
};

/**
 * Text for one Arrow cell in an export. It reads the cell the way the result
 * table does: a decimal keeps every digit, a float its own precision, a date
 * or timestamp is a date, not its epoch, and a timestamp keeps its seconds.
 */
export const getArrowExportText = (value: unknown, type?: DataType): string => {
  if (value === null || value === undefined || !type) return cellText(value);
  if (DataType.isDecimal(type)) return decimalText(value, type.scale);
  if (DataType.isFloat(type) && typeof value === "number") return floatText(value, type);
  if (DataType.isDate(type)) return formatDate(value as number);
  if (DataType.isTime(type)) return formatTime(value as number);
  return fullTimestampText(value, type) ?? cellText(value);
};

function isSnowflakeTimestamp(value: unknown, type: DataType): boolean {
  return (
    Struct.isStruct(type) &&
    typeof value === "object" &&
    value !== null &&
    "epoch" in value &&
    "fraction" in value
  );
}

type SnowflakeTimestamp = { epoch: number | bigint; fraction: number | bigint };

function snowflakeMillis(value: SnowflakeTimestamp): number {
  const epoch = typeof value.epoch === "bigint" ? Number(value.epoch) : value.epoch;
  const fraction = typeof value.fraction === "bigint" ? Number(value.fraction) : value.fraction;
  return epoch * 1000 + Math.floor(fraction / 1_000_000);
}

function formatDate(value: number | string): string {
  return dayjs.utc(value).format("YYYY-MM-DD");
}

function formatDateTime(value: number | string, tz: string | null | undefined, format: string) {
  if (tz) return dayjs(value).tz(tz).format(format);
  return dayjs.utc(value).format(format);
}

function formatTime(value: number | bigint | string): string {
  if (typeof value === "bigint") {
    // DuckDB returns TIME as BigInt microseconds since midnight
    const totalSeconds = Number(value / 1000000n);
    const hours = Math.floor(totalSeconds / 3600);
    const minutes = Math.floor((totalSeconds % 3600) / 60);
    const seconds = totalSeconds % 60;
    return `${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
  }
  return dayjs.utc(value).format("HH:mm:ss");
}

/**
 * Monetary column-name detection. When a numeric column's name contains any of
 * these words (see `inferColumnFormat` for the split), and none that says otherwise
 * (see `inferColumnFormat`), the value is formatted as currency even if the
 * app.yml didn't declare a `format` hint. Keeps existing dashboards legible
 * without requiring regeneration.
 */
const MONETARY_KEYWORDS: ReadonlySet<string> = new Set([
  "sales",
  "revenue",
  "price",
  "prices",
  "cost",
  "costs",
  "spend",
  "spends",
  "spending",
  "profit",
  "profits",
  "gmv",
  "arr",
  "mrr",
  "ltv",
  "aov",
  "arpu",
  "acv",
  "tcv",
  "cac",
  "payment",
  "payments",
  "payout",
  "payouts",
  "amount",
  "amounts",
  "fee",
  "fees",
  "balance",
  "balances",
  "income",
  "expense",
  "expenses",
  "usd",
  "eur",
  "gbp"
]);

/**
 * Words that say a number is something other than an amount of money, however
 * monetary the rest of its name: `payment_id` is a key, `discount_count` a
 * tally, `revenue_pct` a ratio and `price_rank` a position.
 */
const NON_MONETARY_WORDS: ReadonlySet<string> = new Set([
  // identifiers and codes
  "id",
  "ids",
  "key",
  "code",
  "number",
  "num",
  "no",
  // flags and enumerations
  "flag",
  "is",
  "has",
  "type",
  "status",
  // tallies and quantities
  "count",
  "counts",
  "cnt",
  "n",
  "qty",
  "quantity",
  "units",
  // ratios
  "pct",
  "percent",
  "percentage",
  "ratio",
  "rate",
  "share",
  "growth",
  // positions on a scale
  "rank",
  "index",
  "score",
  "tier",
  "level"
]);

/**
 * Nouns that, named after the money word, are what the number is instead:
 * `profit_margin` is a margin and `sales_orders` a count of orders. Money named
 * after one of them is still money (`margin_amount`, `orders_revenue`).
 *
 * A margin or a discount is usually a ratio (`profit_margin` 0.25 is 25%, not
 * $0.25), and the name is all there is to go by. Read as a ratio, a margin that
 * is in dollars only loses its `$`; read as money, every ratio is misstated.
 * So neither word says money on its own, and `gross_margin` is a plain number.
 */
const NOT_AN_AMOUNT: ReadonlySet<string> = new Set([
  "margin",
  "margins",
  "discount",
  "discounts",
  // things counted
  "orders",
  "transactions",
  "invoices",
  "customers",
  "users",
  "accounts",
  "visits",
  "sessions",
  "calls",
  "deals",
  "leads",
  "reps",
  "items",
  "products",
  "tickets"
]);

/**
 * Calendar parts. As the last word of a name one says what the number is
 * (`payment_year` is 2024), unless the word before it makes it the period the
 * money is for (`revenue_last_month`, `sales_per_day`).
 */
const CALENDAR_PARTS: ReadonlySet<string> = new Set([
  "year",
  "quarter",
  "month",
  "week",
  "day",
  "date",
  "hour"
]);
const PERIOD_QUALIFIERS: ReadonlySet<string> = new Set([
  "per",
  "by",
  "last",
  "this",
  "next",
  "prior",
  "previous",
  "prev",
  "current"
]);

/**
 * Whether a column holds numbers. A format is only inferred from the name of
 * one that does: `payment_date` is a date and `discount_code` is text, however
 * monetary their names.
 */
export const isNumericType = (type: DataType | undefined): boolean =>
  DataType.isInt(type) || DataType.isFloat(type) || DataType.isDecimal(type);

/**
 * The format a column's name and type imply when the app declares none: the
 * one rule for a table column and for a chart's value column. It is
 * `"currency"` for a numeric column whose name says money and does not say
 * something else, and `undefined` otherwise, leaving the plain number.
 *
 * It errs towards the plain number. A missing `$` costs nothing but looks; a
 * `$` on an id, a count or a percentage misstates the data.
 *
 *   `oxymart__total_weekly_sales` → `"currency"` (matches `sales`)
 *   `product_price`, `totalRevenue` → `"currency"` (camelCase is words too)
 *   `revenue_last_month`          → `"currency"` (the month is the period)
 *   `payment_id`, `revenue_pct`, `payment_year`, `paymentId`      → `undefined`
 *   `profit_margin`, `sales_orders` (a margin, a count of orders) → `undefined`
 *   `payment_date` (a date), `discount_code` (text)               → `undefined`
 *   `oxymart__store`, `holiday_flag`                              → `undefined`
 */
export function inferColumnFormat(
  columnName: string | undefined | null,
  type: DataType | undefined
): DisplayFormat | undefined {
  if (!columnName || !isNumericType(type)) return undefined;
  // Split into words so each is checked on its own: on any non-alphanumeric
  // separator (`oxymart__total_weekly_sales` is oxymart, total, weekly, sales),
  // and where a capital starts one (`avgOrderValueUSD` is avg, order, value, usd).
  const words = columnName
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/([A-Z])([A-Z][a-z])/g, "$1 $2")
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter(Boolean);
  let lastMonetary = -1;
  words.forEach((word, index) => {
    if (MONETARY_KEYWORDS.has(word)) lastMonetary = index;
  });
  if (lastMonetary < 0) return undefined;
  if (words.some((word) => NON_MONETARY_WORDS.has(word))) return undefined;
  if (words.slice(lastMonetary + 1).some((word) => NOT_AN_AMOUNT.has(word))) return undefined;
  const namesCalendarPart =
    CALENDAR_PARTS.has(words[words.length - 1]) &&
    !(words.length > 1 && PERIOD_QUALIFIERS.has(words[words.length - 2]));
  return namesCalendarPart ? undefined : "currency";
}

// Intl.NumberFormat instances are expensive to construct; cache one per
// (format, compact) pairing so repeat chart renders don't allocate.
const formatterCache = new Map<string, Intl.NumberFormat>();

function getFormatter(format: DisplayFormat, compact: boolean): Intl.NumberFormat {
  const key = `${format}:${compact}`;
  const cached = formatterCache.get(key);
  if (cached) return cached;

  let options: Intl.NumberFormatOptions;
  if (format === "currency") {
    options = compact
      ? { style: "currency", currency: "USD", notation: "compact", maximumFractionDigits: 1 }
      : { style: "currency", currency: "USD", maximumFractionDigits: 2 };
  } else if (format === "percent") {
    // Values are treated as already-scaled percentages (0–100), so divide
    // by 100 to hand Intl a ratio.
    options = { style: "percent", maximumFractionDigits: 2 };
  } else {
    options = compact
      ? { notation: "compact", maximumFractionDigits: 1 }
      : { maximumFractionDigits: 2 };
  }

  const formatter = new Intl.NumberFormat("en-US", options);
  formatterCache.set(key, formatter);
  return formatter;
}

/**
 * Text for one value out of an Arrow result. Arrow values (lists, structs,
 * decimals), Dates and arrays print through their own toString — their JSON
 * form is worse (a decimal comes out quoted) and throws on a BIGINT inside a
 * list. Only a bare object has no toString of its own; it would read
 * "[object Object]", so that one prints as JSON. null and undefined are empty.
 */
export const cellText = (value: unknown): string => {
  if (typeof value !== "object" || value === null || value.toString === Object.prototype.toString) {
    return toText(value);
  }
  // oxlint-disable-next-line typescript/no-base-to-string -- the check above rules out Object's default toString
  return String(value);
};

/**
 * Format a numeric value according to the requested `DisplayFormat`.
 *
 * - `currency` → `$301,397,792.46` (compact: `$301M`)
 * - `percent`  → `12.50%` (input is already a percentage, 0–100)
 * - `number`   → `1,234,567` (compact: `1.2M`)
 * - none       → the number itself, unrounded: `0.004`, not `0.00`
 *
 * Returns a passthrough string conversion when the value is not a finite
 * number, so callers can pipe every cell value through the same helper.
 *
 * A raw Arrow DECIMAL cell is only its unscaled integer, so pass the column's
 * `type` with it: without the scale it cannot be read as a number, and prints
 * as those unscaled digits.
 */
export function formatValue(
  value: unknown,
  format?: DisplayFormat,
  options: { compact?: boolean; type?: DataType } = {}
): string {
  if (value === null || value === undefined) return "";
  const compact = options.compact ?? false;

  const num =
    typeof value === "number"
      ? value
      : typeof value === "bigint"
        ? Number(value)
        : typeof value === "string" && value.trim() !== "" && Number.isFinite(Number(value))
          ? Number(value)
          : typeof value === "object" && options.type && DataType.isDecimal(options.type)
            ? parseFloat(decimalText(value, options.type.scale))
            : NaN;

  if (!Number.isFinite(num)) {
    return cellText(value);
  }

  if (!format) return String(num);

  const formatter = getFormatter(format, compact);
  return format === "percent" ? formatter.format(num / 100) : formatter.format(num);
}

type KeyPart = {
  key: string;
  index?: number;
};

const getKeyParts = (key: string) => {
  const parts: KeyPart[] = [];
  let currentPart = "";
  let currentIndex: number | undefined;
  for (let i = 0; i < key.length; i++) {
    const char = key[i];
    if (char === ".") {
      if (currentPart) {
        parts.push({ key: currentPart, index: currentIndex });
        currentPart = "";
        currentIndex = undefined;
      }
    } else if (char === "[") {
      if (currentPart) {
        parts.push({ key: currentPart });
        currentPart = "";
      }
      const endIndex = key.indexOf("]", i);
      if (endIndex !== -1) {
        currentIndex = parseInt(key.slice(i + 1, endIndex), 10);
        // eslint-disable-next-line sonarjs/updated-loop-counter
        i = endIndex;
      }
    } else {
      currentPart += char;
    }
  }
  if (currentPart) {
    parts.push({ key: currentPart, index: currentIndex });
  }
  return parts;
};

const getNextDataFromArray = (array: DataContainer, index?: number): DataContainer | null => {
  if (!Array.isArray(array) || index === undefined || index >= array.length) {
    return null;
  }
  const value = array[index];
  return value === null || value === undefined ? null : value;
};

const getNextDataFromObject = (obj: DataContainer, key: string): DataContainer | null => {
  if (typeof obj !== "object" || obj === null || !(key in obj)) {
    return null;
  }
  const value = (obj as Record<string, DataContainer>)[key];
  return value === null ? null : value;
};

const getNextData = (currentData: DataContainer, part: KeyPart): DataContainer | null => {
  if (currentData === null || currentData === undefined) {
    return null;
  }

  let nextData: DataContainer = currentData;

  if (Array.isArray(nextData)) {
    nextData = getNextDataFromArray(nextData, part.index);
    if (nextData === null) {
      return null;
    }
  }

  if (typeof nextData === "object" && nextData !== null) {
    nextData = getNextDataFromObject(nextData, part.key);
    if (nextData === null) {
      return null;
    }
  } else if (typeof nextData !== "object") {
    return null;
  }

  return nextData ?? null;
};

export const getData = (data: DataContainer, key: string) => {
  if (isFilePath(key)) {
    return {
      file_path: key
    };
  }
  const parts = getKeyParts(key);
  let currentData: DataContainer = data;
  for (const part of parts) {
    currentData = getNextData(currentData, part);
    if (currentData === null) {
      return null;
    }
  }
  return currentData;
};

const isFilePath = (key: string) => {
  return key.endsWith(".parquet") || key.endsWith(".csv") || key.endsWith(".json");
};

const registerAuthenticatedFile = async (
  filePath: string,
  projectId: string,
  branchName: string
): Promise<string> => {
  const db = await getDuckDB();
  const file_name = `${encodeBase64(filePath)}.parquet`;

  const pathb64 = encodeBase64(filePath);
  const response = await apiClient.get(`/${projectId}/apps/file/${pathb64}`, {
    responseType: "arraybuffer",
    params: { branch: branchName }
  });
  const fileData = new Uint8Array(response.data);

  await db.registerFileBuffer(file_name, fileData);

  return file_name;
};

/**
 * Register table data in DuckDB, preferring inline JSON over a network download.
 *
 * When the server embeds query results as `json` in the TableData (which it
 * always does for parameterized runs), we load that JSON directly into DuckDB's
 * virtual filesystem and expose it as a VIEW — zero extra HTTP round-trips.
 * When `json` is absent we fall back to the normal parquet download.
 *
 * Returns a name that can be used directly in `FROM "name"` queries.
 */
export const registerFromTableData = async (
  tableData: { file_path: string; json?: string | null },
  projectId: string,
  branchName: string
): Promise<string> => {
  const db = await getDuckDB();

  if (tableData.json) {
    // Stable name derived from the file path so the same data isn't re-registered
    const safeId = encodeBase64(tableData.file_path).replace(/[^a-zA-Z0-9]/g, "_");
    const jsonFileName = `${safeId}.json`;
    const viewName = safeId;

    await db.registerFileText(jsonFileName, tableData.json);
    const conn = await db.connect();
    try {
      await conn.query(
        `CREATE OR REPLACE VIEW "${viewName}" AS SELECT * FROM read_json_auto('${jsonFileName}')`
      );
    } finally {
      await conn.close();
    }
    return viewName;
  }

  return registerAuthenticatedFile(tableData.file_path, projectId, branchName);
};

/**
 * Download a project source file as Parquet from the server and register it in
 * DuckDB WASM under a `.parquet`-suffixed name.
 *
 * Returns { original: 'oxymart.csv', registered: 'oxymart.parquet' } so the
 * caller can rewrite SQL references before running in DuckDB WASM.
 *
 * Subsequent calls for the same path are no-ops and return the cached mapping.
 */
const registeredSourceFiles = new Map<string, string>();

export const registerSourceFile = async (
  filePath: string,
  projectId: string,
  branchName: string
): Promise<{ original: string; registered: string }> => {
  const safeId = encodeBase64(filePath).replace(/[^a-zA-Z0-9]/g, "_");
  const parquetName = `${safeId}.parquet`;

  const cacheKey = `${projectId}:${branchName}:${filePath}`;
  const cached = registeredSourceFiles.get(cacheKey);
  if (cached) {
    return { original: filePath, registered: cached };
  }

  const db = await getDuckDB();
  const pathb64 = encodeBase64(filePath);
  const response = await apiClient.get(`/${projectId}/apps/source/${pathb64}`, {
    responseType: "arraybuffer",
    params: { branch: branchName }
  });
  const data = new Uint8Array(response.data);
  // Server returns Parquet bytes — register under the .parquet name.
  await db.registerFileBuffer(parquetName, data);
  registeredSourceFiles.set(cacheKey, parquetName);
  return { original: filePath, registered: parquetName };
};

/**
 * Minimal Jinja-compatible renderer for SQL templates.
 * Minimal client-side Jinja renderer for app task SQL.
 *
 * Supported patterns (only these are handled — anything else is unsupported):
 *   {% if controls.x %}...{% endif %}     — conditional block (truthy check only)
 *   {{ controls.x | default('v') }}       — substitution with string fallback
 *   {{ controls.x }}                      — raw value substitution
 *
 * Single quotes inside substituted values are escaped to prevent SQL injection.
 *
 * If any Jinja tokens remain after rendering ({% ... %} or {{ ... }}), the
 * template uses unsupported syntax. This function throws in that case so the
 * caller can fall back to the server rather than silently producing wrong SQL.
 */
export function renderJinja(template: string, controls: Record<string, unknown>): string {
  let result = template;

  // {% if controls.x %}...{% endif %}
  result = result.replace(
    /\{%-?\s*if\s+controls\.(\w+)\s*-?%\}([\s\S]*?)\{%-?\s*endif\s*-?%\}/g,
    (_, name: string, body: string) => (controls[name] ? body : "")
  );

  // {{ controls.x | sqlquote }} — wraps value in single quotes with internal quotes escaped.
  //
  // Doubling the quote is the whole escape here, and only here. This SQL has
  // one destination — `runSqlInDuckDB` below, DuckDB WASM in the browser — and
  // DuckDB reads `''` as a quote and a backslash as an ordinary character. The
  // server's `sqlquote` cannot do the same: its SQL goes to the task's
  // `database`, and ClickHouse, MySQL, Snowflake, Redshift and BigQuery read a
  // backslash as an escape, so it escapes by that engine's rule. If this
  // renderer's output is ever sent anywhere but DuckDB, it needs that rule too.
  result = result.replace(
    /\{\{-?\s*controls\.(\w+)\s*\|\s*sqlquote\s*-?\}\}/g,
    (_, name: string) => `'${toText(controls[name]).replace(/'/g, "''")}'`
  );

  // {{ controls.x | default('fallback') }}
  result = result.replace(
    /\{\{-?\s*controls\.(\w+)\s*\|\s*default\(['"]([^'"]*)['"]\)\s*-?\}\}/g,
    (_, name: string, fallback: string) => toText(controls[name] ?? fallback).replace(/'/g, "''")
  );

  // {{ controls.x }}
  result = result.replace(/\{\{-?\s*controls\.(\w+)\s*-?\}\}/g, (_, name: string) =>
    toText(controls[name]).replace(/'/g, "''")
  );

  // Detect any remaining Jinja tokens — unsupported syntax.
  if (/\{[{%]/.test(result)) {
    throw new Error(
      "renderJinja: unsupported Jinja syntax detected after rendering. " +
        "Only {% if %}...{% endif %}, {{ x }}, {{ x | sqlquote }}, and {{ x | default('v') }} are supported client-side."
    );
  }

  return result;
}

/**
 * One cell of a task run in the browser, as the JSON that `read_json_auto`
 * reads back as the type the cell had: the result is registered from it.
 *
 * - A number is a JSON number, which is a double. A DECIMAL cell (and with it a
 *   HUGEINT, or the SUM of an integer column) is its unscaled 128-bit integer,
 *   read whole and at the column's scale.
 * - A date, a time and a timestamp are text, as the table shows them, which
 *   `read_json_auto` reads as a date or a timestamp again. As a number (epoch
 *   milliseconds) a date came back as a BIGINT. A timestamp with a time zone is
 *   its instant in UTC: the clock time in its zone does not say which instant.
 * - Any other cell held in a typed array (a BLOB's bytes, an INTERVAL's parts)
 *   has no JSON that reads back as what it was, so the run refuses it, naming
 *   the column. The app then runs its tasks on the server, which can.
 */
const jsonCell = (value: unknown, type: DataType, column: string): unknown => {
  if (value === null || value === undefined) return value;
  if (DataType.isDecimal(type)) return Number(decimalText(value, type.scale));
  if (DataType.isDate(type)) return formatDate(value as number);
  if (DataType.isTime(type)) return formatTime(value as number);
  if (DataType.isTimestamp(type)) {
    return type.timezone ? instantText(value as number) : fullTimestampText(value, type);
  }
  // Arrow Int64 / BigInt → Number (values within JS safe-integer range are exact)
  if (typeof value === "bigint") return Number(value);
  if (ArrayBuffer.isView(value)) {
    throw new Error(
      `Column "${column}" holds ${Type[type.typeId]} values, which a task run in the browser ` +
        "cannot pass on as they are"
    );
  }
  return value;
};

/**
 * Run a (Jinja-rendered) SQL query in DuckDB WASM and serialize the result
 * as a JSON string compatible with read_json_auto / registerFromTableData.
 */
export async function runSqlInDuckDB(sql: string): Promise<string> {
  const db = await getDuckDB();
  const conn = await db.connect();
  let result: Awaited<ReturnType<typeof conn.query>>;
  try {
    result = await conn.query(sql);
  } finally {
    await conn.close();
  }

  const rows = result.toArray().map((row) => {
    const obj: Record<string, unknown> = {};
    for (const field of result.schema.fields) {
      // duckdb-wasm types its result with its own, older Arrow release, whose
      // DataType this one's type guards do not accept: the same type, read as ours.
      const type = field.type as unknown as DataType;
      obj[field.name] = jsonCell((row as Record<string, unknown>)[field.name], type, field.name);
    }
    return obj;
  });

  return JSON.stringify(rows);
}

export const getArrowFieldType = (fieldName: string, schema: ArrowSchema): DataType | undefined => {
  return schema.fields.find((f) => f.name === fieldName)?.type;
};
