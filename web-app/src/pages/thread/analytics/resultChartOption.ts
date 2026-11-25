import type { EChartsOption, SeriesOption } from "echarts";
import type { ResultChartData } from "@/services/api/analytics";

/**
 * ECharts options for a react `result_chart`, over its rows exactly as given.
 *
 * Nothing here sums, groups, sorts or fills: x positions keep the order the query
 * returned them in, a null stays a gap, and a value keeps its sign and digits. The
 * server already refused any chart whose (x, series) pair repeats, so each point is
 * one row and the chart cannot disagree with the query.
 */
export function resultChartOption(chart: ResultChartData, palette: string[]): EChartsOption {
  const yAt = chart.series ? 2 : 1;
  const common: EChartsOption = {
    color: palette,
    legend: chart.y.length > 1 || chart.series ? { type: "scroll", bottom: 0 } : undefined
  };
  switch (chart.kind) {
    case "pie":
      return {
        ...common,
        tooltip: { trigger: "item" },
        series: [
          {
            type: "pie",
            name: chart.y[0],
            // "-" is ECharts' empty value: a null slice is shown as missing, not as 0.
            data: chart.rows.map((r) => ({ name: label(r[0]), value: num(r[yAt]) ?? "-" }))
          }
        ]
      };
    case "scatter":
      return {
        ...common,
        tooltip: { trigger: "item" },
        xAxis: { type: "value", name: chart.x, scale: true },
        yAxis: { type: "value", name: chart.y.length === 1 ? chart.y[0] : undefined },
        series: scatterSeries(chart, yAt)
      };
    default:
      return categorySeries(chart, common, yAt);
  }
}

/** A cell as an axis or slice label: null is named, never dropped. */
function label(v: unknown): string {
  if (v === null || v === undefined) return "(null)";
  if (typeof v === "string") return v;
  return typeof v === "number" || typeof v === "boolean" ? String(v) : JSON.stringify(v);
}

/** A cell as a value: a number as given, anything else a gap. */
function num(v: unknown): number | null {
  return typeof v === "number" ? v : null;
}

/** Distinct labels in the order they first appear. */
function firstSeen(values: unknown[]): string[] {
  return [...new Set(values.map(label))];
}

function categorySeries(chart: ResultChartData, common: EChartsOption, yAt: number): EChartsOption {
  const type = chart.kind === "bar" ? "bar" : "line";
  // Without a series every x is distinct (the server checked), so each row is one
  // position; labels are not deduped, as 1 and "1" would collapse into one.
  const xs = chart.series
    ? firstSeen(chart.rows.map((r) => r[0]))
    : chart.rows.map((r) => label(r[0]));
  let series: SeriesOption[];
  if (chart.series) {
    // One line or bar set per series value, each aligned to the shared x positions.
    // A pair the result lacks is a gap, not a zero.
    series = firstSeen(chart.rows.map((r) => r[1])).map((name) => {
      const at = new Map(
        chart.rows.filter((r) => label(r[1]) === name).map((r) => [label(r[0]), num(r[yAt])])
      );
      return { type, name, data: xs.map((x) => at.get(x) ?? null) };
    });
  } else {
    series = chart.y.map((name, i) => ({
      type,
      name,
      data: chart.rows.map((r) => num(r[yAt + i]))
    }));
  }
  return {
    ...common,
    tooltip: { trigger: "axis" },
    xAxis: { type: "category", name: chart.x, data: xs },
    yAxis: { type: "value", name: chart.y.length === 1 ? chart.y[0] : undefined },
    series
  };
}

function scatterSeries(chart: ResultChartData, yAt: number): SeriesOption[] {
  if (chart.series) {
    return firstSeen(chart.rows.map((r) => r[1])).map((name) => ({
      type: "scatter",
      name,
      data: chart.rows.filter((r) => label(r[1]) === name).map((r) => [num(r[0]), num(r[yAt])])
    }));
  }
  return chart.y.map((name, i) => ({
    type: "scatter",
    name,
    data: chart.rows.map((r) => [num(r[0]), num(r[yAt + i])])
  }));
}
