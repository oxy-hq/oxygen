// @vitest-environment jsdom

import { describe, expect, it, vi } from "vitest";

vi.mock("@lottiefiles/react-lottie-player", () => ({ Player: "div" }));

import type { ArtifactItem } from "@/hooks/analyticsSteps";
import type { AnalyticsDisplayBlock } from "@/hooks/useAnalyticsRun";
import type { DataContainer, Display, TableData } from "@/types/app";
import {
  parseToolJson,
  sqlArtifactFromExecutePreview,
  toDisplayProps
} from "./analyticsArtifactHelpers";

// ── parseToolJson ───────────────────────────────────────────────────────────

describe("parseToolJson", () => {
  it("parses a double-encoded JSON string", () => {
    const raw = JSON.stringify(JSON.stringify({ a: 1 }));
    expect(parseToolJson(raw)).toEqual({ a: 1 });
  });

  it("returns null for undefined input", () => {
    expect(parseToolJson(undefined)).toBeNull();
  });
});

// ── sqlArtifactFromExecutePreview ───────────────────────────────────────────

function makeArtifact(
  input: Record<string, unknown>,
  output: Record<string, unknown> | undefined
): ArtifactItem {
  return {
    kind: "artifact",
    id: "test-id",
    toolName: "execute_preview",
    toolInput: JSON.stringify(JSON.stringify(input)),
    toolOutput: output ? JSON.stringify(JSON.stringify(output)) : undefined,
    isStreaming: false
  };
}

describe("sqlArtifactFromExecutePreview", () => {
  it("handles rows that are arrays", () => {
    const item = makeArtifact(
      { sql: "SELECT 1" },
      {
        columns: ["a", "b"],
        rows: [
          ["1", "2"],
          ["3", "4"]
        ]
      }
    );
    const result = sqlArtifactFromExecutePreview(item);
    expect(result?.content.value.result).toEqual([
      ["a", "b"],
      ["1", "2"],
      ["3", "4"]
    ]);
  });

  it("handles rows that are objects (not arrays)", () => {
    const item = makeArtifact(
      { sql: "SELECT 1" },
      {
        columns: ["a", "b"],
        rows: [
          { a: "1", b: "2" },
          { a: "3", b: "4" }
        ]
      }
    );
    const result = sqlArtifactFromExecutePreview(item);
    expect(result?.content.value.result).toEqual([
      ["a", "b"],
      ["1", "2"],
      ["3", "4"]
    ]);
  });

  it("handles rows with null/undefined values", () => {
    const item = makeArtifact(
      { sql: "SELECT 1" },
      { columns: ["a", "b"], rows: [[null, undefined]] }
    );
    const result = sqlArtifactFromExecutePreview(item);
    expect(result?.content.value.result).toEqual([
      ["a", "b"],
      ["", ""]
    ]);
  });

  it("returns null when sql is missing", () => {
    const item = makeArtifact({}, { columns: ["a"], rows: [] });
    expect(sqlArtifactFromExecutePreview(item)).toBeNull();
  });
});

// ── toDisplayProps — unique data keys per block ─────────────────────────────

describe("toDisplayProps", () => {
  const makeBlock = (
    chartType: AnalyticsDisplayBlock["config"]["chart_type"],
    columns: string[],
    rows: unknown[][],
    title?: string
  ): AnalyticsDisplayBlock => ({
    config: { chart_type: chartType, title },
    columns,
    rows
  });

  // `Display` and `DataContainer` are the app-wide unions: a display need not read
  // any data, and a container may be a scalar, a list or one table. A chart built by
  // `toDisplayProps` is narrower than that, and these say so with a runtime check.

  /** The key a chart or table display reads its rows from. */
  const dataKeyOf = (display: Display): string => {
    if (!("data" in display)) throw new Error(`a ${display.type} display reads no data`);
    return display.data;
  };

  const isTableData = (value: DataContainer): value is TableData =>
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    typeof value.file_path === "string";

  /** The container as the keyed map of tables it is meant to be. */
  const asDataMap = (data: DataContainer): Record<string, DataContainer> => {
    if (typeof data !== "object" || data === null || Array.isArray(data) || isTableData(data)) {
      throw new Error("expected a map of data keys to tables");
    }
    return data;
  };

  const tableAt = (data: DataContainer, key: string): TableData => {
    const entry = asDataMap(data)[key];
    if (!isTableData(entry)) throw new Error(`no table registered under ${key}`);
    return entry;
  };

  it("uses unique data keys for different block indices within the same run", () => {
    const block0 = makeBlock("line_chart", ["week", "value"], [["2024-01", 10]]);
    const block1 = makeBlock("bar_chart", ["month", "count"], [["Jan", 5]]);

    const props0 = toDisplayProps(block0, 0, "run-A");
    const props1 = toDisplayProps(block1, 1, "run-A");

    const dataKey0 = dataKeyOf(props0.display);
    const dataKey1 = dataKeyOf(props1.display);
    expect(dataKey0).not.toBe(dataKey1);

    expect(Object.keys(asDataMap(props0.data))).toEqual([dataKey0]);
    expect(Object.keys(asDataMap(props1.data))).toEqual([dataKey1]);
    expect(tableAt(props0.data, dataKey0).file_path).not.toBe(
      tableAt(props1.data, dataKey1).file_path
    );
  });

  it("uses unique data keys for the same block index across different runs", () => {
    const block = makeBlock("line_chart", ["week", "value"], [["2024-01", 10]]);

    const propsRunA = toDisplayProps(block, 0, "run-A");
    const propsRunB = toDisplayProps(block, 0, "run-B");

    const dataKeyA = dataKeyOf(propsRunA.display);
    const dataKeyB = dataKeyOf(propsRunB.display);
    expect(dataKeyA).not.toBe(dataKeyB);
    expect(tableAt(propsRunA.data, dataKeyA).file_path).not.toBe(
      tableAt(propsRunB.data, dataKeyB).file_path
    );
  });

  it("embeds correct JSON in each block's data", () => {
    const block = makeBlock(
      "line_chart",
      ["x", "y"],
      [
        ["a", 1],
        ["b", 2]
      ],
      "My Chart"
    );
    const { data, display } = toDisplayProps(block, 0, "run-1");
    // A table with no inline JSON parses to null here and fails the comparison.
    const json: unknown = JSON.parse(tableAt(data, dataKeyOf(display)).json ?? "null");
    expect(json).toEqual([
      { x: "a", y: 1 },
      { x: "b", y: 2 }
    ]);
  });
});
