import { describe, expect, it, vi } from "vitest";
import {
  getPieChartData,
  getSeriesData,
  getSeriesValues,
  getSimpleAggregatedData,
  getXAxisData
} from "./chartQueries";

// ── helpers ───────────────────────────────────────────────────────────────────

function makeConnection(queryResult: object = {}, prepareResult: object = {}) {
  const stmtQuery = vi.fn().mockResolvedValue(prepareResult);
  const prepare = vi.fn().mockResolvedValue({ query: stmtQuery });
  const query = vi.fn().mockResolvedValue(queryResult);
  return { connection: { query, prepare }, query, prepare, stmtQuery };
}

// Override the column readers to just return what we hand back from the mock.
const columnValues = vi.hoisted(() => (_result: unknown, col: string) => {
  if (col === "x") return ["Mon", "Tue"];
  if (col === "y") return [100, 200];
  if (col === "series") return ["A", "B"];
  if (col === "name") return ["Cat A", "Cat B"];
  if (col === "value") return [10, 20];
  return [];
});
vi.mock("../utils", () => ({
  getArrowColumnValues: columnValues,
  getArrowColumnKeys: columnValues
}));

// ── regression: column names containing whitespace ────────────────────────────

describe("chartQueries – column names with spaces are quoted", () => {
  const fileName = "test_file";
  const fieldWithSpace = "Total Calories (kcal)";

  it("getXAxisData quotes xField that contains spaces", async () => {
    const { connection, query } = makeConnection();
    await getXAxisData(connection as never, fileName, fieldWithSpace);
    const sql: string = query.mock.calls[0][0];
    expect(sql).toContain(`"${fieldWithSpace}"`);
    expect(sql).not.toMatch(new RegExp(`(?<!")${fieldWithSpace.replace(/[()]/g, "\\$&")}(?!")`));
  });

  it("getSeriesData quotes seriesField that contains spaces", async () => {
    const { connection, prepare } = makeConnection();
    await getSeriesData(connection as never, fileName, fieldWithSpace);
    const sql: string = prepare.mock.calls[0][0];
    expect(sql).toContain(`"${fieldWithSpace}"`);
  });

  it("getSeriesValues quotes xField and yField that contain spaces", async () => {
    const xWithSpace = "Week Day";
    const yWithSpace = "Total Calories (kcal)";
    const seriesWithSpace = "Meal Type";
    const { connection, prepare } = makeConnection();
    await getSeriesValues(
      connection as never,
      fileName,
      xWithSpace,
      yWithSpace,
      seriesWithSpace,
      "Lunch"
    );
    const sql: string = prepare.mock.calls[0][0];
    expect(sql).toContain(`"${xWithSpace}"`);
    expect(sql).toContain(`"${yWithSpace}"`);
    expect(sql).toContain(`"${seriesWithSpace}"`);
  });

  it("getSimpleAggregatedData quotes xField and yField that contain spaces", async () => {
    const xWithSpace = "Week Day";
    const yWithSpace = "Total Calories (kcal)";
    const { connection, query } = makeConnection();
    await getSimpleAggregatedData(connection as never, fileName, xWithSpace, yWithSpace);
    const sql: string = query.mock.calls[0][0];
    expect(sql).toContain(`"${xWithSpace}"`);
    expect(sql).toContain(`"${yWithSpace}"`);
  });

  it("getPieChartData quotes nameField and valueField that contain spaces", async () => {
    const nameWithSpace = "Category Name";
    const valueWithSpace = "Total Calories (kcal)";
    const { connection, query } = makeConnection();
    await getPieChartData(connection as never, fileName, nameWithSpace, valueWithSpace);
    const sql: string = query.mock.calls[0][0];
    expect(sql).toContain(`"${nameWithSpace}"`);
    expect(sql).toContain(`"${valueWithSpace}"`);
  });

  it("still works for field names without spaces", async () => {
    const { connection, query } = makeConnection();
    await getXAxisData(connection as never, fileName, "week");
    const sql: string = query.mock.calls[0][0];
    // Quoted identifiers are valid SQL regardless of spaces — no regression.
    expect(sql).toContain('"week"');
  });
});
