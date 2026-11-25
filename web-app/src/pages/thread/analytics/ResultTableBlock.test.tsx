// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/Code/CodeView", () => ({ default: () => null }));

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "p1" }, branchName: "main" })
}));

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { extractResultTables } from "@/hooks/useAnalyticsRun";
import { AnalyticsService } from "@/services/api/analytics";
import { previewTable, ReactQueryPanel, resultIdOf } from "./ReactQueryPanel";
import { cellText, PAGE_ROWS, ResultTableBlock } from "./ResultTableBlock";

afterEach(cleanup);

describe("cellText", () => {
  it("keeps a number's sign and digits", () => {
    expect(cellText(-1234.5, "number")).toBe((-1234.5).toLocaleString());
    expect(cellText(0.000123, "number")).toBe(
      (0.000123).toLocaleString(undefined, { maximumFractionDigits: 20 })
    );
    expect(cellText(-0.5, "number").startsWith("-")).toBe(true);
  });

  it("shows a null as a dash and text as written", () => {
    expect(cellText(null, "number")).toBe("—");
    expect(cellText(undefined, "text")).toBe("—");
    expect(cellText("-12", "text")).toBe("-12");
  });
});

describe("extractResultTables", () => {
  it("reads result_table rows in order and ignores the rest", () => {
    const table = {
      result_id: "r2",
      columns: ["n"],
      column_kinds: ["number"],
      rows: [[-1]],
      row_count: 1,
      truncated: false
    };
    const events = [
      { id: "1", type: "answer_result", data: { result_id: "r2" } },
      { id: "2", type: "result_table", data: table },
      { id: "3", type: "done", data: {} }
    ];
    expect(extractResultTables(events as never)).toEqual([table]);
  });
});

describe("a react query's preview in the table", () => {
  const item = {
    kind: "sql" as const,
    id: "s1",
    sql: "select region, revenue from sales",
    isStreaming: false,
    rowCount: 30,
    result: [
      ["region", "revenue"],
      ["east", -12.5],
      ["west", null]
    ] as unknown as string[][]
  };

  it("keeps the full row count and types columns from the preview", () => {
    expect(previewTable(item, "r2")).toEqual({
      result_id: "r2",
      columns: ["region", "revenue"],
      column_kinds: ["text", "number"],
      rows: [
        ["east", -12.5],
        ["west", null]
      ],
      row_count: 30,
      truncated: false
    });
  });

  it("fetches page 1 when the row holds less than a full page", async () => {
    const getResultPage = vi.spyOn(AnalyticsService, "getResultPage").mockResolvedValue({
      columns: ["region", "revenue"],
      column_kinds: ["text", "number"],
      rows: [["north", 7]],
      offset: 0,
      row_count: 30,
      truncated: false,
      created_at: "2026-11-25T00:00:00Z"
    });
    render(<ResultTableBlock runId='run-1' table={previewTable(item, "r2")} />);
    await waitFor(() => expect(screen.getByText("north")).toBeTruthy());
    expect(getResultPage).toHaveBeenCalledWith("p1", "run-1", "r2", {
      offset: 0,
      limit: PAGE_ROWS,
      sort: undefined,
      desc: undefined
    });
    getResultPage.mockRestore();
  });

  it("keeps the preview when the stored result is gone", async () => {
    const getResultPage = vi
      .spyOn(AnalyticsService, "getResultPage")
      .mockRejectedValue({ response: { status: 404 } });
    render(<ResultTableBlock runId='run-1' table={previewTable(item, "r2")} />);
    await waitFor(() => expect(screen.getByText(/only its first rows are shown/)).toBeTruthy());
    expect(screen.getByText("east")).toBeTruthy();
    getResultPage.mockRestore();
  });
});

describe("resultIdOf", () => {
  it("reads a react query's result id only from an object output", () => {
    expect(resultIdOf(JSON.stringify({ result_id: "r3", sql: "select 1" }))).toBe("r3");
    expect(resultIdOf(JSON.stringify("first"))).toBeUndefined();
    expect(resultIdOf(JSON.stringify({ result_id: 3 }))).toBeUndefined();
    expect(resultIdOf("not json")).toBeUndefined();
    expect(resultIdOf(undefined)).toBeUndefined();
  });
});

describe("ReactQueryPanel", () => {
  const page = {
    columns: ["region", "revenue"],
    column_kinds: ["text", "number"] as ("text" | "number")[],
    rows: [["north", 7]],
    offset: 0,
    row_count: 30,
    truncated: false,
    created_at: "2026-11-25T00:00:00Z"
  };
  const query = () => ({
    kind: "sql" as const,
    id: "s1",
    sql: "select region, revenue from sales",
    isStreaming: false,
    rowCount: 30,
    result: [
      ["region", "revenue"],
      ["east", "1"]
    ]
  });

  it("fetches a page once while the run streams new items", async () => {
    const get = vi.spyOn(AnalyticsService, "getResultPage").mockResolvedValue(page);
    const { rerender } = render(
      <ReactQueryPanel item={query()} resultId='r2' runId='run-1' onClose={() => {}} />
    );
    await waitFor(() => expect(screen.getByText("north")).toBeTruthy());
    for (let i = 0; i < 3; i++) {
      rerender(<ReactQueryPanel item={query()} resultId='r2' runId='run-1' onClose={() => {}} />);
    }
    await waitFor(() => expect(screen.getByText("north")).toBeTruthy());
    expect(get).toHaveBeenCalledTimes(1);
    get.mockRestore();
  });

  it("opens another query at its first page, unsorted", async () => {
    const get = vi.spyOn(AnalyticsService, "getResultPage").mockResolvedValue(page);
    const { rerender } = render(
      <ReactQueryPanel item={query()} resultId='r2' runId='run-1' onClose={() => {}} />
    );
    await waitFor(() => expect(screen.getByText("north")).toBeTruthy());
    fireEvent.click(screen.getByText("revenue"));
    await waitFor(() =>
      expect(get).toHaveBeenLastCalledWith(
        "p1",
        "run-1",
        "r2",
        expect.objectContaining({ sort: 1 })
      )
    );
    rerender(<ReactQueryPanel item={query()} resultId='r3' runId='run-1' onClose={() => {}} />);
    await waitFor(() =>
      expect(get).toHaveBeenLastCalledWith("p1", "run-1", "r3", {
        offset: 0,
        limit: PAGE_ROWS,
        sort: undefined,
        desc: undefined
      })
    );
    get.mockRestore();
  });
});
