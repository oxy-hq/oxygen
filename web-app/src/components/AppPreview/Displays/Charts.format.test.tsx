// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import {
  makeVector,
  Table,
  TimestampMillisecond,
  type Vector,
  vectorFromArray
} from "apache-arrow";
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
const connectionFor = (
  valueColumn: string,
  values: Vector,
  sums: Vector = makeVector(new Float64Array([1234]))
) => ({
  query: (sql: string) => {
    if (sql.includes("LIMIT 0")) return Promise.resolve(new Table({ [valueColumn]: values }));
    const labels = vectorFromArray(["north"]);
    if (sql.includes(" as name")) return Promise.resolve(new Table({ name: labels, value: sums }));
    return Promise.resolve(new Table({ x: labels, y: sums }));
  }
});

const optionsOf = async (
  chart: ReactElement,
  valueColumn: string,
  values: Vector,
  sums?: Vector
) => {
  render(chart);
  if (!captured.build) throw new Error("the chart did not build its options");
  return captured.build({
    display: captured.display,
    connection: connectionFor(valueColumn, values, sums),
    fileName: "orders",
    isDarkMode: false
  });
};

/** The values the chart's first series plots, as ECharts reads them. */
const plotted = (options: EChartsOption) => {
  const [series] = options.series as { data: unknown[] }[];
  return series.data.map((value) => Number(value));
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

  it("plots a small value as itself, not rounded to two places", async () => {
    // A ratio: no format, so ECharts labels the axis ticks and the tooltip itself.
    const ratios = () => makeVector(new Float64Array([0.004]));
    const options = await optionsOf(chart("conversion"), "conversion", ratios(), ratios());

    // It used to reach ECharts as "0.00", and was drawn at 0.
    expect(plotted(options)).toEqual([0.004]);
    expect(axisLabel(options)).toBeNull();
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

  it("are as small as their values, and say so, when the value column has no format", async () => {
    const ratios = () => makeVector(new Float64Array([0.004]));
    const options = await optionsOf(chart("conversion"), "conversion", ratios(), ratios());

    const [series] = options.series as { data: { value: unknown }[] }[];
    expect(series.data.map((slice) => Number(slice.value))).toEqual([0.004]);
    const tooltip = options.tooltip as { formatter: (params: unknown) => string };
    expect(tooltip.formatter({ name: "north", value: series.data[0].value })).toBe(
      "north: <b>0.004</b>"
    );
  });
});

/** One point of a series: its category and its value at a moment. */
type Point = { series: string; at: string; y: number };

/**
 * A source table of `points`, as the chart's queries see it: the x axis is
 * every distinct moment, and each series' query only its own points.
 */
const seriesConnectionFor = (points: Point[]) => {
  const moments = (list: Point[]) =>
    vectorFromArray(
      list.map((point) => new Date(point.at)),
      new TimestampMillisecond()
    );
  return {
    query: (sql: string) => {
      if (sql.includes("LIMIT 0")) {
        return Promise.resolve(new Table({ conversion: makeVector(new Float64Array([1])) }));
      }
      const distinct = [...new Set(points.map((point) => point.at))].sort();
      return Promise.resolve(
        new Table({ x: moments(distinct.map((at) => ({ series: "", at, y: 0 }))) })
      );
    },
    prepare: (sql: string) =>
      Promise.resolve({
        query: (series?: unknown) => {
          if (sql.includes(" as series")) {
            const names = [...new Set(points.map((point) => point.series))];
            return Promise.resolve(new Table({ series: vectorFromArray(names) }));
          }
          const own = points.filter((point) => point.series === series);
          return Promise.resolve(
            new Table({
              x: moments(own),
              y: makeVector(new Float64Array(own.map((point) => point.y)))
            })
          );
        }
      })
  };
};

describe.each<[string, (series: string) => ReactElement]>([
  [
    "a bar chart",
    (series) => (
      <BarChart display={{ type: "bar", data: "orders", x: "at", y: "conversion", series }} />
    )
  ],
  [
    "a line chart",
    (series) => (
      <LineChart display={{ type: "line", data: "orders", x: "at", y: "conversion", series }} />
    )
  ]
])("the series of %s over time", (_name, chart) => {
  const optionsFor = async (points: Point[]) => {
    render(chart("region"));
    if (!captured.build) throw new Error("the chart did not build its options");
    const options = await captured.build({
      display: captured.display,
      connection: seriesConnectionFor(points),
      fileName: "orders",
      isDarkMode: false
    });
    const xAxis = options.xAxis as { data: unknown[] };
    const series = options.series as { data: unknown[] }[];
    return { labels: xAxis.data, data: series.map((one) => one.data) };
  };

  it("keeps two moments within one minute apart, and labels them to the second", async () => {
    const { labels, data } = await optionsFor([
      { series: "north", at: "2024-03-05T12:34:56Z", y: 1 },
      { series: "north", at: "2024-03-05T12:34:57Z", y: 2 }
    ]);

    // Both used to be "2024-03-05 12:34", and the second point overwrote the first.
    expect({ labels, data }).toEqual({
      labels: ["2024-03-05 12:34:56", "2024-03-05 12:34:57"],
      data: [["1", "2"]]
    });
  });

  it("puts each series' points at their own moments when one series is finer than another", async () => {
    const { labels, data } = await optionsFor([
      { series: "north", at: "2024-03-05T12:34:00Z", y: 1 },
      { series: "south", at: "2024-03-05T12:34:30Z", y: 2 }
    ]);

    expect({ labels, data }).toEqual({
      labels: ["2024-03-05 12:34:00", "2024-03-05 12:34:30"],
      data: [
        ["1", null],
        [null, "2"]
      ]
    });
  });

  it("labels moments to the minute when none of them has seconds", async () => {
    const { labels, data } = await optionsFor([
      { series: "north", at: "2024-03-05T12:34:00Z", y: 1 },
      { series: "north", at: "2024-03-05T12:35:00Z", y: 2 }
    ]);

    expect(labels).toEqual(["2024-03-05 12:34", "2024-03-05 12:35"]);
    expect(data).toEqual([["1", "2"]]);
  });
});

describe.each<[string, (series: string) => ReactElement]>([
  [
    "a bar chart",
    (series) => (
      <BarChart display={{ type: "bar", data: "orders", x: "region", y: "conversion", series }} />
    )
  ],
  [
    "a line chart",
    (series) => (
      <LineChart display={{ type: "line", data: "orders", x: "region", y: "conversion", series }} />
    )
  ]
])("%s whose series are moments with a time zone", (_name, chart) => {
  it("selects each series' rows by its exact instant", async () => {
    // 01:30 in New York, twice: once before the clocks go back, once after.
    const instants = ["2024-11-03T05:30:00Z", "2024-11-03T06:30:00Z"];
    const bound: unknown[] = [];
    const connection = {
      query: (sql: string) =>
        Promise.resolve(
          sql.includes("LIMIT 0")
            ? new Table({ conversion: makeVector(new Float64Array([1])) })
            : new Table({ x: vectorFromArray(["north"]) })
        ),
      prepare: (sql: string) =>
        Promise.resolve({
          query: (series?: unknown) => {
            if (sql.includes(" as series")) {
              return Promise.resolve(
                new Table({
                  series: vectorFromArray(
                    instants.map((at) => new Date(at)),
                    new TimestampMillisecond("America/New_York")
                  )
                })
              );
            }
            bound.push(series);
            return Promise.resolve(
              new Table({
                x: vectorFromArray(["north"]),
                y: makeVector(new Float64Array([bound.length]))
              })
            );
          }
        })
    };

    render(chart("at"));
    if (!captured.build) throw new Error("the chart did not build its options");
    await captured.build({
      display: captured.display,
      connection,
      fileName: "orders",
      isDarkMode: false
    });

    // Each used to be bound as the clock time it showed, "2024-11-03 01:30" for
    // both, which DuckDB reads in its own time zone: rows of neither, or one twice.
    expect(bound).toEqual(["2024-11-03T05:30:00.000Z", "2024-11-03T06:30:00.000Z"]);
  });
});
