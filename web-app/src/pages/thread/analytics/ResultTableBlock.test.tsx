// @vitest-environment jsdom

import { describe, expect, it, vi } from "vitest";

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "p1" }, branchName: "main" })
}));

import { extractResultTables } from "@/hooks/useAnalyticsRun";
import { cellText } from "./ResultTableBlock";

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
