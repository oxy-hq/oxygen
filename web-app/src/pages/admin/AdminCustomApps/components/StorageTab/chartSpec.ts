import type { EChartsOption } from "echarts";
import { resolveColor, resolveColorWithAlpha } from "@/components/Echarts/resolveColor";
import type { StorageHistoryPoint } from "@/types/apps";
import { formatBytes } from "./utils";

/**
 * Shared chart spec for the Storage tab.
 *
 * ## Why one hue and no legend
 *
 * Every chart here plots **one** series — bytes held over time, or one app's
 * split by prefix. A single series needs no legend (the heading names it) and no
 * categorical palette; it takes a step from the sequential blue ramp.
 *
 * That is not only the safe default, it is the only correct option here: the
 * repo's categorical tokens fail a CVD/normal-vision check as a set —
 * `--chart-4` (#ffb900) and `--chart-5` (#fe9a00) sit ΔE 7.4 apart in *normal*
 * vision, well under the 15 floor, and `--chart-3` (#104e64) reads gray. A
 * five-series chart built on them would be unreadable for everyone, not just
 * CVD readers. Fixing those tokens is a design-system change, out of scope here;
 * avoiding them is free.
 *
 * ## Colors are resolved, not referenced
 *
 * ECharts paints to canvas and cannot read a CSS custom property, so tokens are
 * resolved to concrete values at build time via `resolveColor`, which reads
 * `getComputedStyle`. The option object must therefore be rebuilt when the theme
 * changes.
 *
 * Both builders take a `_themeKey` they never read, for exactly that reason: the
 * dependency is on the DOM's current theme, which React's exhaustive-deps lint
 * cannot see. Taking it as an argument turns an invisible, comment-only contract
 * into one the linter enforces — a caller that forgets it fails the build rather
 * than silently rendering last theme's colors until the next refetch.
 */

/** The one hue every chart on this surface uses. */
const SERIES_TOKEN = "--chart-seq-4";

/** Bytes no retention rule covers. Reserved status color, never a series hue. */
const UNTAGGED_TOKEN = "--warning";

/** Axis/grid ink. Recessive on purpose — the data is the figure, not the frame. */
const AXIS_TOKEN = "--muted-foreground";
const GRID_TOKEN = "--border";

/**
 * Y-axis tick labels. Bytes are unreadable raw, and a chart whose axis says
 * `4294967296` has failed before anyone reads the shape.
 */
const axisLabelFormatter = (value: number) => formatBytes(value);

/**
 * Usage-over-time area chart.
 *
 * Area rather than line because a single series over time reads as a *level* —
 * how much is held — and the filled region says "quantity" where a bare line
 * says "rate". The fill is a low-alpha gradient so it never competes with the
 * 2px stroke that carries the actual values.
 */
export function usageOverTimeOption(
  points: StorageHistoryPoint[],
  _themeKey: string
): EChartsOption {
  const line = resolveColor(SERIES_TOKEN);
  const axis = resolveColor(AXIS_TOKEN);
  const grid = resolveColor(GRID_TOKEN);

  return {
    grid: { top: 16, right: 16, bottom: 24, left: 56, containLabel: false },
    // Crosshair + shared tooltip: on a dense daily series, hitting an exact
    // point with the cursor is not a reasonable thing to ask of anyone.
    tooltip: {
      trigger: "axis",
      axisPointer: { type: "line", lineStyle: { color: axis, width: 1 } },
      formatter: (params: unknown) => {
        const rows = params as { axisValue: string; data: number }[];
        const row = rows?.[0];
        if (!row) return "";
        return `${row.axisValue}<br/><strong>${formatBytes(row.data)}</strong>`;
      }
    },
    xAxis: {
      type: "category",
      data: points.map((p) => p.date),
      boundaryGap: false,
      axisLine: { lineStyle: { color: grid } },
      axisTick: { show: false },
      axisLabel: {
        color: axis,
        fontSize: 10,
        // A 90-day window cannot show 90 dates; let ECharts thin them.
        hideOverlap: true,
        formatter: (value: string) => value.slice(5)
      }
    },
    yAxis: {
      type: "value",
      axisLabel: { color: axis, fontSize: 10, formatter: axisLabelFormatter },
      // Horizontal rules only, dashed and recessive: enough to read a value
      // off, not enough to compete with the series.
      splitLine: { lineStyle: { color: grid, type: "dashed", opacity: 0.6 } }
    },
    series: [
      {
        type: "line",
        data: points.map((p) => p.bytes),
        smooth: false,
        showSymbol: false,
        // Big enough to hit on hover without drawing a dot on every day.
        symbolSize: 8,
        lineStyle: { width: 2, color: line },
        itemStyle: { color: line },
        areaStyle: {
          color: {
            type: "linear",
            x: 0,
            y: 0,
            x2: 0,
            y2: 1,
            colorStops: [
              { offset: 0, color: resolveColorWithAlpha(SERIES_TOKEN, 0.28) },
              { offset: 1, color: resolveColorWithAlpha(SERIES_TOKEN, 0.02) }
            ]
          }
        }
      }
    ]
  };
}

/** One slice of the prefix composition bar. */
export interface PrefixSlice {
  prefix: string;
  bytes: number;
  expireAfter?: string;
}

/**
 * Prefix composition as a single stacked horizontal bar.
 *
 * Part-to-whole, and the reader's job is "which prefix dominates, and is it
 * covered?" — magnitude plus one status, not identity. So the slices take
 * ordered steps from the sequential ramp (largest darkest) and anything without
 * a retention rule takes the reserved warning color instead. Identity is carried
 * by the labels beneath, never by hue alone.
 *
 * Horizontal because prefix names are long and a vertical stack would rotate them.
 */
export function prefixCompositionOption(slices: PrefixSlice[], _themeKey: string): EChartsOption {
  // Largest first so the ramp reads dark → light left to right.
  const ordered = [...slices].sort((a, b) => b.bytes - a.bytes);
  const rampSteps = ["--chart-seq-5", "--chart-seq-4", "--chart-seq-3", "--chart-seq-2"];

  return {
    grid: { top: 0, right: 0, bottom: 0, left: 0, containLabel: false },
    tooltip: {
      trigger: "item",
      formatter: (params: unknown) => {
        const p = params as { seriesName: string; value: number };
        const slice = ordered.find((s) => s.prefix === p.seriesName);
        const rule = slice?.expireAfter ? `expires ${slice.expireAfter}` : "no retention rule";
        return `<strong>${p.seriesName}</strong><br/>${formatBytes(p.value)}<br/>${rule}`;
      }
    },
    xAxis: { type: "value", show: false, max: "dataMax" },
    yAxis: { type: "category", show: false, data: [""] },
    series: ordered.map((slice, i) => ({
      name: slice.prefix,
      type: "bar" as const,
      stack: "total",
      data: [slice.bytes],
      barWidth: 14,
      itemStyle: {
        color: slice.expireAfter
          ? resolveColor(rampSteps[Math.min(i, rampSteps.length - 1)])
          : resolveColor(UNTAGGED_TOKEN),
        // 2px surface gap between segments so adjacent fills never fuse into
        // one block — the separation that makes a stacked bar readable.
        borderColor: resolveColor("--background"),
        borderWidth: 2,
        // Rounded outer ends only; interior joins stay square so the bar reads
        // as one quantity split up, not as separate pills. The single-slice case
        // is checked FIRST — it is both the first and the last segment, and the
        // ordered ternary would otherwise leave its right end square.
        borderRadius:
          ordered.length === 1
            ? 4
            : i === 0
              ? [4, 0, 0, 4]
              : i === ordered.length - 1
                ? [0, 4, 4, 0]
                : 0
      },
      emphasis: { itemStyle: { borderColor: resolveColor("--background"), borderWidth: 2 } },
      label: { show: false }
    }))
  };
}
