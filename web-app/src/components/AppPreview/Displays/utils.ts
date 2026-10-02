import { DataType, Struct, type Timestamp } from "apache-arrow";

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
  if (value instanceof Uint32Array) return formatNumber(value[0]);
  if (value instanceof Float32Array) return formatNumber(value[0]);
  if (value instanceof Float64Array) return formatNumber(value[0]);
  if (typeof value === "bigint") {
    return value.toString();
  }
  if (typeof value === "number") {
    return formatNumber(value);
  }
  return value;
};

export const getArrowColumnValues = (table: ArrowTable, columnName: string) => {
  const fieldType = getArrowFieldType(columnName, table.schema);
  return table.toArray().map((row: unknown) => {
    const value = (row as Record<string, unknown>)[columnName];
    if (!fieldType) {
      return getArrowValue(value);
    }
    return getArrowValueWithType(value, fieldType);
  });
};

export const getArrowValueWithType = (
  value: unknown,
  type: DataType
): number | string | unknown => {
  // A NULL cell stays null whatever its column type, for the caller to show as
  // it shows any other NULL. The readers below expect a value: a NULL decimal
  // throws in them and a NULL date reads "Invalid Date".
  if (value === null || value === undefined) return value;
  if (DataType.isDate(type)) {
    return formatDate(value as number);
  }
  if (DataType.isTimestamp(type)) {
    return formatDateTime(value as number, (type as Timestamp)?.timezone);
  }
  if (DataType.isTime(type)) {
    return formatTime(value as number);
  }
  // in the BE we are using snowflake-rs library which doesn't return field metadata
  // so there is no way to know if a field is snowflake timestamp or not
  // except checking the structure of the value itself
  if (isSnowflakeTimestamp(value, type)) {
    return formatSnowflakeTimestamp(value as { epoch: number; fraction: number });
  }
  if (DataType.isDecimal(type)) {
    return formatNumber(parseFloat(decimalText(value, type.scale)));
  }
  return getArrowValue(value);
};

/**
 * The exact decimal an Arrow DECIMAL cell stands for, e.g. "1234.56". The cell
 * holds only the unscaled integer (123456); the scale is on the column's type.
 */
const decimalText = (value: unknown, scale: number): string => {
  // BigNum.valueOf() / Number(bigNum) throws "is not safe to convert to a number"
  // when the internal 128-bit integer exceeds Number.MAX_SAFE_INTEGER.
  // Call .toString() directly (which invokes bigNumToString, not bigNumToNumber)
  // to get the raw integer digits, then manually insert the decimal point.
  const rawStr = (value as { toString(): string }).toString();
  if (!scale) return rawStr;
  const isNeg = rawStr.startsWith("-");
  const digits = isNeg ? rawStr.slice(1) : rawStr;
  const padded = digits.padStart(scale + 1, "0");
  return `${isNeg ? "-" : ""}${padded.slice(0, -scale)}.${padded.slice(-scale)}`;
};

// A timestamp in an export keeps its seconds, and its milliseconds when it has any.
const EXPORT_TIMESTAMP_FORMAT = "YYYY-MM-DD HH:mm:ss.SSS";
const withoutZeroMillis = (timestamp: string) => timestamp.replace(/\.000$/, "");

/**
 * Text for one Arrow cell in an export. It reads the cell the way the table
 * does (a decimal is scaled, a date or timestamp is a date, not its epoch) but
 * without the table's display rounding: a decimal keeps every digit, a float
 * its full precision and a timestamp its seconds.
 */
export const getArrowExportText = (value: unknown, type?: DataType): string => {
  if (value === null || value === undefined || !type) return cellText(value);
  if (DataType.isDecimal(type)) return decimalText(value, type.scale);
  if (DataType.isDate(type)) return formatDate(value as number);
  if (DataType.isTimestamp(type)) {
    return withoutZeroMillis(
      formatDateTime(value as number, type.timezone, EXPORT_TIMESTAMP_FORMAT)
    );
  }
  if (DataType.isTime(type)) return formatTime(value as number);
  if (isSnowflakeTimestamp(value, type)) {
    return withoutZeroMillis(
      formatSnowflakeTimestamp(
        value as { epoch: number; fraction: number },
        EXPORT_TIMESTAMP_FORMAT
      )
    );
  }
  return cellText(value);
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

function formatSnowflakeTimestamp(
  value: {
    epoch: number | bigint;
    fraction: number | bigint;
  },
  format = "YYYY-MM-DD HH:mm"
): string {
  const epoch = typeof value.epoch === "bigint" ? Number(value.epoch) : value.epoch;
  const fraction = typeof value.fraction === "bigint" ? Number(value.fraction) : value.fraction;
  const milliseconds = epoch * 1000 + Math.floor(fraction / 1_000_000);
  return dayjs.utc(milliseconds).format(format);
}

function formatDate(value: number | string): string {
  return dayjs.utc(value).format("YYYY-MM-DD");
}

function formatDateTime(
  value: number | string,
  tz?: string | null,
  format = "YYYY-MM-DD HH:mm"
): string {
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

// NOTE: This function is on the data-extraction path — `getArrowValue` calls
// it on every cell value pulled out of an Arrow table, including the series
// data fed into ECharts. Its return value is consumed by ECharts on a
// `type: "value"` axis, which coerces numeric *strings* back to numbers only
// when they contain pure digits. Adding locale-aware thousands separators
// here breaks that coercion (`"43,149,473.45"` is no longer parseable) and
// blanks the y-axis and line. Human-friendly formatting with commas /
// dollar signs lives at the render layer (`formatValue`, chart tooltip
// formatter, table cells).
function formatNumber(num: number) {
  return num % 1 === 0 ? num.toString() : num.toFixed(2);
}

/**
 * Monetary column-name detection. When a column name contains any of these
 * word parts (split on non-alphanumerics), the value is formatted as
 * currency even if the app.yml didn't declare a `format` hint. Keeps
 * existing dashboards legible without requiring regeneration.
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
  "margin",
  "margins",
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
  "discount",
  "discounts",
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
 * Infer a `DisplayFormat` from a column name. Returns `"currency"` when the
 * column name contains a word that strongly suggests a monetary measure,
 * and `undefined` otherwise so the caller can fall back to the default
 * numeric formatter.
 *
 * Examples:
 *   `oxymart__total_weekly_sales` → `"currency"` (matches `sales`)
 *   `oxymart__store`              → `undefined`
 *   `holiday_flag`                → `undefined`
 *   `product_price`               → `"currency"` (matches `price`)
 */
export function inferCurrencyFormat(
  columnName: string | undefined | null
): DisplayFormat | undefined {
  if (!columnName) return undefined;
  // Split on any non-alphanumeric separator so `oxymart__total_weekly_sales`
  // becomes `["oxymart", "total", "weekly", "sales"]` and we can check each
  // part against the keyword set individually.
  const parts = columnName
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter(Boolean);
  return parts.some((part) => MONETARY_KEYWORDS.has(part)) ? "currency" : undefined;
}

/**
 * Whether a column holds numbers. A format is only inferred from the name of
 * one that does: `payment_date` is a date and `discount_code` is text, however
 * monetary their names.
 */
export const isNumericType = (type: DataType | undefined): boolean =>
  DataType.isInt(type) || DataType.isFloat(type) || DataType.isDecimal(type);

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
 *
 * Returns a passthrough string conversion when the value is not a finite
 * number or when `format` is undefined, so callers can pipe every cell value
 * through the same helper.
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

  if (!format) {
    return formatNumber(num);
  }

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

  // {{ controls.x | sqlquote }} — wraps value in single quotes with internal quotes escaped
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

  // Convert Arrow Table to plain JSON array, ensuring all numeric Arrow types
  // become plain JS numbers so JSON.stringify produces numeric literals and
  // read_json_auto infers the correct column type (not VARCHAR).
  const rows = result.toArray().map((row) => {
    const obj: Record<string, unknown> = {};
    for (const field of result.schema.fields) {
      const val = (row as Record<string, unknown>)[field.name];
      if (typeof val === "bigint") {
        // Arrow Int64 / BigInt → Number (values within JS safe-integer range are exact)
        obj[field.name] = Number(val);
      } else if (ArrayBuffer.isView(val) && !(val instanceof DataView)) {
        // TypedArray — DuckDB WASM represents HUGEINT as Uint32Array(4) in Arrow.
        // Extract the scalar from index 0 (lowest 32-bit word, sufficient for typical
        // COUNT/SUM results < 2^32; larger values accept the same precision loss that
        // the existing chart-display path already accepts).
        obj[field.name] = Number((val as unknown as ArrayLike<number | bigint>)[0]);
      } else {
        obj[field.name] = val;
      }
    }
    return obj;
  });

  return JSON.stringify(rows);
}

export const getArrowFieldType = (fieldName: string, schema: ArrowSchema): DataType | undefined => {
  return schema.fields.find((f) => f.name === fieldName)?.type;
};
