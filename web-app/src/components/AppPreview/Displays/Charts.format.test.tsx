// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { makeVector, Table, type Vector, vectorFromArray } from "apache-arrow";
import type { EChartsOption } from "echarts";
import type { ReactElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DisplayFormat } from "@/types/app";

vi.mock("@/libs/duckdb", () => ({ getDuckDB: vi.fn() }));
vi.mock("@/services/api/axios", () => ({ apiClient: {} }));
vi.mock("@/components/Echarts", () => ({ Echarts: () => null }));
vi.mock("@/components/Echarts/resolveColor", () => ({
  resolveColor: () => "#000",
  resolveColorWithAlpha: () => "#000"
}));

// The chart hands `useChartBase` the function that builds its options from a
// DuckDB connection. The test takes that function and runs it on its own data.
type Build = (params: {
  display: unknown;
  connection: unknown;
  fileName: string;
  isDarkMode: boolean;
}) => Promise<EChartsOption>;
const captured = vi.hoisted(() => ({ build: null as Build | null, display: null as unknown }));
vi.mock("./hooks", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./hooks")>()),
  useChartBase: ({
    display,
    buildChartOptions
  }: {
    display: unknown;
    buildChartOptions: Build;
  }) => {
    captured.build = buildChartOptions;
    captured.display = display;
    return { isLoading: false, chartOptions: {}, isDarkMode: false };
  }
}));

import { BarChart } from "./BarChart";
import { LineChart } from "./LineChart";
import { PieChart } from "./PieChart";

/**
 * A source table with one category column and one value column of the given
 * type. The schema query sees the value column as it is stored; the chart's own
 * queries see it summed under the alias they select.
 */
const connectionFor = (valueColumn: string, values: Vector) => ({
  query: (sql: string) => {
    if (sql.includes("LIMIT 0")) return Promise.resolve(new Table({ [valueColumn]: values }));
    const labels = vectorFromArray(["north"]);
    const sums = makeVector(new Float64Array([1234]));
    if (sql.includes(" as name")) return Promise.resolve(new Table({ name: labels, value: sums }));
    return Promise.resolve(new Table({ x: labels, y: sums }));
  }
});

const optionsOf = async (chart: ReactElement, valueColumn: string, values: Vector) => {
  render(chart);
  if (!captured.build) throw new Error("the chart did not build its options");
  return captured.build({
    display: captured.display,
    connection: connectionFor(valueColumn, values),
    fileName: "orders",
    isDarkMode: false
  });
};

/** What the y axis prints for 1234, or null when it has no formatter of its own. */
const axisLabel = (options: EChartsOption) => {
  const yAxis = options.yAxis as { axisLabel?: { formatter?: (value: number) => string } };
  return yAxis.axisLabel?.formatter?.(1234) ?? null;
};

/** What the pie's tooltip prints for a slice worth 1234. */
const sliceLabel = (options: EChartsOption) => {
  const tooltip = options.tooltip as { formatter: (params: unknown) => string };
  return tooltip.formatter({ name: "north", value: 1234 });
};

const amounts = () => makeVector(new Float64Array([12.5]));
const integers = () => makeVector(new Int32Array([7]));

afterEach(() => {
  cleanup();
  captured.build = null;
});

/** A chart of `y` by region, with the format the app declares for it, if any. */
type ChartOfY = (y: string, y_format?: DisplayFormat) => ReactElement;
const barChart: ChartOfY = (y, y_format) => (
  <BarChart display={{ type: "bar", data: "orders", x: "region", y, y_format }} data={{}} />
);
const lineChart: ChartOfY = (y, y_format) => (
  <LineChart display={{ type: "line", data: "orders", x: "region", y, y_format }} data={{}} />
);

describe.each<[string, ChartOfY]>([
  ["a bar chart", barChart],
  ["a line chart", lineChart]
])("the y axis of %s", (_name, chart) => {
  it("is in dollars when the y column is an amount of money", async () => {
    expect(axisLabel(await optionsOf(chart("total_sales"), "total_sales", amounts()))).toBe(
      "$1.2K"
    );
  });

  it("is a plain number when the y column is a count, whatever it counts", async () => {
    const options = await optionsOf(chart("payment_count"), "payment_count", integers());
    expect(axisLabel(options)).toBeNull();
  });

  it("follows the format the app declares over the column's name", async () => {
    expect(
      axisLabel(await optionsOf(chart("payment_count", "currency"), "payment_count", integers()))
    ).toBe("$1.2K");
    expect(
      axisLabel(await optionsOf(chart("total_sales", "number"), "total_sales", amounts()))
    ).toBe("1.2K");
  });
});

describe("the slices of a pie chart", () => {
  const chart = (value: string, value_format?: DisplayFormat) => (
    <PieChart
      display={{ type: "pie", data: "orders", name: "region", value, value_format }}
      data={{}}
    />
  );

  it("are in dollars when the value column is an amount of money", async () => {
    const options = await optionsOf(chart("total_sales"), "total_sales", amounts());
    expect(sliceLabel(options)).toBe("north: <b>$1,234.00</b>");
  });

  it("are plain numbers when the value column is a count", async () => {
    const options = await optionsOf(chart("payment_count"), "payment_count", integers());
    expect(sliceLabel(options)).toBe("north: <b>1234</b>");
  });

  it("follow the format the app declares over the column's name", async () => {
    const options = await optionsOf(
      chart("payment_count", "currency"),
      "payment_count",
      integers()
    );
    expect(sliceLabel(options)).toBe("north: <b>$1,234.00</b>");
  });
});
