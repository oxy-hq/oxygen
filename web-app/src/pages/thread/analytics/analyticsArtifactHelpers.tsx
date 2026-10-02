import { memo } from "react";
import { DisplayBlock } from "@/components/AppPreview/Displays";
import type { ArtifactItem, SqlItem } from "@/hooks/analyticsSteps";
import type { AnalyticsDisplayBlock } from "@/hooks/useAnalyticsRun";
import { toText } from "@/libs/utils/string";
import type { DataContainer, Display } from "@/types/app";
import type { SqlArtifact } from "@/types/artifact";

// ── JSON helpers ──────────────────────────────────────────────────────────────

/**
 * Parse a tool input/output string that may be double-encoded.
 * analyticsSteps.ts stores toolInput/toolOutput as JSON.stringify(payload),
 * so the value is often a JSON-encoded string that needs a second parse.
 */
export function parseToolJson<T = unknown>(raw: string | undefined): T | null {
  if (!raw) return null;
  try {
    let v = JSON.parse(raw);
    if (typeof v === "string") v = JSON.parse(v);
    return v as T;
  } catch {
    return null;
  }
}

// ── Chart display helpers ─────────────────────────────────────────────────────

const AGENTIC_DATA_KEY = "__agentic_result__";

export function toDisplayProps(
  block: AnalyticsDisplayBlock,
  index: number,
  runId: string
): {
  display: Display;
  data: DataContainer;
} {
  const { config, columns, rows } = block;
  const json = JSON.stringify(
    rows.map((row) => Object.fromEntries(columns.map((col, i) => [col, row[i]])))
  );
  const dataKey = `${AGENTIC_DATA_KEY}_${runId}_${index}`;
  const data: DataContainer = {
    [dataKey]: { file_path: dataKey, json }
  };

  const ct = config.chart_type;
  let display: Display;
  if (ct === "line_chart") {
    display = {
      type: "line_chart",
      x: config.x ?? columns[0] ?? "",
      y: config.y ?? columns[1] ?? "",
      data: dataKey,
      series: config.series,
      title: config.title,
      xAxisTitle: config.x_axis_label,
      yAxisTitle: config.y_axis_label
    };
  } else if (ct === "bar_chart") {
    display = {
      type: "bar_chart",
      x: config.x ?? columns[0] ?? "",
      y: config.y ?? columns[1] ?? "",
      data: dataKey,
      series: config.series,
      title: config.title
    };
  } else if (ct === "pie_chart") {
    display = {
      type: "pie_chart",
      name: config.name ?? columns[0] ?? "",
      value: config.value ?? columns[1] ?? "",
      data: dataKey,
      title: config.title
    };
  } else {
    display = { type: "table", data: dataKey, title: config.title };
  }

  return { display, data };
}

/** Stable wrapper so parent re-renders don't recreate display/data objects. */
export const AnalyticsDisplayBlockItem = memo(
  ({ block, index, runId }: { block: AnalyticsDisplayBlock; index: number; runId: string }) => {
    const { display, data } = toDisplayProps(block, index, runId);
    return <DisplayBlock display={display} data={data} />;
  }
);

// ── SQL artifact helpers ──────────────────────────────────────────────────────

export function sqlArtifactFromSqlItem(item: SqlItem): SqlArtifact {
  return {
    id: item.id,
    name: "SQL Query",
    kind: "execute_sql",
    content: {
      type: "execute_sql",
      value: {
        database: item.database ?? "",
        sql_query: item.sql,
        result: item.result,
        is_result_truncated: false,
        error: item.error
      }
    }
  };
}

export function sqlArtifactFromPreviewData(item: ArtifactItem): SqlArtifact | null {
  const input = parseToolJson<Record<string, unknown>>(item.toolInput);
  const table = input?.table;
  if (!table || typeof table !== "string") return null;
  const limit = typeof input?.limit === "number" ? input.limit : 5;
  const sql = `SELECT * FROM "${table}" LIMIT ${limit}`;

  let result: string[][] | undefined;
  const output = parseToolJson<Record<string, unknown>>(item.toolOutput ?? "");
  const cols = output?.columns as string[] | undefined;
  const rows = output?.rows as unknown[] | undefined;
  if (cols?.length) {
    result = [
      cols,
      ...(rows ?? []).map((row) =>
        Array.isArray(row)
          ? row.map((v) => toText(v))
          : cols.map((col) => toText((row as Record<string, unknown>)?.[col]))
      )
    ];
  }

  return {
    id: item.id,
    name: "preview_data",
    kind: "execute_sql",
    content: {
      type: "execute_sql",
      value: { database: "", sql_query: sql, result, is_result_truncated: false }
    }
  };
}

export function sqlArtifactFromExecutePreview(item: ArtifactItem): SqlArtifact | null {
  const input = parseToolJson<Record<string, unknown>>(item.toolInput);
  const sql = input?.sql;
  if (!sql || typeof sql !== "string") return null;

  let result: string[][] | undefined;
  const output = parseToolJson<Record<string, unknown>>(item.toolOutput ?? "");
  const cols = output?.columns as string[] | undefined;
  const rows = output?.rows as unknown[][] | undefined;
  if (cols?.length) {
    result = [
      cols,
      ...(rows ?? []).map((row) =>
        Array.isArray(row)
          ? row.map((v) => toText(v))
          : cols.map((col) => toText((row as Record<string, unknown>)?.[col]))
      )
    ];
  }

  return {
    id: item.id,
    name: "execute_preview",
    kind: "execute_sql",
    content: {
      type: "execute_sql",
      value: { database: "", sql_query: sql, result, is_result_truncated: false }
    }
  };
}

function rowsToTable(
  columns: string[] | undefined,
  rows: unknown[] | undefined
): string[][] | undefined {
  if (!columns?.length) return undefined;

  return [
    columns,
    ...(rows ?? []).map((row) =>
      Array.isArray(row)
        ? row.map((value) => toText(value))
        : columns.map((column) => toText((row as Record<string, unknown>)?.[column]))
    )
  ];
}

export function sqlArtifactFromExecuteSql(item: ArtifactItem): SqlArtifact | null {
  const input = parseToolJson<Record<string, unknown>>(item.toolInput);
  const output = parseToolJson<Record<string, unknown>>(item.toolOutput ?? "");
  const sql = input?.sql;

  if (!sql || typeof sql !== "string") return null;

  const columns = output?.columns as string[] | undefined;
  const rows = output?.rows as unknown[] | undefined;
  const database = typeof output?.database === "string" ? output.database : "";

  let error: string | undefined;
  if (output && output.ok === false) {
    error = typeof output.error === "string" ? output.error : undefined;
  } else if (!output && item.toolOutput) {
    // Plain-text error (non-JSON tool output).
    try {
      const raw = JSON.parse(item.toolOutput);
      if (typeof raw === "string") error = raw;
    } catch {
      // ignore
    }
  }

  return {
    id: item.id,
    name: "execute_sql",
    kind: "execute_sql",
    content: {
      type: "execute_sql",
      value: {
        database,
        sql_query: sql,
        result: rowsToTable(columns, rows),
        is_result_truncated: false,
        error
      }
    }
  };
}

export function sqlArtifactFromSemanticQuery(item: ArtifactItem): SqlArtifact | null {
  const output = parseToolJson<Record<string, unknown>>(item.toolOutput ?? "");
  const sql = output?.sql_generated;

  if (!sql || typeof sql !== "string") return null;

  const columns = output?.columns as string[] | undefined;
  const rows = output?.rows as unknown[] | undefined;
  const database = typeof output?.database === "string" ? output.database : "";

  return {
    id: item.id,
    name: "semantic_query",
    kind: "execute_sql",
    content: {
      type: "execute_sql",
      value: {
        database,
        sql_query: sql,
        result: rowsToTable(columns, rows),
        is_result_truncated: false
      }
    }
  };
}
