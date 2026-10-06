import type { BarSeriesOption, EChartsOption } from "echarts";
import { resolveColor } from "@/components/Echarts/resolveColor";
import type { ApiKeyUsageDay } from "@/types/apiKey";
import { okCount } from "../../activity";

/**
 * 30 days of requests as stacked columns: OK at the base, 4xx then 5xx on top.
 *
 * Color by job, not by series count. OK takes a step of the sequential blue ramp (the same
 * one the storage charts use); 4xx and 5xx take the reserved status tokens, so an error reads
 * as an error wherever it appears. Each carries a text label in the legend beside it, so
 * identity never rests on color alone.
 *
 * ECharts paints to canvas and can't read CSS variables, so tokens are resolved when the
 * option is built. `_themeKey` is unused on purpose: taking it makes the caller rebuild the
 * option when the theme flips, which a `useMemo` dep list would otherwise not know about.
 */
export const USAGE_TOKENS = {
  ok: "--chart-seq-4",
  errors4xx: "--warning",
  errors5xx: "--destructive"
} as const;

type SeriesKey = keyof typeof USAGE_TOKENS;

const segmentValue = (d: ApiKeyUsageDay, key: SeriesKey) =>
  key === "ok" ? okCount(d) : key === "errors4xx" ? d.errors_4xx : d.errors_5xx;

/** The topmost non-empty segment of a day's stack: the only one whose top is rounded. */
const topKey = (d: ApiKeyUsageDay): SeriesKey | null => {
  if (d.errors_5xx > 0) return "errors5xx";
  if (d.errors_4xx > 0) return "errors4xx";
  return okCount(d) > 0 ? "ok" : null;
};

const shortDay = (day: string) =>
  new Date(`${day}T00:00:00Z`).toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    timeZone: "UTC"
  });

const tooltipHtml = (d: ApiKeyUsageDay) => {
  const n = (v: number) => v.toLocaleString("en-US");
  return [
    `<div style="font-size:11px">${shortDay(d.day)}</div>`,
    `<div><strong>${n(d.requests)}</strong> requests</div>`,
    `<div><strong>${n(d.errors_4xx)}</strong> 4xx</div>`,
    `<div><strong>${n(d.errors_5xx)}</strong> 5xx</div>`
  ].join("");
};

export function usageChartOption(days: ApiKeyUsageDay[], _themeKey: string): EChartsOption {
  const surface = resolveColor("--background");
  const axis = resolveColor("--muted-foreground");
  const grid = resolveColor("--border");

  const series = (Object.keys(USAGE_TOKENS) as SeriesKey[]).map(
    (key): BarSeriesOption => ({
      type: "bar",
      name: key,
      stack: "requests",
      barMaxWidth: 10,
      barCategoryGap: "30%",
      // The 1px surface-colored border is the gap between stacked segments.
      itemStyle: { color: resolveColor(USAGE_TOKENS[key]), borderColor: surface, borderWidth: 1 },
      emphasis: { disabled: true },
      data: days.map((d) => ({
        value: segmentValue(d, key),
        itemStyle: { borderRadius: topKey(d) === key ? [2, 2, 0, 0] : 0 }
      }))
    })
  );

  return {
    animation: false,
    grid: { top: 2, right: 0, bottom: 16, left: 0 },
    tooltip: {
      trigger: "axis",
      axisPointer: { type: "shadow", shadowStyle: { color: grid, opacity: 0.4 } },
      confine: true,
      formatter: (params: unknown) => {
        const index = (params as { dataIndex: number }[])?.[0]?.dataIndex;
        const day = index === undefined ? undefined : days[index];
        return day ? tooltipHtml(day) : "";
      }
    },
    xAxis: {
      type: "category",
      data: days.map((d) => d.day),
      axisLine: { lineStyle: { color: grid } },
      axisTick: { show: false },
      axisLabel: {
        color: axis,
        fontSize: 10,
        // First and last day only: the window's two ends are the only dates worth reading.
        interval: (index: number) => index === 0 || index === days.length - 1,
        formatter: shortDay,
        alignMinLabel: "left",
        alignMaxLabel: "right"
      }
    },
    yAxis: { type: "value", min: 0, show: false },
    series
  };
}
